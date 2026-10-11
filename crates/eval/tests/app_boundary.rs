use mcp_vault_core::VaultCore;
use mcp_vault_domain::{
    Actor, Permission, PermissionSet, Revision, SemanticSourceId, SourcePlane, VaultContext,
    VaultId, VaultPath, VaultPathPolicy, VaultSlug, WritePrecondition,
};
use mcp_vault_eval::{
    ArmSemanticRuntime, ComparisonArm, EvalSource, EvalTask, SemanticArmRoot, SemanticArmRoots,
    SemanticMemoryAppBoundary, SemanticMemoryServiceAppBoundary, TaskSplit,
};
use mcp_vault_indexer::IndexService;
use mcp_vault_memory::{
    MemoryPackRequest, MemoryPackSourceScope, SemanticAccess, SemanticMemoryService,
    SemanticPublicFacade, semantic::organize::SemanticOrganizationService,
};
use mcp_vault_state::{StateStore, VaultStatus};
use mcp_vault_storage_fs::{DurabilityPolicy, StorageOptions};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

struct Fixture {
    state: StateStore,
    context: VaultContext,
    core: VaultCore,
    source_root: PathBuf,
    state_db_path: PathBuf,
    history_root: PathBuf,
}

async fn fixture(root: &std::path::Path, name: &str) -> Fixture {
    let source_root = root.join(name).join("vault");
    let state_db_path = root.join(name).join("state.sqlite3");
    let history_root = root.join(name).join("history");
    let state = StateStore::connect_and_migrate(&format!("sqlite://{}", state_db_path.display()))
        .await
        .unwrap();
    let context = VaultContext::new(
        VaultId::new(),
        VaultSlug::new(name).unwrap(),
        source_root.clone(),
        Revision::ZERO,
    )
    .unwrap();
    state
        .vaults()
        .insert(&context, name, VaultStatus::Active)
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
    let core = VaultCore::new(
        state.clone(),
        history_root.clone(),
        VaultPathPolicy::default(),
        StorageOptions {
            durability: DurabilityPolicy::None,
            minimum_free_bytes: 0,
            ..StorageOptions::default()
        },
        Default::default(),
    );
    Fixture {
        state,
        context,
        core,
        source_root,
        state_db_path,
        history_root,
    }
}

fn source(logical_id: &str, path: &str, body: &str) -> EvalSource {
    EvalSource {
        logical_id: logical_id.into(),
        synthetic_placeholder: false,
        vault_id: "baseline-vault".into(),
        file_id: format!("baseline-file-{logical_id}"),
        path: path.into(),
        file_revision: 1,
        content_hash: format!("{:x}", Sha256::digest(body.as_bytes())),
        logical_block_count: 0,
        source_revision_id: format!("baseline-revision-{logical_id}"),
        authorization_revision: 1,
        profile_id: "semantic-profile-v1".into(),
        rules_revision: 1,
        source_generation: 1,
        extraction_commit_sequence: 1,
    }
}

