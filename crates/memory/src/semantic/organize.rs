//! Deterministic, bounded M2 organization service.
//!
//! Candidate discovery is intentionally lexical and conservative. Durable
//! relations and composed cards are accepted only after strict validation and
//! are committed through the State/Core snapshot boundary.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use mcp_vault_core::{VaultCore, VaultError};
use mcp_vault_domain::{
    Actor, ComposedCardId, ComposedCardItemId, ComposedCardRevisionId, RelationCandidateId,
    RelationDecisionId, SemanticSourceId, SourcePlane, SupportGroupId, SupportMemberId,
    VaultContext, VaultPath,
};
use mcp_vault_state::{
    ComposedCardItemInput, ComposedCardRevisionInput, OrganizationActionAuditInput,
    OrganizationObservation, OrganizationSourceFence, RelationCandidateInput,
    RelationDecisionInput, RelationEvidenceInput, StateStore, SupportGroupInput,
    SupportMemberInput,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::MemoryError;

#[derive(Clone, Debug, PartialEq)]
pub struct OrganizationCandidate {
    pub id: RelationCandidateId,
    pub left: OrganizationObservation,
    pub right: OrganizationObservation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationProposal {
    pub actions: Vec<OrganizationAction>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationAction {
    pub action: OrganizationActionKind,
    pub candidate_ids: Vec<String>,
    #[serde(default)]
    pub card_ref: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub item_kind: Option<String>,
    #[serde(default)]
    pub support_operator: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationActionKind {
    CreateComposedCard,
    AttachEquivalentEvidence,
    AddSupportedInformation,
    SupersedeWithEvidence,
    RecordConflict,
    LinkRelatedOnly,
    KeepSeparateScope,
    NoChange,
}

#[derive(Clone)]
pub struct SemanticOrganizationService {
    state: StateStore,
}

impl SemanticOrganizationService {
    pub fn new(state: StateStore) -> Self {
        Self { state }
    }

    /// Persist cancellation before an organization job prepares a canonical
    /// composed-card snapshot.
    pub async fn cancel_organization(
        &self,
        context: &VaultContext,
        job_id: mcp_vault_domain::OrganizationJobId,
    ) -> Result<(), MemoryError> {
        self.state
            .semantic_organization()
            .cancel_job(context, job_id, "semantic_organization_cancelled")
            .await?;
        Ok(())
    }

    pub async fn discover_candidates(
        &self,
        context: &VaultContext,
        source_ids: &[SemanticSourceId],
    ) -> Result<Vec<OrganizationCandidate>, MemoryError> {
        let repository = self.state.semantic_organization();
        let observations = repository.current_observations(context, source_ids).await?;
        Ok(build_candidates(observations))
    }

    /// List composed cards only after verifying their canonical managed file
    /// through Vault Core. State qualification alone is not a current-read
    /// proof for a task pack.
    pub async fn list_composed_cards(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        limit: u32,
    ) -> Result<Vec<mcp_vault_state::ComposedCardRecord>, MemoryError> {
        let cards = self
            .state
            .semantic_organization()
            .list_composed_cards(context, limit)
            .await?;
        let mut readable = Vec::new();
        for card in cards {
            let mut read = match core.read_managed(context, &card.canonical_path).await {
                Ok(read) => read,
                Err(
                    VaultError::NotFound | VaultError::ExternalMismatch | VaultError::NeedsReview,
                ) => {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let mut bytes = Vec::new();
            read.reader
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| MemoryError::SourceIngestion("semantic_composed_card_read_failed"))?;
            let Some(file_id) = card.canonical_file_id else {
                continue;
            };
            let Some(file_revision) = card.canonical_file_revision else {
                continue;
            };
            if read.file.id == file_id
                && read.file.current_revision == file_revision
                && read.file.content_hash.as_deref() == Some(card.canonical_markdown_hash.as_str())
                && hash_bytes(&bytes) == card.canonical_markdown_hash
            {
                readable.push(card);
            }
        }
        Ok(readable)
    }

    pub async fn get_composed_card(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        card_id: mcp_vault_domain::ComposedCardId,
    ) -> Result<Option<mcp_vault_state::ComposedCardRecord>, MemoryError> {
        let Some(card) = self
            .state
            .semantic_organization()
            .get_composed_card(context, card_id)
            .await?
        else {
            return Ok(None);
        };
        let readable = self
            .list_composed_cards(context, core, 200)
            .await?
            .into_iter()
            .any(|current| current.id == card.id && current.revision_id == card.revision_id);
        Ok(readable.then_some(card))
    }

    /// Validate a fixed proposal and publish a composed card through the
    /// existing Core witness/CAS seam. No provider or model is called here.
    pub async fn organize_json(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        source_ids: &[SemanticSourceId],
        proposal_json: &str,
    ) -> Result<(), MemoryError> {
        let repository = self.state.semantic_organization();
        let observations = repository.current_observations(context, source_ids).await?;
        let candidates = build_candidates(observations);
        if candidates.is_empty() {
            return Err(MemoryError::GeneratedOutput(
                "semantic_no_organization_candidates",
            ));
        }
        let proposal: OrganizationProposal = serde_json::from_str(proposal_json)
            .map_err(|_| MemoryError::GeneratedOutput("semantic_organization_proposal_schema"))?;
        let fences = repository
            .current_source_fences(context, source_ids)
            .await?;
        let source_material = fences
            .iter()
            .map(|fence| {
                format!(
                    "{}:{}:{}:{}:{}:{}",
                    fence.source_id,
                    fence.source_revision_id,
                    fence.content_hash,
                    fence.authorization_revision,
                    fence.source_generation,
                    fence.extraction_commit_sequence
                )
            })
            .collect::<Vec<_>>();
        let observation_material = repository
            .current_observations(context, source_ids)
            .await?
            .into_iter()
            .map(|observation| {
                [
                    observation.id.to_string(),
                    observation.source_id.to_string(),
                    observation.source_revision_id.to_string(),
                    observation.kind,
                    observation.statement,
                    observation.scope,
                    observation.assertion_status,
                    format!("{:?}", observation.conditions),
                    format!("{:?}", observation.exceptions),
                    format!("{:?}", observation.ordered_steps),
                    format!("{:?}", observation.result),
                    format!("{:?}", observation.uncertainty),
                    observation.source_time_scope.to_string(),
                    observation.value_for_future_work,
                ]
                .join("|")
            })
            .collect::<Vec<_>>();
        let mut input_material = source_material;
        input_material.extend(observation_material);
        input_material.sort();
        input_material.push("semantic-memory-m2-v1:proposal-schema-1".to_owned());
        input_material.push(proposal_json.to_owned());
        let input_hash = hash_bytes(input_material.join("|").as_bytes());
        let request_hash = hash_bytes(format!("m2:{input_hash}").as_bytes());
        let (job, created) = repository
            .start_job(
                context,
                mcp_vault_domain::OrganizationJobId::new(),
                &input_hash,
                0,
                "semantic-memory-m2-v1",
                &format!("semantic-organization:{request_hash}"),
                &request_hash,
                &fences,
            )
            .await?;
        if !created {
            return if job.lifecycle_state == "applied" {
                Ok(())
            } else {
                // A replay of an in-flight job is not a second publication and
                // must not masquerade as a successful organization request.
                Err(MemoryError::Conflict)
            };
        }
        // Discovery IDs are stable proposal references. Durable candidate
        // rows are job-scoped so a failed job can remain auditable without
        // colliding with a later retry of the same source pair.
        let scoped_candidates = candidates
            .iter()
            .map(|candidate| {
                let mut scoped = candidate.clone();
                scoped.id = job_candidate_id(job.id, candidate.id);
                scoped
            })
            .collect::<Vec<_>>();
        let candidate_map = candidates
            .iter()
            .zip(scoped_candidates.iter())
            .map(|(discovered, scoped)| (discovered.id.to_string(), scoped.clone()))
            .collect::<HashMap<_, _>>();
        for candidate in &scoped_candidates {
            if let Err(error) = repository
                .record_candidate(context, job.id, &candidate_input(candidate))
                .await
            {
                let _ = repository
                    .mark_job_failed(context, job.id, "semantic_candidate_failed", false)
                    .await;
                return Err(error.into());
            }
        }
        let prepared = match build_state_proposal(
            context,
            job.id,
            core.managed_root(),
            &proposal,
            &candidate_map,
            &fences,
        ) {
            Ok(value) => value,
            Err(error) => {
                let _ = repository
                    .mark_job_failed(context, job.id, error.code(), false)
                    .await;
                return Err(error);
            }
        };
        let mut prepared = prepared;
        for card in &mut prepared.1 {
            if let Some((existing_id, existing_revision, existing_number)) = repository
                .current_composed_card_identity(context, &card.identity_key)
                .await?
            {
                card.card_id = existing_id;
                card.expected_card_revision_id = Some(existing_revision);
                card.revision_number = existing_number
                    .checked_add(1)
                    .ok_or(MemoryError::Conflict)?;
                let child = VaultPath::parse(&format!("semantic-memory/composed/{existing_id}.md"))
                    .map_err(|_| {
                        MemoryError::InvalidInput("semantic composed card path is invalid")
                    })?;
                card.canonical_path = core.managed_root().join(&child).map_err(|_| {
                    MemoryError::InvalidInput("semantic composed card path is invalid")
                })?;
            }
        }
        let audits = proposal
            .actions
            .iter()
            .enumerate()
            .map(|(ordinal, action)| {
                Ok::<_, MemoryError>(OrganizationActionAuditInput {
                    ordinal: u32::try_from(ordinal).unwrap_or(u32::MAX),
                    action_kind: action_kind_name(&action.action).to_owned(),
                    candidate_ids: action
                        .candidate_ids
                        .iter()
                        .map(|value| {
                            candidate_map
                                .get(value)
                                .map(|candidate| candidate.id)
                                .ok_or_else(|| {
                                    MemoryError::GeneratedOutput(
                                        "semantic_organization_candidate_invalid",
                                    )
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| {
                            MemoryError::GeneratedOutput("semantic_organization_candidate_invalid")
                        })?,
                    reason: action.reason.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>();
        let audits = match audits {
            Ok(value) => value,
            Err(error) => {
                let _ = repository
                    .mark_job_failed(context, job.id, error.code(), false)
                    .await;
                return Err(error);
            }
        };
        if let Err(error) = repository
            .record_action_audits(context, job.id, &audits)
            .await
        {
            let _ = repository
                .mark_job_failed(context, job.id, "semantic_action_audit_failed", false)
                .await;
            return Err(error.into());
        }
        let snapshots = match repository
            .prepare_organization(context, job.id, &prepared.0, &prepared.1)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                let _ = repository
                    .mark_job_failed(context, job.id, "semantic_prepare_failed", false)
                    .await;
                return Err(error.into());
            }
        };
        let job_rules = repository
            .get_job(context, job.id)
            .await?
            .ok_or(MemoryError::Conflict)?
            .rules_revision;
        if job_rules
            != self
                .state
                .semantic_rules()
                .current_rules_revision(context)
                .await?
        {
            let _ = repository
                .mark_job_failed(context, job.id, "semantic_rules_changed", true)
                .await;
            return Err(MemoryError::Conflict);
        }
        for snapshot in snapshots {
            if let Err(error) = repository.validate_job_write_fence(context, job.id).await {
                let _ = repository
                    .mark_job_failed(context, job.id, "semantic_source_fence_conflict", true)
                    .await;
                return Err(MemoryError::State(error));
            }
            let file = match ensure_composed_file(context, core, &snapshot).await {
                Ok(value) => value,
                Err(error) => {
                    // Core may have persisted a prepared/file-committed
                    // journal before returning an error. Keep this job and
                    // its snapshot discoverable as a semantic barrier even
                    // when the failure is otherwise terminal.
                    let _ = repository
                        .mark_job_failed(context, job.id, error.code(), true)
                        .await;
                    return Err(error);
                }
            };
            if let Err(error) = repository
                .mark_snapshot_written(
                    context,
                    snapshot.id,
                    file.id,
                    file.current_revision,
                    &snapshot.proposed_file_hash,
                )
                .await
            {
                let _ = repository
                    .mark_job_failed(context, job.id, "semantic_snapshot_witness_failed", true)
                    .await;
                return Err(error.into());
            }
        }
        if let Err(error) = repository.apply_organization(context, job.id).await {
            let _ = repository
                .mark_job_failed(context, job.id, "semantic_apply_failed", true)
                .await;
            return Err(error.into());
        }
        Ok(())
    }

    pub async fn recover_organization(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        job_id: mcp_vault_domain::OrganizationJobId,
    ) -> Result<(), MemoryError> {
        super::preflight_all_semantic_recovery(&self.state, context).await?;
        self.recover_organization_inner(context, core, job_id, true, None)
            .await
    }

    /// Deterministic test seam for the final State apply boundary. The gate is
    /// deliberately hidden from normal callers; it lets recovery tests change
    /// a source fence after Core witness and before State CAS.
    #[doc(hidden)]
    pub async fn recover_organization_with_apply_gate(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        job_id: mcp_vault_domain::OrganizationJobId,
        reached: Arc<tokio::sync::Notify>,
        proceed: Arc<tokio::sync::Notify>,
    ) -> Result<(), MemoryError> {
        super::preflight_all_semantic_recovery(&self.state, context).await?;
        self.recover_organization_inner(context, core, job_id, true, Some((reached, proceed)))
            .await
    }

    async fn recover_organization_inner(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        job_id: mcp_vault_domain::OrganizationJobId,
        recover_core: bool,
        apply_gate: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
    ) -> Result<(), MemoryError> {
        let repository = self.state.semantic_organization();
        let job = repository
            .get_job(context, job_id)
            .await?
            .ok_or(MemoryError::NotFound)?;
        let current_rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        // Rules are a preflight fence: do not ask Core to recover journals for
        // a semantic job that can no longer be published under current rules.
        if job.rules_revision != current_rules_revision {
            let _ = repository
                .mark_job_failed(context, job_id, "semantic_rules_changed", true)
                .await;
            return Err(MemoryError::Conflict);
        }
        if let Err(error) = repository.validate_job_write_fence(context, job_id).await {
            let _ = repository
                .mark_job_failed(context, job_id, "semantic_source_fence_conflict", true)
                .await;
            return Err(MemoryError::State(error));
        }
        if recover_core && let Err(error) = core.recover(context).await {
            let _ = repository
                .mark_job_failed(context, job_id, "semantic_core_recovery_failed", true)
                .await;
            return Err(error.into());
        }
        let snapshots = repository.pending_snapshots(context, job_id).await?;
        if snapshots.is_empty() {
            if job.lifecycle_state == "applied" {
                return Ok(());
            }
            if job.lifecycle_state == "prepared" {
                return match repository.apply_organization(context, job_id).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        let _ = repository
                            .mark_job_failed(
                                context,
                                job_id,
                                "organization_recovery_without_snapshot",
                                true,
                            )
                            .await;
                        Err(MemoryError::State(error))
                    }
                };
            }
            // A crash before prepare leaves no durable proposal from which
            // recovery could safely reconstruct a card. Terminalize it
            // instead of repeatedly retrying an unrecoverable job.
            let _ = repository
                .mark_job_failed(
                    context,
                    job_id,
                    "organization_recovery_without_snapshot",
                    true,
                )
                .await;
            return Err(MemoryError::Conflict);
        }
        let job_rules = job.rules_revision;
        if job_rules
            != self
                .state
                .semantic_rules()
                .current_rules_revision(context)
                .await?
        {
            let _ = repository
                .mark_job_failed(context, job_id, "semantic_rules_changed", true)
                .await;
            return Err(MemoryError::Conflict);
        }
        for snapshot in snapshots {
            if let Err(error) = repository.validate_job_write_fence(context, job_id).await {
                let _ = repository
                    .mark_job_failed(context, job_id, "semantic_source_fence_conflict", true)
                    .await;
                return Err(MemoryError::State(error));
            }
            let file = match ensure_composed_file(context, core, &snapshot).await {
                Ok(value) => value,
                Err(error) => {
                    let _ = repository
                        .mark_job_failed(context, job_id, error.code(), true)
                        .await;
                    return Err(error);
                }
            };
            if let Err(error) = repository
                .mark_snapshot_written(
                    context,
                    snapshot.id,
                    file.id,
                    file.current_revision,
                    &snapshot.proposed_file_hash,
                )
                .await
            {
                let _ = repository
                    .mark_job_failed(context, job_id, "semantic_recovery_witness_failed", true)
                    .await;
                return Err(error.into());
            }
        }
        if let Some((reached, proceed)) = apply_gate {
            reached.notify_one();
            proceed.notified().await;
        }
        match repository.apply_organization(context, job_id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                // A source-fence or card-CAS conflict after all Core writes
                // were witnessed is terminal. Leave no written job pending
                // for an endless recovery loop.
                let _ = repository
                    .mark_job_failed(context, job_id, "semantic_recovery_apply_failed", true)
                    .await;
                Err(MemoryError::State(error))
            }
        }
    }

    pub async fn recover_pending_organizations(
        &self,
        context: &VaultContext,
        core: &VaultCore,
    ) -> Result<(), MemoryError> {
        super::preflight_all_semantic_recovery(&self.state, context).await?;
        self.recover_pending_organizations_after_core(context, core)
            .await
    }

    /// Check all pending organization job source/rules fences without
    /// entering Vault Core recovery. Startup runs this before the generic
    /// maintenance pass.
    pub async fn preflight_pending_organizations(
        &self,
        context: &VaultContext,
    ) -> Result<(), MemoryError> {
        let jobs = self
            .state
            .semantic_organization()
            .pending_jobs(context)
            .await?;
        let rules_revision = self
            .state
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        let repository = self.state.semantic_organization();
        for job_id in &jobs {
            let job = repository
                .get_job(context, *job_id)
                .await?
                .ok_or(MemoryError::NotFound)?;
            if job.rules_revision != rules_revision {
                repository
                    .mark_job_failed(context, *job_id, "semantic_rules_changed", true)
                    .await?;
                return Err(MemoryError::Conflict);
            }
            if let Err(error) = repository.validate_job_write_fence(context, *job_id).await {
                if !matches!(error, mcp_vault_state::StateError::Conflict) {
                    return Err(MemoryError::State(error));
                }
                repository
                    .mark_job_failed(context, *job_id, "semantic_source_fence_conflict", true)
                    .await?;
                return Err(MemoryError::Conflict);
            }
        }
        Ok(())
    }

    /// Apply pending organization jobs after generic Core recovery has
    /// completed. Each job performs its own source/rules/witness rechecks.
    pub async fn recover_pending_organizations_after_core(
        &self,
        context: &VaultContext,
        core: &VaultCore,
    ) -> Result<(), MemoryError> {
        let jobs = self
            .state
            .semantic_organization()
            .pending_jobs(context)
            .await?;
        let mut first_error = None;
        for job in jobs {
            if let Err(error) = self
                .recover_organization_inner(context, core, job, false, None)
                .await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn build_candidates(observations: Vec<OrganizationObservation>) -> Vec<OrganizationCandidate> {
    let mut candidates = Vec::new();
    for (left_index, left) in observations.iter().enumerate() {
        for right in observations.iter().skip(left_index + 1) {
            if left.source_id == right.source_id
                || left.kind != right.kind
                || token_overlap(&left.statement, &right.statement) < 0.5
            {
                continue;
            }
            let id = stable_candidate_id(left, right);
            candidates.push(OrganizationCandidate {
                id,
                left: left.clone(),
                right: right.clone(),
            });
        }
    }
    candidates
}

fn candidate_input(candidate: &OrganizationCandidate) -> RelationCandidateInput {
    RelationCandidateInput {
        id: candidate.id,
        left_observation_id: candidate.left.id,
        left_source_id: candidate.left.source_id,
        left_source_revision_id: candidate.left.source_revision_id,
        right_observation_id: candidate.right.id,
        right_source_id: candidate.right.source_id,
        right_source_revision_id: candidate.right.source_revision_id,
        candidate_input_hash: hash_bytes(
            format!("{}:{}", candidate.left.statement, candidate.right.statement).as_bytes(),
        ),
        similarity_hint: None,
        profile_id: "semantic-memory-m2-v1".to_owned(),
    }
}

fn build_state_proposal(
    _context: &VaultContext,
    _job_id: mcp_vault_domain::OrganizationJobId,
    managed_root: &VaultPath,
    proposal: &OrganizationProposal,
    candidates: &HashMap<String, OrganizationCandidate>,
    _fences: &[OrganizationSourceFence],
) -> Result<(Vec<RelationDecisionInput>, Vec<ComposedCardRevisionInput>), MemoryError> {
    if proposal.actions.is_empty() {
        return Err(MemoryError::GeneratedOutput(
            "semantic_organization_action_required",
        ));
    }
    let mut decisions = Vec::new();
    let mut cards = BTreeMap::<String, CardBuilder>::new();
    let mut seen_candidates = HashSet::new();
    for action in &proposal.actions {
        if action.candidate_ids.is_empty() {
            return Err(MemoryError::GeneratedOutput(
                "semantic_organization_candidate_required",
            ));
        }
        let selected = action
            .candidate_ids
            .iter()
            .map(|id| {
                candidates.get(id).ok_or(MemoryError::GeneratedOutput(
                    "semantic_organization_candidate_invalid",
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let first = selected[0];
        for candidate in &selected {
            if !seen_candidates.insert(candidate.id) {
                return Err(MemoryError::GeneratedOutput(
                    "semantic_organization_candidate_reused",
                ));
            }
        }
        match action.action {
            OrganizationActionKind::CreateComposedCard => {
                let card_ref = action
                    .card_ref
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_card_ref_required",
                    ))?;
                let title = action
                    .title
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_title_required",
                    ))?;
                let operator = action.support_operator.as_deref().unwrap_or("or");
                if !matches!(operator, "and" | "or") {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_organization_support_operator",
                    ));
                }
                if selected
                    .iter()
                    .any(|candidate| !equivalent_candidate(candidate))
                {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_organization_equivalent_unsupported",
                    ));
                }
                if selected
                    .iter()
                    .skip(1)
                    .any(|candidate| !shares_anchor(candidate, first.left.id))
                {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_organization_non_transitive_merge",
                    ));
                }
                cards.insert(
                    card_ref.to_owned(),
                    CardBuilder::new(card_ref, title, first, selected.clone(), operator),
                );
                decisions.extend(
                    selected
                        .iter()
                        .skip(1)
                        .map(|candidate| equivalent_decision(candidate)),
                );
            }
            OrganizationActionKind::AttachEquivalentEvidence => {
                let card_ref = action
                    .card_ref
                    .as_deref()
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_card_ref_required",
                    ))?;
                let card = cards.get_mut(card_ref).ok_or(MemoryError::GeneratedOutput(
                    "semantic_organization_card_ref_invalid",
                ))?;
                for candidate in selected {
                    if !equivalent_candidate(candidate) {
                        return Err(MemoryError::GeneratedOutput(
                            "semantic_organization_equivalent_unsupported",
                        ));
                    }
                    card.add_support(candidate);
                    decisions.push(equivalent_decision(candidate));
                }
            }
            OrganizationActionKind::AddSupportedInformation => {
                let card_ref = action
                    .card_ref
                    .as_deref()
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_card_ref_required",
                    ))?;
                let card = cards.get_mut(card_ref).ok_or(MemoryError::GeneratedOutput(
                    "semantic_organization_card_ref_invalid",
                ))?;
                let content = action
                    .content
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_content_required",
                    ))?;
                for candidate in selected {
                    if candidate.left.scope != candidate.right.scope {
                        return Err(MemoryError::GeneratedOutput(
                            "semantic_organization_scope_mismatch",
                        ));
                    }
                    card.add_item(
                        content,
                        action.item_kind.as_deref().unwrap_or("optional_detail"),
                        candidate,
                    );
                    decisions.push(supplement_decision(candidate));
                }
            }
            OrganizationActionKind::SupersedeWithEvidence => {
                if action.reason.as_deref().is_none_or(str::is_empty)
                    || selected.len() != 1
                    || first.left.scope != first.right.scope
                {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_organization_supersede_evidence_required",
                    ));
                }
                decisions.push(relation_decision(
                    first,
                    "supersedes",
                    action.reason.as_deref().unwrap_or("explicit"),
                ));
                let card_ref = action
                    .card_ref
                    .as_deref()
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_card_ref_required",
                    ))?;
                let title = action
                    .title
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .ok_or(MemoryError::GeneratedOutput(
                        "semantic_organization_title_required",
                    ))?;
                cards.insert(
                    card_ref.to_owned(),
                    CardBuilder::new_right(card_ref, title, first),
                );
            }
            OrganizationActionKind::RecordConflict => {
                decisions.push(relation_decision(
                    first,
                    "conflicts",
                    action.reason.as_deref().unwrap_or("conflict"),
                ));
            }
            OrganizationActionKind::LinkRelatedOnly => {
                decisions.push(relation_decision(first, "related", "navigation_only"));
            }
            OrganizationActionKind::KeepSeparateScope => {
                if first.left.scope == first.right.scope {
                    return Err(MemoryError::GeneratedOutput(
                        "semantic_organization_scope_not_different",
                    ));
                }
                decisions.push(relation_decision(
                    first,
                    "different_scope",
                    "different_scope",
                ));
            }
            OrganizationActionKind::NoChange => {}
        }
    }
    Ok((
        decisions,
        cards
            .into_values()
            .map(|builder| builder.into_input(managed_root))
            .collect(),
    ))
}

