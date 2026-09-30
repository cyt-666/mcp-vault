use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Permission, PermissionSet, Revision, SourcePlane, VaultContext, VaultId, VaultPath,
    VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_eval::{
    EvalSource, EvalTask, EvaluationManifest, ShadowBuildConfig, ShadowCatchupState, TaskSplit,
    prepare_shadow_dry_run, validate_manifest,
};
use mcp_vault_memory::{
    SemanticAccess, SemanticActor, SemanticMemoryService, SemanticMutation, SemanticPublicFacade,
    SemanticRuleCommand,
};
use mcp_vault_state::{
    SemanticExtractionRecord, SemanticSourceRecord, SemanticSourceRevisionRecord, StateStore,
    VaultStatus,
};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::json;

struct VaultFixture {
    state: StateStore,
    context: VaultContext,
    core: VaultCore,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShadowReadTarget {
    Source,
    Shadow,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShadowReadFence {
    manifest: EvaluationManifest,
    manifest_hash: String,
    source_id: String,
    source_file_id: String,
    source_path: VaultPath,
    source_revision: String,
    source_file_revision: u64,
    source_content_hash: String,
    rules_revision: i64,
    source_outbox_checkpoint: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShadowReadClient {
    target: ShadowReadTarget,
    fence: ShadowReadFence,
}

impl ShadowReadClient {
    fn for_source(fence: ShadowReadFence) -> Self {
        Self {
            target: ShadowReadTarget::Source,
            fence,
        }
    }

    async fn switch_to_shadow(
        &mut self,
        source: &VaultFixture,
        shadow: &VaultFixture,
    ) -> Result<(), &'static str> {
        let previous_target = self.target;
        self.target = ShadowReadTarget::Shadow;
        if self.verify_fence(source, shadow).await.is_err() {
            self.target = previous_target;
            return Err("shadow read fence drifted");
        }
        Ok(())
    }

    async fn read_card_ids(
        &self,
        source: &VaultFixture,
        shadow: &VaultFixture,
    ) -> Result<Vec<String>, &'static str> {
        self.verify_fence(source, shadow).await?;
        let fixture = match self.target {
            ShadowReadTarget::Source => source,
            ShadowReadTarget::Shadow => shadow,
        };
        let facade = SemanticPublicFacade::new(fixture.state.clone());
        let access = SemanticAccess::new(PermissionSet::from_iter([
            Permission::ReadMemory,
            Permission::ReadVault,
        ]));
        let (cards, _) = facade
            .list_cards(&fixture.context, &fixture.core, &access, 20)
            .await
            .map_err(|_| "semantic read failed")?;
        Ok(cards.into_iter().map(|card| card.card_id).collect())
    }

    async fn verify_fence(
        &self,
        source: &VaultFixture,
        shadow: &VaultFixture,
    ) -> Result<(), &'static str> {
        let source_file = source
            .core
            .read(&source.context, &self.fence.source_path)
            .await
            .map_err(|_| "source file fence unavailable")?
            .file;
        if source_file.id.to_string() != self.fence.source_file_id
            || source_file.current_revision.value() != self.fence.source_file_revision
            || source_file.content_hash.as_deref() != Some(self.fence.source_content_hash.as_str())
        {
            return Err("shadow read fence drifted");
        }
        let source_record = source
            .state
            .semantic_memory()
            .get_source_by_file(&source.context, source_file.id)
            .await
            .map_err(|_| "source semantic fence unavailable")?
            .ok_or("source semantic fence unavailable")?;
        if source_record.source_id.to_string() != self.fence.source_id
            || source_record.current_revision_id.map(|id| id.to_string())
                != Some(self.fence.source_revision.clone())
        {
            return Err("shadow read fence drifted");
        }
        let mut current_manifest = self.fence.manifest.clone();
        let manifest_source = current_manifest
            .sources
            .iter_mut()
            .find(|entry| entry.logical_id == "S01")
            .ok_or("source manifest entry missing")?;
        manifest_source.file_revision = source_file.current_revision.value();
        manifest_source.content_hash = source_file
            .content_hash
            .clone()
            .ok_or("source content hash missing")?;
        manifest_source.source_revision_id = source_record
            .current_revision_id
            .map(|id| id.to_string())
            .ok_or("source revision missing")?;
        if validate_manifest(&current_manifest).map_err(|_| "source manifest invalid")?
            != self.fence.manifest_hash
        {
            return Err("source manifest fence drifted");
        }
        let source_outbox_checkpoint = source
            .state
            .files()
            .count_outbox_events(&source.context, &self.fence.source_file_id)
            .await
            .map_err(|_| "source outbox checkpoint unavailable")?;
        if source_outbox_checkpoint != self.fence.source_outbox_checkpoint {
            return Err("source outbox checkpoint drifted");
        }
        if self.target == ShadowReadTarget::Shadow
            && shadow
                .state
                .semantic_rules()
                .current_rules_revision(&shadow.context)
                .await
                .map_err(|_| "shadow rules fence unavailable")?
                != self.fence.rules_revision
        {
            return Err("shadow rules fence drifted");
        }
        Ok(())
    }
}