async fn publish(fixture: &Fixture, path: &str, statement: &str) {
    let path = VaultPath::parse(path).unwrap();
    fixture
        .core
        .create_bytes(
            &fixture.context,
            &path,
            format!("# Decision\n{statement}\n").as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let service = SemanticMemoryService::new(fixture.state.clone());
    let input = service
        .prepare_source(&fixture.context, &fixture.core, &path)
        .await
        .unwrap();
    let body = input.blocks.last().unwrap();
    let proposal = json!({
        "outcome":"success_nonempty",
        "observations":[{
            "kind":"decision",
            "statement":statement,
            "scope":"project",
            "assertion_status":"source_asserted",
            "conditions":["approval is required"],
            "exceptions":["emergency changes"],
            "ordered_steps":["review", "apply"],
            "admission_reason":"isolated eval fixture",
            "value_for_future_work":"keep the boundary",
            "body_block_ids":[body.local_id.clone()]
        }],
        "cards":[{"title":"Isolated decision", "kind":"decision", "scope":"project", "assertion_status":"source_asserted", "observation_indices":[0]}]
    });
    service
        .submit_proposal_json(
            &fixture.context,
            &fixture.core,
            &path,
            &proposal.to_string(),
        )
        .await
        .unwrap();
}

fn task() -> EvalTask {
    EvalTask {
        id: "Q-isolated".into(),
        split: TaskSplit::Holdout,
        query_refs: vec!["S-B".into(), "S-C".into()],
        query: "rollback boundary".into(),
        source_ids: vec!["S-B".into(), "S-C".into()],
        source_fence: Vec::new(),
        must_preserve: Vec::new(),
        must_not_infer: Vec::new(),
        expected_usable_information: vec!["rollback boundary".into()],
        expected_no_answer: false,
        expected_status: "supported".into(),
        expected_source_relations: Vec::new(),
        severity: "normal".into(),
    }
}

fn budget() -> mcp_vault_eval::Budget {
    mcp_vault_eval::Budget {
        max_entries: 20,
        max_bytes: 32_000,
        max_tokens: 8_000,
        external_request_budget: 20,
    }
}

fn runtime(fixture: &Fixture) -> ArmSemanticRuntime {
    ArmSemanticRuntime {
        state: fixture.state.clone(),
        context: fixture.context.clone(),
        core: fixture.core.clone(),
        source_root: fixture.source_root.clone(),
        state_db_path: fixture.state_db_path.clone(),
        history_root: fixture.history_root.clone(),
    }
}

#[tokio::test]
async fn production_adapter_maps_real_ids_and_keeps_b_c_state_and_cards_disjoint() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let b_text = "Keep the B rollback boundary.";
    let c_text = "Keep the C rollback boundary.";
    let outside_text = "Keep the outside rollback boundary.";
    publish(&b, "notes/b.md", b_text).await;
    publish(&b, "notes/outside.md", outside_text).await;
    publish(&c, "notes/c.md", c_text).await;
    publish(&c, "notes/outside.md", outside_text).await;

    let adapter = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.state_db_path.clone(),
        baseline.history_root.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let source_b = source("S-B", "notes/b.md", &format!("# Decision\n{b_text}\n"));
    let source_c = source("S-C", "notes/c.md", &format!("# Decision\n{c_text}\n"));
    let task = task();
    let b_pack = adapter
        .build_pack_for_arm(
            ComparisonArm::B,
            &task,
            std::slice::from_ref(&source_b),
            &budget(),
        )
        .await
        .unwrap();
    let c_pack = adapter
        .build_pack_for_arm(
            ComparisonArm::C,
            &task,
            std::slice::from_ref(&source_c),
            &budget(),
        )
        .await
        .unwrap();
    assert_eq!(b_pack["current_context"].as_array().unwrap().len(), 1);
    assert_eq!(c_pack["current_context"].as_array().unwrap().len(), 1);
    let b_entry = &b_pack["current_context"][0];
    let c_entry = &c_pack["current_context"][0];
    for entry in [b_entry, c_entry] {
        let refs = entry["evidence_refs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<std::collections::BTreeSet<_>>();
        let excerpts = entry["evidence_excerpts"].as_array().unwrap();
        assert!(
            entry["evidence_refs"].as_array().unwrap().len() > refs.len(),
            "fixture exercises shared support references"
        );
        assert_eq!(
            excerpts.len(),
            refs.len(),
            "shared references must not duplicate source text"
        );
        assert!(
            excerpts
                .iter()
                .all(|excerpt| !excerpt["spans"].as_array().unwrap().is_empty())
        );
    }
    assert!(
        b_entry["core_assertions"][0]
            .as_str()
            .unwrap()
            .contains("B rollback")
    );
    assert!(
        c_entry["core_assertions"][0]
            .as_str()
            .unwrap()
            .contains("C rollback")
    );
    assert!(!b_pack.to_string().contains("outside rollback"));
    assert!(!c_pack.to_string().contains("outside rollback"));
    assert!(!b_pack.to_string().contains("C rollback"));
    assert!(!c_pack.to_string().contains("B rollback"));
    let b_source_id = b_entry["source_references"][0]["source_id"]
        .as_str()
        .unwrap();
    assert_ne!(
        b_source_id, "S-B",
        "scope must use State's SemanticSourceId"
    );
    assert!(SemanticSourceId::parse(b_source_id).is_ok());

    let b_cards = SemanticPublicFacade::new(b.state.clone())
        .list_cards(
            &b.context,
            &b.core,
            &SemanticAccess::new(PermissionSet::from_iter([
                Permission::ReadMemory,
                Permission::ReadVault,
            ])),
            20,
        )
        .await
        .unwrap()
        .0;
    let c_cards = SemanticPublicFacade::new(c.state.clone())
        .list_cards(
            &c.context,
            &c.core,
            &SemanticAccess::new(PermissionSet::from_iter([
                Permission::ReadMemory,
                Permission::ReadVault,
            ])),
            20,
        )
        .await
        .unwrap()
        .0;
    assert_eq!(b_cards.len(), 2);
    assert_eq!(c_cards.len(), 2);
    assert!(
        b_cards
            .iter()
            .all(|card| !card.items[0].content.contains("C rollback"))
    );
    assert!(
        c_cards
            .iter()
            .all(|card| !card.items[0].content.contains("B rollback"))
    );
}

#[tokio::test]
async fn relation_adapter_publishes_m2_cards_before_the_c_pack_is_built() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let statement = "The project requires a reviewed deployment and a verified rollback path.";
    let first_path = "notes/decision-a.md";
    let second_path = "notes/decision-b.md";
    publish(&c, first_path, statement).await;
    publish(&c, second_path, statement).await;

    let adapter = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.state_db_path.clone(),
        baseline.history_root.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let body = format!("# Decision\n{statement}\n");
    let first = source("S-left", first_path, &body);
    let second = source("S-right", second_path, &body);
    let mut task = task();
    task.id = "Q-m2-relation".into();
    task.query_refs = vec!["Q-m2-relation".into()];
    task.source_ids = vec![first.logical_id.clone(), second.logical_id.clone()];
    task.query = "What deployment and rollback requirements are supported by both sources?".into();

    let prepared = adapter
        .prepare_relation_input_for_arm(ComparisonArm::C, &task, &[first.clone(), second.clone()])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prepared["dispatch_required"], true);
    assert_eq!(prepared["candidate_count"], 1);
    let candidate_alias = prepared["candidates"][0]["candidate_id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(candidate_alias, "C01");

    let first_file = c
        .state
        .files()
        .get_active(&c.context, &VaultPath::parse(first_path).unwrap())
        .await
        .unwrap()
        .unwrap();
    let second_file = c
        .state
        .files()
        .get_active(&c.context, &VaultPath::parse(second_path).unwrap())
        .await
        .unwrap()
        .unwrap();
    let source_ids = vec![
        c.state
            .semantic_memory()
            .get_source_by_file(&c.context, first_file.id)
            .await
            .unwrap()
            .unwrap()
            .source_id,
        c.state
            .semantic_memory()
            .get_source_by_file(&c.context, second_file.id)
            .await
            .unwrap()
            .unwrap()
            .source_id,
    ];
    let stored_candidates = SemanticOrganizationService::new(c.state.clone())
        .discover_candidates(&c.context, &source_ids)
        .await
        .unwrap();
    assert_eq!(stored_candidates.len(), 1);
    assert_ne!(candidate_alias, stored_candidates[0].id.to_string());

    let proposal = json!({
        "actions": [{
            "action": "create_composed_card",
            "candidate_ids": [candidate_alias],
            "card_ref": "deployment-rollback",
            "title": "Reviewed deployment with verified rollback",
            "support_operator": "or",
            "reason": "Both current sources independently support the complete requirement."
        }]
    });
    let applied = adapter
        .submit_relation_proposal_for_arm(
            ComparisonArm::C,
            &task,
            &[first.clone(), second.clone()],
            &proposal,
        )
        .await
        .unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(applied["candidate_count"], 1);

    let composed = SemanticOrganizationService::new(c.state.clone())
        .list_composed_cards(&c.context, &c.core, 20)
        .await
        .unwrap();
    assert_eq!(composed.len(), 1);
    assert!(composed[0].title.contains("verified rollback"));

    let pack = adapter
        .build_pack_for_arm(ComparisonArm::C, &task, &[first, second], &budget())
        .await
        .unwrap();
    assert_eq!(pack["current_context"].as_array().unwrap().len(), 1);
    assert!(pack.to_string().contains("source_references"));

    let first_revision = c
        .state
        .semantic_memory()
        .get_source_by_file(&c.context, first_file.id)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
        .unwrap();
    let second_revision = c
        .state
        .semantic_memory()
        .get_source_by_file(&c.context, second_file.id)
        .await
        .unwrap()
        .unwrap()
        .current_revision_id
        .unwrap();
    let m2_only = SemanticPublicFacade::new(c.state.clone())
        .build_pack(
            &c.context,
            &c.core,
            &SemanticAccess::new(PermissionSet::from_iter([
                Permission::ReadMemory,
                Permission::ReadVault,
            ])),
            &MemoryPackRequest {
                task: task.query.clone(),
                query: Some(task.query.clone()),
                comparison_source_paths: vec![first_path.into(), second_path.into()],
                source_scope: Some(MemoryPackSourceScope {
                    source_ids: source_ids.iter().map(ToString::to_string).collect(),
                    source_paths: vec![first_path.into(), second_path.into()],
                    source_revision_ids: vec![
                        first_revision.to_string(),
                        second_revision.to_string(),
                    ],
                }),
                include_m1: false,
                include_m2: true,
                ..MemoryPackRequest::default()
            },
        )
        .await
        .unwrap();
    assert!(
        m2_only
            .current_context
            .iter()
            .chain(m2_only.relevant_experiences.iter())
            .any(|entry| entry
                .title
                .contains("Reviewed deployment with verified rollback"))
    );
}