#[derive(Clone)]
struct CardBuilder {
    identity_key: String,
    title: String,
    anchor: OrganizationObservation,
    support_members: Vec<SupportMemberInput>,
    items: Vec<(String, String, OrganizationCandidate)>,
    operator: String,
}

impl CardBuilder {
    fn new(
        _card_ref: &str,
        title: &str,
        first: &OrganizationCandidate,
        support: Vec<&OrganizationCandidate>,
        operator: &str,
    ) -> Self {
        Self {
            identity_key: stable_card_identity(first.left.clone(), operator),
            title: title.to_owned(),
            anchor: first.left.clone(),
            support_members: support
                .iter()
                .flat_map(|candidate| member_inputs(candidate))
                .collect(),
            items: Vec::new(),
            operator: operator.to_owned(),
        }
    }

    fn new_right(_card_ref: &str, title: &str, candidate: &OrganizationCandidate) -> Self {
        Self {
            identity_key: stable_card_identity(candidate.right.clone(), "or"),
            title: title.to_owned(),
            anchor: candidate.right.clone(),
            support_members: vec![member_input_observation(&candidate.right)],
            items: Vec::new(),
            operator: "or".to_owned(),
        }
    }

    fn add_support(&mut self, candidate: &OrganizationCandidate) {
        self.support_members.extend(member_inputs(candidate));
    }