fn count_files(root: &std::path::Path) -> usize {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| {
                    if entry.path().is_dir() {
                        count_files(&entry.path())
                    } else {
                        1
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

async fn vault_fixture(
    database: &std::path::Path,
    root: &std::path::Path,
    slug: &str,
) -> VaultFixture {
    vault_fixture_with_id(database, root, slug, VaultId::new()).await
}

async fn vault_fixture_with_id(
    database: &std::path::Path,
    root: &std::path::Path,
    slug: &str,
    vault_id: VaultId,
) -> VaultFixture {
    let state = StateStore::connect_and_migrate(&format!("sqlite://{}", database.display()))
        .await
        .unwrap();
    let context = VaultContext::new(
        vault_id,
        VaultSlug::new(slug).unwrap(),
        root.to_owned(),
        Revision::ZERO,
    )
    .unwrap();
    if state
        .vaults()
        .find_by_id(context.id())
        .await
        .unwrap()
        .is_none()
    {
        state
            .vaults()
            .insert(&context, slug, VaultStatus::Active)
            .await
            .unwrap();
        state
            .settings()
            .set_vault(
                &context,
                "memory.units.policy",
                &json!({"enabled":true,"request_timeout_seconds":300}),
                WritePrecondition::Unconditional,
                None,
            )
            .await
            .unwrap();
    }
    let core = VaultCore::new(
        state.clone(),
        root.parent()
            .unwrap_or(root)
            .join(format!("{slug}-history")),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    VaultFixture {
        state,
        context,
        core,
    }
}

fn manifest_for(
    source: &SemanticSourceRecord,
    revision: &SemanticSourceRevisionRecord,
    extraction: &SemanticExtractionRecord,
) -> EvaluationManifest {
    EvaluationManifest {
        schema_version: "semantic-memory-eval-manifest-v1".into(),
        dataset_id: "m7-shadow-fixture".into(),
        synthetic_only: true,
        sources: vec![EvalSource {
            logical_id: "S01".into(),
            synthetic_placeholder: false,
            vault_id: source.vault_id.to_string(),
            file_id: source.file_id.to_string(),
            path: source.source_path.to_string(),
            file_revision: revision.file_revision.value(),
            content_hash: revision.content_hash.clone(),
            logical_block_count: 0,
            source_revision_id: revision.source_revision_id.to_string(),
            authorization_revision: extraction.authorization_revision,
            profile_id: extraction.profile_id.clone(),
            rules_revision: extraction.rules_revision,
            source_generation: extraction.source_generation,
            extraction_commit_sequence: extraction.extraction_commit_sequence,
        }],
        tasks: vec![EvalTask {
            id: "T01".into(),
            split: TaskSplit::Development,
            query_refs: vec!["Q01".into()],
            query: "What is the shadow decision?".into(),
            source_ids: vec!["S01".into()],
            source_fence: vec![],
            must_preserve: vec!["scope".into()],
            must_not_infer: vec!["provider quality".into()],
            expected_usable_information: vec!["fixture decision".into()],
            expected_no_answer: false,
            expected_status: "available".into(),
            expected_source_relations: vec![],
            severity: "high".into(),
        }],
    }
}

#[tokio::test]
async fn isolated_shadow_publishes_reads_and_persists_suppression_without_touching_source() {
    let temp = tempfile::tempdir().unwrap();
    let source_root = temp.path().join("source");
    let shadow_root = temp.path().join("shadow");
    let source_db = temp.path().join("source.sqlite3");
    let shadow_db = temp.path().join("shadow-state.sqlite3");
    let source_history_root = source_root.parent().unwrap().join("source-history");
    let shadow_history_root = shadow_root.parent().unwrap().join("shadow-history");
    let source = vault_fixture(&source_db, &source_root, "source").await;
    let path = VaultPath::parse("notes/shadow.md").unwrap();
    let bytes = b"# Shadow\nThe isolated fixture decision is retained.\n";
    source
        .core
        .create_bytes(
            &source.context,
            &path,
            bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let source_memory = SemanticMemoryService::new(source.state.clone());
    let source_input = source_memory
        .prepare_source(&source.context, &source.core, &path)
        .await
        .unwrap();
    let source_block = source_input.blocks.last().unwrap();
    let source_proposal = json!({"outcome":"success_nonempty","observations":[{"kind":"decision","statement":"The isolated fixture decision is retained.","scope":"project","assertion_status":"source_asserted","admission_reason":"source fixture","value_for_future_work":"structure only","body_block_ids":[source_block.local_id.clone()]}],"cards":[{"title":"Source fixture decision","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]});
    let source_submission = source_memory
        .submit_proposal_json(
            &source.context,
            &source.core,
            &path,
            &source_proposal.to_string(),
        )
        .await
        .unwrap();
    let source_file = source.core.read(&source.context, &path).await.unwrap().file;
    let source_record = source
        .state
        .semantic_memory()
        .get_source_by_file(&source.context, source_file.id)
        .await
        .unwrap()
        .unwrap();
    let source_revision = source
        .state
        .semantic_memory()
        .get_source_revision(
            &source.context,
            source_submission.extraction.source_revision_id,
        )
        .await
        .unwrap()
        .unwrap();
    let source_cards_before = source_memory
        .list_cards(&source.context, &source.core, 20)
        .await
        .unwrap();
    let source_file_before = source.core.read(&source.context, &path).await.unwrap().file;
    let source_file_count_before = count_files(&source_root);
    let source_history_before = count_files(&source_history_root);
    let manifest = manifest_for(
        &source_record,
        &source_revision,
        &source_submission.extraction,
    );
    let shadow = vault_fixture(&shadow_db, &shadow_root, "shadow").await;
    let manifest_hash = validate_manifest(&manifest).unwrap();
    let source_before = std::fs::read(source_root.join("notes/shadow.md")).unwrap();
    let shadow_config = ShadowBuildConfig {
        manifest: manifest.clone(),
        source_root: source_root.display().to_string(),
        shadow_root: shadow_root.display().to_string(),
        shadow_state: temp
            .path()
            .join("shadow-state.sqlite3")
            .display()
            .to_string(),
        shadow_history: shadow_history_root.display().to_string(),
        artifact_root: temp.path().join("shadow-artifacts").display().to_string(),
        source_allowlist: vec!["S01".into(), source_file.id.to_string()],
        outbox_cursor: 0,
        rules_revision: 1,
        shadow_vault_id: shadow.context.id().to_string(),
    };
    let plan = prepare_shadow_dry_run(
        &shadow_config,
        &ShadowCatchupState {
            outbox_cursor: 0,
            rules_revision: 1,
            manifest_hash,
        },
    )
    .unwrap();
    assert!(!plan.provider_started && !plan.source_publish_attempted);
    assert_eq!(
        std::fs::read(source_root.join("notes/shadow.md")).unwrap(),
        source_before
    );
    assert_eq!(count_files(&source_root), source_file_count_before);
    let mut drifted = shadow_config.clone();
    drifted.manifest.sources[0].content_hash = "b".repeat(64);
    assert!(
        prepare_shadow_dry_run(
            &drifted,
            &ShadowCatchupState {
                outbox_cursor: 0,
                rules_revision: 1,
                manifest_hash: validate_manifest(&shadow_config.manifest).unwrap()
            }
        )
        .is_err()
    );

    assert_ne!(source.context.id(), shadow.context.id());
    assert_eq!(plan.shadow_vault_id, shadow_config.shadow_vault_id);
    shadow
        .core
        .reconcile(&shadow.context, Actor::system())
        .await
        .unwrap();
    let shadow_file = shadow.core.read(&shadow.context, &path).await.unwrap().file;
    assert_ne!(shadow_file.id, source_file.id);
    assert_ne!(source_history_root, shadow_history_root);
    let memory = SemanticMemoryService::new(shadow.state.clone());
    let input = memory
        .prepare_source(&shadow.context, &shadow.core, &path)
        .await
        .unwrap();
    let block = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{"kind":"decision","statement":"The isolated fixture decision is retained.","scope":"project","assertion_status":"source_asserted","admission_reason":"local shadow test","value_for_future_work":"structure only","body_block_ids":[block.local_id.clone()]}],
        "cards":[{"title":"Shadow fixture decision","kind":"decision","scope":"project","assertion_status":"source_asserted","observation_indices":[0]}]
    });
    memory
        .submit_proposal_json(&shadow.context, &shadow.core, &path, &proposal.to_string())
        .await
        .unwrap();
    let cards = memory
        .list_cards(&shadow.context, &shadow.core, 20)
        .await
        .unwrap();
    assert_eq!(cards.len(), 1);
    let source_cards_after_shadow_publish = source_memory
        .list_cards(&source.context, &source.core, 20)
        .await
        .unwrap();
    assert!(!source_cards_after_shadow_publish.is_empty());

    let source_outbox_checkpoint = source
        .state
        .files()
        .count_outbox_events(&source.context, &source_file.id.to_string())
        .await
        .unwrap();
    let shadow_rules_revision = shadow
        .state
        .semantic_rules()
        .current_rules_revision(&shadow.context)
        .await
        .unwrap();
    let read_fence = ShadowReadFence {
        manifest: manifest.clone(),
        manifest_hash: plan.manifest_hash.clone(),
        source_id: source_record.source_id.to_string(),
        source_file_id: source_file.id.to_string(),
        source_path: path.clone(),
        source_revision: source_revision.source_revision_id.to_string(),
        source_file_revision: source_file_before.current_revision.value(),
        source_content_hash: source_file_before.content_hash.clone().unwrap(),
        rules_revision: shadow_rules_revision,
        source_outbox_checkpoint,
    };
    let mut read_client = ShadowReadClient::for_source(read_fence.clone());
    let source_card_id = source_cards_before[0].id.to_string();
    let source_target_ids = read_client.read_card_ids(&source, &shadow).await.unwrap();
    assert!(source_target_ids.contains(&source_card_id));
    read_client
        .switch_to_shadow(&source, &shadow)
        .await
        .unwrap();

    let card = &cards[0];
    let shadow_target_ids = read_client.read_card_ids(&source, &shadow).await.unwrap();
    assert_eq!(shadow_target_ids, vec![card.id.to_string()]);
    let canonical_path = card.canonical_path.clone();
    let shadow_card_file = shadow
        .context
        .content_root()
        .join(card.canonical_path.as_str());
    assert!(shadow_card_file.is_file());
    assert!(shadow_card_file.starts_with(&shadow_root));
    assert!(!shadow_card_file.starts_with(&source_root));
    assert!(!shadow_history_root.starts_with(&source_root));
    assert!(
        shadow
            .core
            .read_managed(&shadow.context, &card.canonical_path)
            .await
            .is_ok()
    );
    assert_ne!(card.vault_id, source.context.id());
    assert!(
        !shadow
            .context
            .content_root()
            .join(card.canonical_path.as_str())
            .starts_with(&source_root)
    );
    let mut stale_source_client = ShadowReadClient::for_source(read_fence.clone());
    let facade = SemanticPublicFacade::new(shadow.state.clone());
    facade
        .apply_rule(
            &shadow.context,
            &SemanticAccess::new(PermissionSet::from_iter([Permission::ManageMemory])),
            &SemanticActor::trusted("m7-shadow-test"),
            &SemanticRuleCommand {
                target_ref: format!("card:{}", card.id),
                parent_ref: None,
                source_id: None,
                source_revision_id: None,
                mutation: SemanticMutation::SuppressRead,
                payload: Some(json!({"reason":"shadow-only"})),
                expected_parent_revision: Some(card.revision_number),
                expected_rules_revision: Some(0),
                idempotency_key: "shadow-suppress-once".into(),
            },
        )
        .await
        .unwrap();
    assert!(read_client.read_card_ids(&source, &shadow).await.is_err());
    assert!(
        stale_source_client
            .switch_to_shadow(&source, &shadow)
            .await
            .is_err()
    );
    assert_eq!(stale_source_client.target, ShadowReadTarget::Source);
    let mut refreshed_fence = read_fence.clone();
    refreshed_fence.rules_revision = shadow
        .state
        .semantic_rules()
        .current_rules_revision(&shadow.context)
        .await
        .unwrap();
    drop(shadow.core);
    drop(shadow.state);
    let reopened =
        vault_fixture_with_id(&shadow_db, &shadow_root, "shadow", shadow.context.id()).await;
    assert_eq!(reopened.context.id(), shadow.context.id());
    assert!(
        SemanticMemoryService::new(reopened.state.clone())
            .list_cards(&reopened.context, &reopened.core, 20)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reopened
            .core
            .read_managed(&reopened.context, &canonical_path)
            .await
            .is_ok()
    );
    assert_eq!(
        std::fs::read(source_root.join("notes/shadow.md")).unwrap(),
        source_before
    );
    let source_cards_after = source_memory
        .list_cards(&source.context, &source.core, 20)
        .await
        .unwrap();
    assert_eq!(
        source_cards_before
            .iter()
            .map(|card| (&card.id, card.revision_number))
            .collect::<Vec<_>>(),
        source_cards_after
            .iter()
            .map(|card| (&card.id, card.revision_number))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        source
            .core
            .read(&source.context, &path)
            .await
            .unwrap()
            .file
            .current_revision,
        source_file_before.current_revision
    );
    assert_eq!(count_files(&source_history_root), source_history_before);

    assert!(read_client.read_card_ids(&source, &reopened).await.is_err());
    let mut refreshed_client = ShadowReadClient::for_source(refreshed_fence);
    refreshed_client
        .switch_to_shadow(&source, &reopened)
        .await
        .unwrap();
    assert!(
        refreshed_client
            .read_card_ids(&source, &reopened)
            .await
            .unwrap()
            .is_empty()
    );
    let default_source_client = ShadowReadClient::for_source(read_fence.clone());
    assert!(
        default_source_client
            .read_card_ids(&source, &reopened)
            .await
            .unwrap()
            .contains(&source_card_id)
    );

    let source_drift_bytes = b"# Shadow\nThe source drift fixture changed.\n";
    source
        .core
        .replace_bytes(
            &source.context,
            &path,
            source_file_before.current_revision,
            source_drift_bytes,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    assert!(read_client.read_card_ids(&source, &reopened).await.is_err());
    assert!(
        default_source_client
            .read_card_ids(&source, &reopened)
            .await
            .is_err()
    );
}