#[tokio::test]
async fn ordinary_lexical_adapter_fails_closed_on_a_stale_index_projection() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let path = VaultPath::parse("notes/lexical.md").unwrap();
    let initial_body = b"# Rollback\n\nThe rollback condition is oldneedle.\n";
    let created = baseline
        .core
        .create_bytes(
            &baseline.context,
            &path,
            initial_body,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    let index = IndexService::new(baseline.state.clone());
    index
        .rebuild_vault(&baseline.core, &baseline.context)
        .await
        .unwrap();
    let mut lexical_source = source(
        "S-A",
        path.as_str(),
        std::str::from_utf8(initial_body).unwrap(),
    );
    lexical_source.vault_id = baseline.context.id().to_string();
    lexical_source.file_id = created.file.id.to_string();
    lexical_source.file_revision = created.file.current_revision.value();
    lexical_source.content_hash = created.file.content_hash.clone().unwrap();

    let adapter = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.state_db_path.clone(),
        baseline.history_root.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let mut lexical_task = task();
    lexical_task.id = "Q-A".into();
    lexical_task.query_refs = vec!["Q-A-support".into()];
    lexical_task.query = "oldneedle".into();
    lexical_task.source_ids = vec!["S-A".into()];
    let current = adapter
        .ordinary_retrieve_with_profile(
            &lexical_task,
            std::slice::from_ref(&lexical_source),
            &budget(),
            "index-frozen-v1",
        )
        .await
        .unwrap();
    assert_eq!(current["coverage"]["complete"], true);
    assert_eq!(current["coverage"]["eligible_count"], 1);

    // Natural questions are not strict conjunctions of every question word.
    // The versioned profile uses the existing lexical recall admission policy.
    lexical_task.query = "What is the rollback condition?".into();
    for (profile, expected_hits) in [
        ("index-frozen-v1", 0),
        ("index-lexical-recall-v2", 1),
        (mcp_vault_eval::M6_A80_INDEX_PROFILE_ID, 1),
    ] {
        let result = adapter
            .ordinary_retrieve_with_profile(
                &lexical_task,
                std::slice::from_ref(&lexical_source),
                &budget(),
                profile,
            )
            .await
            .unwrap();
        assert_eq!(result["sources"].as_array().unwrap().len(), expected_hits);
        assert!(result["degradation_reasons"].as_array().unwrap().is_empty());
    }
    lexical_task.query = "What is the unrelated unicorn deadline?".into();
    let unrelated = adapter
        .ordinary_retrieve_with_profile(
            &lexical_task,
            std::slice::from_ref(&lexical_source),
            &budget(),
            mcp_vault_eval::M6_A80_INDEX_PROFILE_ID,
        )
        .await
        .unwrap();
    assert!(unrelated["sources"].as_array().unwrap().is_empty());
    lexical_task.query = "oldneedle".into();

    let replacement_body = b"# Rollback\n\nThe rollback condition is newneedle.\n";
    let replaced = baseline
        .core
        .replace_bytes(
            &baseline.context,
            &path,
            created.file.current_revision,
            replacement_body,
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap();
    lexical_source.file_revision = replaced.file.current_revision.value();
    lexical_source.content_hash = replaced.file.content_hash.clone().unwrap();
    let stale = adapter
        .ordinary_retrieve_with_profile(
            &lexical_task,
            std::slice::from_ref(&lexical_source),
            &budget(),
            "index-frozen-v1",
        )
        .await
        .unwrap();
    assert_eq!(stale["coverage"]["complete"], false);
    assert_eq!(stale["coverage"]["current_indexed_source_count"], 0);
    assert!(stale["sources"].as_array().unwrap().is_empty());
    assert!(
        stale["degradation_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason == "ordinary_index_coverage_incomplete")
    );
    assert!(
        stale["quality_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event == "lexical_no_match")
    );
}

#[tokio::test]
async fn ordinary_complete_evidence_is_canonical_and_bound_to_frozen_source() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let body = format!(
        "# Rollback\n\nStatus: historical design.\n\nThe rollback policy {} No automatic deletion is permitted; 历史证据保留。\n",
        "requires review of the complete source. ".repeat(18)
    );
    let path = VaultPath::parse("notes/policy.md").unwrap();
    let file = baseline
        .core
        .create_bytes(
            &baseline.context,
            &path,
            body.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    IndexService::new(baseline.state.clone())
        .rebuild_vault(&baseline.core, &baseline.context)
        .await
        .unwrap();
    let mut source = source("S-policy", path.as_str(), &body);
    source.vault_id = baseline.context.id().to_string();
    source.file_id = file.id.to_string();
    source.file_revision = file.current_revision.value();
    let adapter = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.state_db_path.clone(),
        baseline.history_root.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let mut task = task();
    task.query = "What is the rollback policy?".into();
    let current = adapter
        .ordinary_retrieve_with_profile(
            &task,
            &[source.clone()],
            &budget(),
            mcp_vault_eval::M6_A80_INDEX_PROFILE_ID,
        )
        .await
        .unwrap();
    let evidence = &current["sources"][0]["evidence"];
    assert_eq!(evidence["kind"], "complete_document");
    assert_eq!(evidence["spans"][0]["text"], body);
    assert_eq!(evidence["spans"][0]["end_byte"], body.len());
    assert_eq!(evidence["source_content_hash"], source.content_hash);
    assert_eq!(
        current["token_count"].as_u64().unwrap(),
        serde_json::to_vec(&current).unwrap().len().div_ceil(4) as u64
    );
    let legacy = adapter
        .ordinary_retrieve_with_profile(
            &task,
            &[source.clone()],
            &budget(),
            "index-lexical-recall-v2",
        )
        .await
        .unwrap();
    assert!(legacy["sources"][0].get("evidence").is_none());
    assert!(
        !legacy["sources"][0]["snippet"]
            .as_str()
            .unwrap()
            .contains("No automatic deletion")
    );
    let mut wrong_hash = source.clone();
    wrong_hash.content_hash = "0".repeat(64);
    assert_eq!(
        adapter
            .ordinary_retrieve_with_profile(
                &task,
                &[wrong_hash],
                &budget(),
                mcp_vault_eval::M6_A80_INDEX_PROFILE_ID
            )
            .await
            .unwrap_err(),
        "ordinary_evidence_source_mismatch"
    );
    source.vault_id = b.context.id().to_string();
    assert_eq!(
        adapter
            .ordinary_retrieve_with_profile(
                &task,
                &[source],
                &budget(),
                mcp_vault_eval::M6_A80_INDEX_PROFILE_ID
            )
            .await
            .unwrap_err(),
        "ordinary_evidence_source_mismatch"
    );
}

#[tokio::test]
async fn ordinary_budget_keeps_whole_section_and_historical_preamble_or_reports_gap() {
    let temp = tempfile::tempdir().unwrap();
    let baseline = fixture(temp.path(), "baseline").await;
    let b = fixture(temp.path(), "arm-b").await;
    let c = fixture(temp.path(), "arm-c").await;
    let section = format!(
        "## Rules\n\nThe rollback policy {} Recovery requires exact source evidence, never a filename match.\n\n### Exception\n\nEmergency approval remains necessary.\n\n",
        "retains historical records. ".repeat(20)
    );
    let preamble = "# Archive\n\nStatus: superseded design, not current policy.\n\n";
    let body = format!(
        "{preamble}{section}## Log\n\n{}",
        "Unrelated maintenance history. ".repeat(2000)
    );
    let path = VaultPath::parse("notes/archive.md").unwrap();
    let file = baseline
        .core
        .create_bytes(
            &baseline.context,
            &path,
            body.as_bytes(),
            Actor::system(),
            SourcePlane::System,
            None,
        )
        .await
        .unwrap()
        .file;
    IndexService::new(baseline.state.clone())
        .rebuild_vault(&baseline.core, &baseline.context)
        .await
        .unwrap();
    let mut source = source("S-archive", path.as_str(), &body);
    source.vault_id = baseline.context.id().to_string();
    source.file_id = file.id.to_string();
    source.file_revision = file.current_revision.value();
    let adapter = SemanticMemoryServiceAppBoundary::new_isolated(
        baseline.state.clone(),
        baseline.context.clone(),
        baseline.state_db_path.clone(),
        baseline.history_root.clone(),
        runtime(&b),
        runtime(&c),
    )
    .unwrap();
    let mut task = task();
    task.query = "What is the rollback policy?".into();
    let mut budget = budget();
    budget.max_bytes = 4000;
    budget.max_tokens = 1000;
    let result = adapter
        .ordinary_retrieve_with_profile(
            &task,
            &[source.clone()],
            &budget,
            mcp_vault_eval::M6_A80_INDEX_PROFILE_ID,
        )
        .await
        .unwrap();
    let evidence = &result["sources"][0]["evidence"];
    assert_eq!(evidence["kind"], "complete_section");
    assert_eq!(evidence["spans"][0]["text"], preamble);
    assert_eq!(evidence["spans"][1]["text"], section);
    let bytes = serde_json::to_vec(&result).unwrap().len();
    assert!(bytes <= budget.max_bytes as usize);
    assert_eq!(result["token_count"], bytes.div_ceil(4));
    budget.max_bytes = 1200;
    budget.max_tokens = 300;
    let gap = adapter
        .ordinary_retrieve_with_profile(
            &task,
            &[source],
            &budget,
            mcp_vault_eval::M6_A80_INDEX_PROFILE_ID,
        )
        .await
        .unwrap();
    assert!(gap["sources"].as_array().unwrap().is_empty());
    assert!(
        gap["degradation_reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("ordinary_complete_evidence_budget_exhausted"))
    );
    assert!(serde_json::to_vec(&gap).unwrap().len() <= 1200);
}

#[test]
fn arm_root_contract_names_separate_vaults_and_paths() {
    let make_root = |prefix: &str| SemanticArmRoot {
        vault_slug: format!("{prefix}-slug"),
        vault_id: format!("{prefix}-vault"),
        source_root: format!("/tmp/{prefix}/source"),
        state_root: format!("/tmp/{prefix}/state"),
        history_root: format!("/tmp/{prefix}/history"),
    };
    let roots = SemanticArmRoots {
        b: make_root("b"),
        c: make_root("c"),
    };
    assert_ne!(roots.b.vault_id, roots.c.vault_id);
    assert_ne!(roots.b.state_root, roots.c.state_root);
}