    fn add_item(&mut self, content: &str, kind: &str, candidate: &OrganizationCandidate) {
        self.items
            .push((content.to_owned(), kind.to_owned(), candidate.clone()));
    }

    fn into_input(self, managed_root: &VaultPath) -> ComposedCardRevisionInput {
        let card_id = ComposedCardId::new();
        let card_revision_id = ComposedCardRevisionId::new();
        let mut items = Vec::new();
        let mut markdown = format!("# {}\n\n", self.title);
        let core_members = self.support_members;
        // Card dependencies are the actual support edges. The organization
        // job keeps the complete source freshness fence separately in State;
        // copying that fence here would let an unrelated source suppression
        // block this card's regeneration.
        let mut dependencies = Vec::new();
        for member in &core_members {
            let dependency = (member.source_id, member.source_revision_id);
            if !dependencies.contains(&dependency) {
                dependencies.push(dependency);
            }
        }
        for (_, _, candidate) in &self.items {
            for member in member_inputs(candidate) {
                let dependency = (member.source_id, member.source_revision_id);
                if !dependencies.contains(&dependency) {
                    dependencies.push(dependency);
                }
            }
        }
        dependencies.sort_by_key(|(source_id, revision_id)| {
            (source_id.to_string(), revision_id.to_string())
        });
        markdown.push_str(&format!("- {}\n", self.anchor.statement));
        items.push(ComposedCardItemInput {
            id: ComposedCardItemId::new(),
            kind: "core_assertion".to_owned(),
            ordinal: 0,
            content: self.anchor.statement.clone(),
            support_groups: vec![SupportGroupInput {
                id: SupportGroupId::new(),
                operator: self.operator,
                ordinal: 0,
                members: core_members.clone(),
            }],
        });
        let mut add_structured = |kind: &str, content: String, ordinal: u32| {
            markdown.push_str(&format!("- {kind}: {content}\n"));
            items.push(ComposedCardItemInput {
                id: ComposedCardItemId::new(),
                kind: kind.to_owned(),
                ordinal,
                content,
                support_groups: vec![SupportGroupInput {
                    id: SupportGroupId::new(),
                    operator: "or".to_owned(),
                    ordinal: 0,
                    members: fresh_members(&core_members),
                }],
            });
        };
        let mut ordinal = 1_u32;
        for condition in &self.anchor.conditions {
            add_structured("condition", condition.clone(), ordinal);
            ordinal = ordinal.saturating_add(1);
        }
        for exception in &self.anchor.exceptions {
            add_structured("exception", exception.clone(), ordinal);
            ordinal = ordinal.saturating_add(1);
        }
        for step in &self.anchor.ordered_steps {
            add_structured("ordered_step", step.clone(), ordinal);
            ordinal = ordinal.saturating_add(1);
        }
        if let Some(result) = &self.anchor.result {
            add_structured("optional_detail", result.clone(), ordinal);
            ordinal = ordinal.saturating_add(1);
        }
        if let Some(uncertainty) = &self.anchor.uncertainty {
            add_structured("unresolved_item", uncertainty.clone(), ordinal);
            ordinal = ordinal.saturating_add(1);
        }
        add_structured(
            "optional_detail",
            self.anchor.value_for_future_work.clone(),
            ordinal,
        );
        for (content, kind, candidate) in self.items {
            let item_ordinal = ordinal;
            ordinal = ordinal.saturating_add(1);
            markdown.push_str(&format!("- {}\n", content));
            items.push(ComposedCardItemInput {
                id: ComposedCardItemId::new(),
                kind,
                ordinal: item_ordinal,
                content,
                support_groups: vec![SupportGroupInput {
                    id: SupportGroupId::new(),
                    operator: "single".replace("single", "or"),
                    ordinal: 0,
                    members: member_inputs(&candidate),
                }],
            });
        }
        let bytes = markdown.into_bytes();
        ComposedCardRevisionInput {
            card_id,
            card_revision_id,
            expected_card_revision_id: None,
            revision_number: 1,
            identity_key: self.identity_key,
            topic_key: normalize(&self.title),
            title: self.title,
            kind: self.anchor.kind.clone(),
            scope_ref: self.anchor.scope.clone(),
            assertion_status: self.anchor.assertion_status.clone(),
            temporal_scope: self.anchor.source_time_scope.clone(),
            composition_profile_id: "semantic-memory-m2-v1".to_owned(),
            canonical_path: managed_root
                .join(&VaultPath::parse(&format!("semantic-memory/composed/{card_id}.md")).unwrap())
                .unwrap(),
            canonical_markdown_hash: hash_bytes(&bytes),
            canonical_bytes: bytes,
            items,
            dependencies,
        }
    }
}

fn member_inputs(candidate: &OrganizationCandidate) -> Vec<SupportMemberInput> {
    [&candidate.left, &candidate.right]
        .into_iter()
        .map(|observation| SupportMemberInput {
            id: SupportMemberId::new(),
            source_id: observation.source_id,
            source_revision_id: observation.source_revision_id,
            observation_id: observation.id,
            evidence_ref_id: observation.evidence_ref_id,
            member_role: "complete".to_owned(),
        })
        .collect()
}

fn member_input_observation(observation: &OrganizationObservation) -> SupportMemberInput {
    SupportMemberInput {
        id: SupportMemberId::new(),
        source_id: observation.source_id,
        source_revision_id: observation.source_revision_id,
        observation_id: observation.id,
        evidence_ref_id: observation.evidence_ref_id,
        member_role: "complete".to_owned(),
    }
}

fn stable_card_identity(anchor: OrganizationObservation, operator: &str) -> String {
    let material = serde_json::json!([
        "semantic-memory-m2-card-v1",
        anchor.kind,
        anchor.scope,
        anchor.assertion_status,
        anchor.statement,
        anchor.conditions,
        anchor.exceptions,
        anchor.ordered_steps,
        anchor.result,
        anchor.uncertainty,
        anchor.source_time_scope,
        anchor.value_for_future_work,
        operator,
    ]);
    format!("card_{}", hash_bytes(material.to_string().as_bytes()))
}

fn fresh_members(members: &[SupportMemberInput]) -> Vec<SupportMemberInput> {
    members
        .iter()
        .map(|member| SupportMemberInput {
            id: SupportMemberId::new(),
            source_id: member.source_id,
            source_revision_id: member.source_revision_id,
            observation_id: member.observation_id,
            evidence_ref_id: member.evidence_ref_id,
            member_role: member.member_role.clone(),
        })
        .collect()
}

fn equivalent_candidate(candidate: &OrganizationCandidate) -> bool {
    candidate.left.scope == candidate.right.scope
        && candidate.left.kind == candidate.right.kind
        && candidate.left.assertion_status == candidate.right.assertion_status
        && normalize(&candidate.left.statement) == normalize(&candidate.right.statement)
        && candidate.left.conditions == candidate.right.conditions
        && candidate.left.exceptions == candidate.right.exceptions
        && candidate.left.ordered_steps == candidate.right.ordered_steps
        && candidate.left.result == candidate.right.result
        && candidate.left.uncertainty == candidate.right.uncertainty
        && candidate.left.source_time_scope == candidate.right.source_time_scope
        && candidate.left.value_for_future_work == candidate.right.value_for_future_work
}

fn shares_anchor(
    candidate: &OrganizationCandidate,
    anchor: mcp_vault_domain::ObservationId,
) -> bool {
    candidate.left.id == anchor || candidate.right.id == anchor
}

fn equivalent_decision(candidate: &OrganizationCandidate) -> RelationDecisionInput {
    relation_decision(candidate, "equivalent", "same_complete_proposition")
}
fn supplement_decision(candidate: &OrganizationCandidate) -> RelationDecisionInput {
    relation_decision(
        candidate,
        "supplements",
        "independent_supported_information",
    )
}
fn relation_decision(
    candidate: &OrganizationCandidate,
    kind: &str,
    reason: &str,
) -> RelationDecisionInput {
    RelationDecisionInput {
        id: RelationDecisionId::new(),
        candidate_id: candidate.id,
        relation_kind: kind.to_owned(),
        decision_state: "accepted".to_owned(),
        decision_reason_code: reason.to_owned(),
        profile_id: "semantic-memory-m2-v1".to_owned(),
        evidence: vec![
            RelationEvidenceInput {
                evidence_ref_id: candidate.left.evidence_ref_id,
                source_id: candidate.left.source_id,
                source_revision_id: candidate.left.source_revision_id,
                role: "left".to_owned(),
            },
            RelationEvidenceInput {
                evidence_ref_id: candidate.right.evidence_ref_id,
                source_id: candidate.right.source_id,
                source_revision_id: candidate.right.source_revision_id,
                role: "right".to_owned(),
            },
        ],
    }
}

fn stable_candidate_id(
    left: &OrganizationObservation,
    right: &OrganizationObservation,
) -> RelationCandidateId {
    let mut hasher = Sha256::new();
    hasher.update(left.id.to_string());
    hasher.update(right.id.to_string());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    RelationCandidateId::from_uuid(Uuid::from_bytes(bytes))
}

fn job_candidate_id(
    job_id: mcp_vault_domain::OrganizationJobId,
    discovered_id: RelationCandidateId,
) -> RelationCandidateId {
    let mut hasher = Sha256::new();
    hasher.update(job_id.to_string());
    hasher.update(discovered_id.to_string());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    RelationCandidateId::from_uuid(Uuid::from_bytes(bytes))
}

fn token_overlap(left: &str, right: &str) -> f32 {
    let left = left.split_whitespace().collect::<BTreeSet<_>>();
    let right = right.split_whitespace().collect::<BTreeSet<_>>();
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    left.intersection(&right).count() as f32 / left.union(&right).count() as f32
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn action_kind_name(action: &OrganizationActionKind) -> &'static str {
    match action {
        OrganizationActionKind::CreateComposedCard => "create_composed_card",
        OrganizationActionKind::AttachEquivalentEvidence => "attach_equivalent_evidence",
        OrganizationActionKind::AddSupportedInformation => "add_supported_information",
        OrganizationActionKind::SupersedeWithEvidence => "supersede_with_evidence",
        OrganizationActionKind::RecordConflict => "record_conflict",
        OrganizationActionKind::LinkRelatedOnly => "link_related_only",
        OrganizationActionKind::KeepSeparateScope => "keep_separate_scope",
        OrganizationActionKind::NoChange => "no_change",
    }
}

async fn ensure_composed_file(
    context: &VaultContext,
    core: &VaultCore,
    snapshot: &mcp_vault_state::OrganizationPreparedSnapshot,
) -> Result<mcp_vault_state::FileRecord, MemoryError> {
    match core.read_managed(context, &snapshot.target_path).await {
        Ok(mut read) => {
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut read.reader, &mut bytes)
                .await
                .map_err(|_| MemoryError::SourceIngestion("semantic_composed_card_read_failed"))?;
            if bytes == snapshot.canonical_bytes
                && hash_bytes(&bytes) == snapshot.proposed_file_hash
            {
                return Ok(read.file);
            }
            if snapshot.expected_file_id != Some(read.file.id)
                || snapshot.expected_file_revision != Some(read.file.current_revision)
            {
                return Err(MemoryError::Conflict);
            }
            let result = core
                .replace_managed_bytes(
                    context,
                    &snapshot.target_path,
                    read.file.current_revision,
                    &snapshot.canonical_bytes,
                    Actor::system(),
                    SourcePlane::System,
                    Some(&format!("semantic-composed:{}", snapshot.id)),
                )
                .await?;
            Ok(result.file)
        }
        Err(mcp_vault_core::VaultError::NotFound) => {
            let result = core
                .create_managed_bytes(
                    context,
                    &snapshot.target_path,
                    &snapshot.canonical_bytes,
                    Actor::system(),
                    SourcePlane::System,
                    Some(&format!("semantic-composed:{}", snapshot.id)),
                )
                .await?;
            Ok(result.file)
        }
        Err(error) => Err(error.into()),
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_vault_domain::ObservationId;

    fn observation(id: ObservationId, statement: &str, scope: &str) -> OrganizationObservation {
        OrganizationObservation {
            id,
            source_id: SemanticSourceId::new(),
            source_revision_id: mcp_vault_domain::SourceRevisionId::new(),
            evidence_ref_id: mcp_vault_domain::EvidenceRefId::new(),
            kind: "decision".to_owned(),
            statement: statement.to_owned(),
            scope: scope.to_owned(),
            assertion_status: "source_asserted".to_owned(),
            source_time_scope: serde_json::json!({"status":"source_stated","value":"2026"}),
            conditions: vec!["Only after approval".to_owned()],
            exceptions: vec!["Except emergency rollback".to_owned()],
            ordered_steps: vec!["Prepare".to_owned(), "Verify".to_owned()],
            result: Some("Rollback remains possible".to_owned()),
            uncertainty: Some("Needs recheck".to_owned()),
            value_for_future_work: "Keep the boundary".to_owned(),
        }
    }

    fn candidate(
        left: &OrganizationObservation,
        right: &OrganizationObservation,
    ) -> OrganizationCandidate {
        OrganizationCandidate {
            id: stable_candidate_id(left, right),
            left: left.clone(),
            right: right.clone(),
        }
    }

    #[test]
    fn different_scope_is_a_candidate_but_not_equivalent() {
        let left = observation(
            ObservationId::new(),
            "Deploy with rollback enabled",
            "production",
        );
        let right = observation(ObservationId::new(), "Deploy with rollback enabled", "test");
        let found = build_candidates(vec![left.clone(), right.clone()]);
        assert_eq!(found.len(), 1);
        assert!(!equivalent_candidate(&found[0]));
    }

    #[test]
    fn differing_qualifiers_cannot_be_folded_into_an_equivalent_composed_card() {
        for field in [
            "result",
            "uncertainty",
            "source_time_scope",
            "value_for_future_work",
        ] {
            let left = observation(
                ObservationId::new(),
                "Deploy with rollback enabled",
                "project",
            );
            let mut right = observation(
                ObservationId::new(),
                "Deploy with rollback enabled",
                "project",
            );
            match field {
                "result" => right.result = Some("Rollback is unavailable".to_owned()),
                "uncertainty" => right.uncertainty = Some("Requires approval".to_owned()),
                "source_time_scope" => {
                    right.source_time_scope = serde_json::json!({
                        "status": "source_stated",
                        "value": "2027"
                    });
                }
                "value_for_future_work" => {
                    right.value_for_future_work = "Prefer the safer path".to_owned();
                }
                _ => unreachable!("the test field list is exhaustive"),
            }

            let mut found = build_candidates(vec![left, right]);
            assert_eq!(found.len(), 1, "{field} must remain a candidate");
            let relation = found.pop().expect("candidate");
            assert!(
                !equivalent_candidate(&relation),
                "{field} difference must prevent equivalent folding"
            );

            let proposal = OrganizationProposal {
                actions: vec![OrganizationAction {
                    action: OrganizationActionKind::CreateComposedCard,
                    candidate_ids: vec![relation.id.to_string()],
                    card_ref: Some("bounded".to_owned()),
                    title: Some("Bounded deployment".to_owned()),
                    content: None,
                    item_kind: None,
                    support_operator: Some("or".to_owned()),
                    reason: None,
                }],
            };
            let mut candidates = HashMap::new();
            candidates.insert(relation.id.to_string(), relation);
            let context = VaultContext::new(
                mcp_vault_domain::VaultId::new(),
                mcp_vault_domain::VaultSlug::new("test").unwrap(),
                std::path::PathBuf::from("/tmp/test"),
                mcp_vault_domain::Revision::ZERO,
            )
            .unwrap();
            assert!(
                build_state_proposal(
                    &context,
                    mcp_vault_domain::OrganizationJobId::new(),
                    &VaultPath::parse("_mcp-vault").unwrap(),
                    &proposal,
                    &candidates,
                    &[],
                )
                .is_err(),
                "{field} difference must not create a composed equivalent card"
            );
        }
    }

    #[test]
    fn composed_rendering_keeps_conditions_exceptions_order_result_and_uncertainty() {
        let left = observation(
            ObservationId::new(),
            "Deploy with rollback enabled",
            "project",
        );
        let right = observation(
            ObservationId::new(),
            "Deploy with rollback enabled",
            "project",
        );
        let relation = candidate(&left, &right);
        let input = CardBuilder::new("decision", "Decision", &relation, vec![&relation], "or")
            .into_input(&VaultPath::parse("_mcp-vault").unwrap());
        let kinds = input
            .items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"condition"));
        assert!(kinds.contains(&"exception"));
        assert!(kinds.contains(&"ordered_step"));
        assert!(kinds.contains(&"optional_detail"));
        assert!(kinds.contains(&"unresolved_item"));
        assert!(
            input
                .items
                .iter()
                .any(|item| item.content == "Except emergency rollback")
        );
    }

    #[test]
    fn composed_cards_preserve_non_decision_kind_in_identity_and_revision() {
        let mut left = observation(ObservationId::new(), "Verify the rollout", "project");
        let mut right = observation(ObservationId::new(), "Verify the rollout", "project");
        left.kind = "procedure".to_owned();
        right.kind = "procedure".to_owned();
        let procedure = candidate(&left, &right);
        let procedure_input = CardBuilder::new(
            "procedure",
            "Rollout procedure",
            &procedure,
            vec![&procedure],
            "or",
        )
        .into_input(&VaultPath::parse("_mcp-vault").unwrap());
        assert_eq!(procedure_input.kind, "procedure");

        let decision = observation(ObservationId::new(), "Verify the rollout", "project");
        let decision_candidate = candidate(
            &decision,
            &observation(ObservationId::new(), "Verify the rollout", "project"),
        );
        let decision_input = CardBuilder::new(
            "decision",
            "Rollout procedure",
            &decision_candidate,
            vec![&decision_candidate],
            "or",
        )
        .into_input(&VaultPath::parse("_mcp-vault").unwrap());
        assert_ne!(procedure_input.identity_key, decision_input.identity_key);
    }

    #[test]
    fn supersedes_accepts_same_scope_different_statement_with_explicit_reason() {
        let left = observation(ObservationId::new(), "Use rollout A", "project");
        let right = observation(ObservationId::new(), "Use rollout B", "project");
        let relation = candidate(&left, &right);
        let proposal = OrganizationProposal {
            actions: vec![OrganizationAction {
                action: OrganizationActionKind::SupersedeWithEvidence,
                candidate_ids: vec![relation.id.to_string()],
                card_ref: Some("current".to_owned()),
                title: Some("Current rollout".to_owned()),
                content: None,
                item_kind: None,
                support_operator: None,
                reason: Some("The source explicitly says to switch to B".to_owned()),
            }],
        };
        let mut map = HashMap::new();
        map.insert(relation.id.to_string(), relation);
        let (decisions, cards) = build_state_proposal(
            &VaultContext::new(
                mcp_vault_domain::VaultId::new(),
                mcp_vault_domain::VaultSlug::new("test").unwrap(),
                std::path::PathBuf::from("/tmp/test"),
                mcp_vault_domain::Revision::ZERO,
            )
            .unwrap(),
            mcp_vault_domain::OrganizationJobId::new(),
            &VaultPath::parse("_mcp-vault").unwrap(),
            &proposal,
            &map,
            &[],
        )
        .unwrap();
        assert_eq!(decisions[0].relation_kind, "supersedes");
        assert_eq!(cards[0].items[0].content, "Use rollout B");
    }

    #[test]
    fn no_change_is_a_successful_empty_state_proposal() {
        let left = observation(ObservationId::new(), "Keep A", "project");
        let right = observation(ObservationId::new(), "Keep A", "project");
        let relation = candidate(&left, &right);
        let proposal = OrganizationProposal {
            actions: vec![OrganizationAction {
                action: OrganizationActionKind::NoChange,
                candidate_ids: vec![relation.id.to_string()],
                card_ref: None,
                title: None,
                content: None,
                item_kind: None,
                support_operator: None,
                reason: Some("No organization change".to_owned()),
            }],
        };
        let mut map = HashMap::new();
        map.insert(relation.id.to_string(), relation);
        let result = build_state_proposal(
            &VaultContext::new(
                mcp_vault_domain::VaultId::new(),
                mcp_vault_domain::VaultSlug::new("test").unwrap(),
                std::path::PathBuf::from("/tmp/test"),
                mcp_vault_domain::Revision::ZERO,
            )
            .unwrap(),
            mcp_vault_domain::OrganizationJobId::new(),
            &VaultPath::parse("_mcp-vault").unwrap(),
            &proposal,
            &map,
            &[],
        )
        .unwrap();
        assert!(result.0.is_empty() && result.1.is_empty());
    }

    #[test]
    fn create_rejects_transitive_merge_without_direct_anchor_relation() {
        let a = observation(ObservationId::new(), "Deploy A", "project");
        let b = observation(ObservationId::new(), "Deploy B", "project");
        let c = observation(ObservationId::new(), "Deploy C", "project");
        let ab = candidate(&a, &b);
        let bc = candidate(&b, &c);
        let proposal = OrganizationProposal {
            actions: vec![OrganizationAction {
                action: OrganizationActionKind::CreateComposedCard,
                candidate_ids: vec![ab.id.to_string(), bc.id.to_string()],
                card_ref: Some("transitive".to_owned()),
                title: Some("Transitive".to_owned()),
                content: None,
                item_kind: None,
                support_operator: Some("or".to_owned()),
                reason: None,
            }],
        };
        let mut map = HashMap::new();
        map.insert(ab.id.to_string(), ab);
        map.insert(bc.id.to_string(), bc);
        assert!(
            build_state_proposal(
                &VaultContext::new(
                    mcp_vault_domain::VaultId::new(),
                    mcp_vault_domain::VaultSlug::new("test").unwrap(),
                    std::path::PathBuf::from("/tmp/test"),
                    mcp_vault_domain::Revision::ZERO
                )
                .unwrap(),
                mcp_vault_domain::OrganizationJobId::new(),
                &VaultPath::parse("_mcp-vault").unwrap(),
                &proposal,
                &map,
                &[]
            )
            .is_err()
        );
    }
}
