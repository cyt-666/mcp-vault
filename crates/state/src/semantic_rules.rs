//! Vault-scoped M3 lifecycle rules and stable semantic target identity.
//!
//! Rules are operational state, not model-controlled durable IDs. The State
//! boundary derives target keys from a kind/scope/fingerprint tuple, advances
//! one monotonic rules revision per mutation, and records idempotent actions.

use mcp_vault_domain::VaultContext;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::{StateError, now_millis};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTargetRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub target_key: String,
    pub target_kind: String,
    pub scope_ref: String,
    pub fingerprint_version: i64,
    pub fingerprint: String,
    pub rules_revision: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTargetSourceBinding {
    pub source_id: String,
    pub source_revision_id: String,
    pub observation_id: Option<String>,
    pub evidence_ref_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTargetResolution {
    pub vault_id: mcp_vault_domain::VaultId,
    pub target_ref: String,
    pub target_key: String,
    pub target_kind: String,
    pub scope_ref: String,
    pub fingerprint_version: i64,
    pub fingerprint: String,
    pub rules_revision: i64,
    pub parent_revision: i64,
    pub card_id: Option<String>,
    pub composed_card_id: Option<String>,
    pub evidence_ref_id: Option<String>,
    pub source_bindings: Vec<SemanticTargetSourceBinding>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticRuleRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub rule_kind: String,
    pub id: String,
    pub target_key: String,
    pub action: String,
    pub scope_ref: String,
    pub active: bool,
    pub revision: i64,
    pub rules_revision: i64,
    pub source_id: Option<String>,
    pub source_revision_id: Option<String>,
    pub observation_id: Option<String>,
    pub card_id: Option<String>,
    pub composed_card_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticTaskStateRecord {
    pub vault_id: mcp_vault_domain::VaultId,
    pub task_id: String,
    pub target_key: String,
    pub state: String,
    pub evidence_observation_id: Option<String>,
    pub evidence_source_id: Option<String>,
    pub evidence_source_revision_id: Option<String>,
    pub rules_revision: i64,
}

#[derive(Clone)]
pub struct SemanticRulesRepository {
    pool: SqlitePool,
}

impl SemanticRulesRepository {
    pub(crate) fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn current_rules_revision(&self, context: &VaultContext) -> Result<i64, StateError> {
        current_rules_revision_pool(&self.pool, context).await
    }

    /// Derive and persist a stable target key. Callers provide semantic
    /// material, never a durable target ID.
    pub async fn register_target(
        &self,
        context: &VaultContext,
        target_kind: &str,
        scope_ref: &str,
        fingerprint_version: i64,
        fingerprint: &str,
    ) -> Result<SemanticTargetRecord, StateError> {
        let mut tx = self.pool.begin().await?;
        let result = ensure_target_tx(
            &mut tx,
            context,
            target_kind,
            scope_ref,
            fingerprint_version,
            fingerprint,
        )
        .await?;
        tx.commit().await?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_correction(
        &self,
        context: &VaultContext,
        target_kind: &str,
        scope_ref: &str,
        fingerprint_version: i64,
        fingerprint: &str,
        correction: &Value,
        source_id: Option<&str>,
        source_revision_id: Option<&str>,
        observation_id: Option<&str>,
        card_id: Option<&str>,
        composed_card_id: Option<&str>,
        expected_rules_revision: Option<i64>,
        authorized_actor: &str,
        idempotency_key: &str,
    ) -> Result<SemanticRuleRecord, StateError> {
        apply_rule(
            &self.pool,
            context,
            "correction",
            "replace",
            target_kind,
            scope_ref,
            fingerprint_version,
            fingerprint,
            correction,
            source_id,
            source_revision_id,
            observation_id,
            card_id,
            composed_card_id,
            expected_rules_revision,
            authorized_actor,
            idempotency_key,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_suppression(
        &self,
        context: &VaultContext,
        target_kind: &str,
        scope_ref: &str,
        fingerprint_version: i64,
        fingerprint: &str,
        policy: &Value,
        action: &str,
        source_id: Option<&str>,
        source_revision_id: Option<&str>,
        observation_id: Option<&str>,
        card_id: Option<&str>,
        composed_card_id: Option<&str>,
        expected_rules_revision: Option<i64>,
        authorized_actor: &str,
        idempotency_key: &str,
    ) -> Result<SemanticRuleRecord, StateError> {
        apply_rule(
            &self.pool,
            context,
            "suppression",
            action,
            target_kind,
            scope_ref,
            fingerprint_version,
            fingerprint,
            policy,
            source_id,
            source_revision_id,
            observation_id,
            card_id,
            composed_card_id,
            expected_rules_revision,
            authorized_actor,
            idempotency_key,
        )
        .await
    }

    pub async fn revoke_correction(
        &self,
        context: &VaultContext,
        correction_id: &str,
        expected_revision: i64,
        idempotency_key: &str,
        authorized_actor: &str,
    ) -> Result<SemanticRuleRecord, StateError> {
        revoke_rule(
            &self.pool,
            context,
            "correction",
            correction_id,
            expected_revision,
            idempotency_key,
            authorized_actor,
        )
        .await
    }

    pub async fn revoke_suppression(
        &self,
        context: &VaultContext,
        suppression_id: &str,
        expected_revision: i64,
        idempotency_key: &str,
        authorized_actor: &str,
    ) -> Result<SemanticRuleRecord, StateError> {
        revoke_rule(
            &self.pool,
            context,
            "suppression",
            suppression_id,
            expected_revision,
            idempotency_key,
            authorized_actor,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn set_task_state(
        &self,
        context: &VaultContext,
        task_id: &str,
        target_kind: &str,
        scope_ref: &str,
        fingerprint_version: i64,
        fingerprint: &str,
        state: &str,
        evidence_observation_id: Option<&str>,
        evidence_source_id: Option<&str>,
        evidence_source_revision_id: Option<&str>,
        expected_rules_revision: Option<i64>,
    ) -> Result<SemanticTaskStateRecord, StateError> {
        if !matches!(
            state,
            "not_started" | "in_progress" | "blocked" | "completed" | "unknown"
        ) {
            return Err(StateError::InvalidInput("invalid semantic task state"));
        }
        if state == "completed"
            && (evidence_observation_id.is_none()
                || evidence_source_id.is_none()
                || evidence_source_revision_id.is_none())
        {
            return Err(StateError::InvalidInput(
                "completed semantic task requires current evidence",
            ));
        }
        let mut tx = self.pool.begin().await?;
        let target = ensure_target_tx(
            &mut tx,
            context,
            target_kind,
            scope_ref,
            fingerprint_version,
            fingerprint,
        )
        .await?;
        let current_rules_revision = current_rules_revision_tx(&mut tx, context).await?;
        if expected_rules_revision.is_some_and(|expected| expected != current_rules_revision) {
            return Err(StateError::Conflict);
        }
        if state == "completed" {
            ensure_current_evidence(
                &mut tx,
                context,
                evidence_observation_id.unwrap_or_default(),
                evidence_source_id.unwrap_or_default(),
                evidence_source_revision_id.unwrap_or_default(),
            )
            .await?;
        }
        let rules_revision = current_rules_revision
            .checked_add(1)
            .ok_or(StateError::IntegrityFailure)?;
        sqlx::query(
            "UPDATE semantic_rules_runtime SET rules_revision=?,updated_at=? WHERE vault_id=?",
        )
        .bind(rules_revision)
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .execute(&mut *tx)
        .await?;
        let now = now_millis()?;
        sqlx::query(
            "INSERT INTO semantic_task_states
             (vault_id,task_id,target_key,state,evidence_observation_id,evidence_source_id,evidence_source_revision_id,rules_revision,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?)
             ON CONFLICT(vault_id,task_id) DO UPDATE SET target_key=excluded.target_key,state=excluded.state,
             evidence_observation_id=excluded.evidence_observation_id,evidence_source_id=excluded.evidence_source_id,
             evidence_source_revision_id=excluded.evidence_source_revision_id,rules_revision=excluded.rules_revision,
             updated_at=excluded.updated_at",
        )
        .bind(context.id().to_string())
        .bind(task_id)
        .bind(&target.target_key)
        .bind(state)
        .bind(evidence_observation_id)
        .bind(evidence_source_id)
        .bind(evidence_source_revision_id)
        .bind(rules_revision)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SemanticTaskStateRecord {
            vault_id: target.vault_id,
            task_id: task_id.to_owned(),
            target_key: target.target_key,
            state: state.to_owned(),
            evidence_observation_id: evidence_observation_id.map(str::to_owned),
            evidence_source_id: evidence_source_id.map(str::to_owned),
            evidence_source_revision_id: evidence_source_revision_id.map(str::to_owned),
            rules_revision,
        })
    }

    pub async fn target(
        &self,
        context: &VaultContext,
        target_key: &str,
    ) -> Result<Option<SemanticTargetRecord>, StateError> {
        let row = sqlx::query_as::<_, (String, String, String, i64, String)>(
            "SELECT target_key,target_kind,scope_ref,fingerprint_version,fingerprint
             FROM semantic_targets WHERE vault_id=? AND target_key=?",
        )
        .bind(context.id().to_string())
        .bind(target_key)
        .fetch_optional(&self.pool)
        .await?;
        let Some((key, kind, scope, version, fingerprint)) = row else {
            return Ok(None);
        };
        Ok(Some(SemanticTargetRecord {
            vault_id: context.id(),
            target_key: key,
            target_kind: kind,
            scope_ref: scope,
            fingerprint_version: version,
            fingerprint,
            rules_revision: self.current_rules_revision(context).await?,
        }))
    }

    pub async fn list_targets(
        &self,
        context: &VaultContext,
    ) -> Result<Vec<SemanticTargetRecord>, StateError> {
        let rows = sqlx::query_as::<_, (String, String, String, i64, String)>(
            "SELECT target_key,target_kind,scope_ref,fingerprint_version,fingerprint
             FROM semantic_targets WHERE vault_id=? ORDER BY target_key",
        )
        .bind(context.id().to_string())
        .fetch_all(&self.pool)
        .await?;
        let rules_revision = self.current_rules_revision(context).await?;
        rows.into_iter()
            .map(
                |(target_key, target_kind, scope_ref, fingerprint_version, fingerprint)| {
                    Ok(SemanticTargetRecord {
                        vault_id: context.id(),
                        target_key,
                        target_kind,
                        scope_ref,
                        fingerprint_version,
                        fingerprint,
                        rules_revision,
                    })
                },
            )
            .collect()
    }

    /// Resolve only an explicit, typed semantic reference. This never searches
    /// arbitrary text and every query is Vault-scoped.
    pub async fn resolve_semantic_target(
        &self,
        context: &VaultContext,
        target_ref: &str,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        self.resolve_semantic_target_inner(context, target_ref, None)
            .await
    }

    pub async fn resolve_semantic_target_for_parent(
        &self,
        context: &VaultContext,
        target_ref: &str,
        parent_ref: &str,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        self.resolve_semantic_target_inner(context, target_ref, Some(parent_ref))
            .await
    }

    async fn resolve_semantic_target_inner(
        &self,
        context: &VaultContext,
        target_ref: &str,
        parent_ref: Option<&str>,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        let (kind, id) = target_ref.split_once(':').ok_or(StateError::InvalidInput(
            "semantic target reference is invalid",
        ))?;
        match kind {
            "card" => self.resolve_card_target(context, target_ref, id).await,
            "composed_card" => self.resolve_composed_target(context, target_ref, id).await,
            "evidence" => {
                self.resolve_evidence_target(context, target_ref, id, parent_ref)
                    .await
            }
            _ => Err(StateError::InvalidInput(
                "semantic target kind is unsupported",
            )),
        }
    }

    async fn resolve_card_target(
        &self,
        context: &VaultContext,
        target_ref: &str,
        id: &str,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        let header: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT c.card_id,r.semantic_target_key,r.scope_ref,r.card_revision_id
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r
               ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             JOIN semantic_sources s
               ON s.vault_id=c.vault_id AND s.source_id=c.source_id
             WHERE c.vault_id=? AND c.card_id=? AND c.eligibility='readable'
               AND r.publication_state='published' AND s.eligible=1 AND s.pending_rebuild=0
               AND s.current_revision_id=r.source_revision_id",
        )
        .bind(context.id().to_string())
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((card_id, target_key, scope_ref, revision_id)) = header else {
            return Ok(None);
        };
        let bindings = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT DISTINCT a.source_id,a.observation_id,a.evidence_ref_id,o.source_revision_id
             FROM semantic_card_items i
             JOIN semantic_card_assertion_supports a
               ON a.vault_id=i.vault_id AND a.item_id=i.item_id AND a.card_revision_id=i.card_revision_id
             JOIN semantic_observations o
               ON o.vault_id=a.vault_id AND o.observation_id=a.observation_id AND o.source_id=a.source_id
             WHERE i.vault_id=? AND i.card_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(revision_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|(source_id, observation_id, evidence_ref_id, source_revision_id)| {
            SemanticTargetSourceBinding {
                source_id,
                source_revision_id,
                observation_id: Some(observation_id),
                evidence_ref_id: Some(evidence_ref_id),
            }
        })
        .collect::<Vec<_>>();
        self.finish_resolution(
            context,
            target_ref,
            "card",
            target_key,
            scope_ref,
            Some(card_id),
            None,
            None,
            bindings,
        )
        .await
        .map(Some)
    }

    async fn resolve_composed_target(
        &self,
        context: &VaultContext,
        target_ref: &str,
        id: &str,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        let header: Option<(String, String, String, String)> = sqlx::query_as(
            "SELECT c.composed_card_id,r.semantic_target_key,r.scope_ref,r.composed_card_revision_id
             FROM semantic_composed_cards c
             JOIN semantic_composed_card_revisions r
               ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.composed_card_id=? AND c.eligibility='readable'
               AND r.publication_state='published'",
        )
        .bind(context.id().to_string())
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((card_id, target_key, scope_ref, revision_id)) = header else {
            return Ok(None);
        };
        let bindings = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT DISTINCT m.source_id,m.observation_id,m.evidence_ref_id,m.source_revision_id
             FROM semantic_composed_support_members m
             JOIN semantic_composed_support_groups g
               ON g.vault_id=m.vault_id AND g.support_group_id=m.support_group_id
             JOIN semantic_composed_card_items i
               ON i.vault_id=g.vault_id AND i.composed_card_item_id=g.composed_card_item_id
             WHERE i.vault_id=? AND i.composed_card_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(revision_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(
            |(source_id, observation_id, evidence_ref_id, source_revision_id)| {
                SemanticTargetSourceBinding {
                    source_id,
                    source_revision_id,
                    observation_id: Some(observation_id),
                    evidence_ref_id: Some(evidence_ref_id),
                }
            },
        )
        .collect::<Vec<_>>();
        self.finish_resolution(
            context,
            target_ref,
            "composed_card",
            target_key,
            scope_ref,
            None,
            Some(card_id),
            None,
            bindings,
        )
        .await
        .map(Some)
    }

    async fn resolve_evidence_target(
        &self,
        context: &VaultContext,
        target_ref: &str,
        id: &str,
        parent_ref: Option<&str>,
    ) -> Result<Option<SemanticTargetResolution>, StateError> {
        let mut parents = sqlx::query_as::<_, (String, String, String, String, String, Option<String>)>(
            "SELECT 'card',c.card_id,r.semantic_target_key,r.scope_ref,e.source_id,e.source_revision_id
             FROM semantic_evidence_refs e
             JOIN semantic_card_assertion_supports a ON a.vault_id=e.vault_id AND a.evidence_ref_id=e.evidence_ref_id
             JOIN semantic_card_revisions r ON r.vault_id=a.vault_id AND r.card_revision_id=a.card_revision_id
             JOIN semantic_memory_cards c ON c.vault_id=r.vault_id AND c.card_id=r.card_id
             JOIN semantic_sources s ON s.vault_id=e.vault_id AND s.source_id=e.source_id
             WHERE e.vault_id=? AND e.evidence_ref_id=? AND e.validation_status='validated'
               AND c.current_revision_id=r.card_revision_id AND c.eligibility='readable'
               AND r.publication_state='published' AND s.current_revision_id=e.source_revision_id
               AND s.eligible=1 AND s.pending_rebuild=0
             UNION
             SELECT 'composed_card',c.composed_card_id,r.semantic_target_key,r.scope_ref,e.source_id,e.source_revision_id
             FROM semantic_evidence_refs e
             JOIN semantic_composed_support_members m ON m.vault_id=e.vault_id AND m.evidence_ref_id=e.evidence_ref_id
             JOIN semantic_composed_support_groups g ON g.vault_id=m.vault_id AND g.support_group_id=m.support_group_id
             JOIN semantic_composed_card_items i ON i.vault_id=g.vault_id AND i.composed_card_item_id=g.composed_card_item_id
             JOIN semantic_composed_card_revisions r ON r.vault_id=i.vault_id AND r.composed_card_revision_id=i.composed_card_revision_id
             JOIN semantic_composed_cards c ON c.vault_id=r.vault_id AND c.composed_card_id=r.composed_card_id
             JOIN semantic_sources s ON s.vault_id=e.vault_id AND s.source_id=e.source_id
             WHERE e.vault_id=? AND e.evidence_ref_id=? AND e.validation_status='validated'
               AND c.current_revision_id=r.composed_card_revision_id AND c.eligibility='readable'
               AND r.publication_state='published' AND s.current_revision_id=e.source_revision_id
               AND s.eligible=1 AND s.pending_rebuild=0",
        )
        .bind(context.id().to_string())
        .bind(id)
        .bind(context.id().to_string())
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        if let Some(parent_ref) = parent_ref {
            parents.retain(|(kind, parent_id, _, _, _, _)| {
                format!("{kind}:{parent_id}") == parent_ref
            });
        }
        parents.sort_by(|left, right| left.1.cmp(&right.1).then(left.0.cmp(&right.0)));
        parents.dedup();
        if parents.len() > 1 {
            return Err(StateError::Conflict);
        }
        let Some((parent_kind, parent_id, target_key, scope_ref, source_id, source_revision_id)) =
            parents.pop()
        else {
            return Ok(None);
        };
        let card_id = (parent_kind == "card").then(|| parent_id.clone());
        let composed_card_id = (parent_kind == "composed_card").then(|| parent_id.clone());
        self.finish_resolution(
            context,
            target_ref,
            "evidence",
            target_key,
            scope_ref,
            card_id,
            composed_card_id,
            Some(id.to_owned()),
            vec![SemanticTargetSourceBinding {
                source_id,
                source_revision_id: source_revision_id.unwrap_or_default(),
                observation_id: None,
                evidence_ref_id: Some(id.to_owned()),
            }],
        )
        .await
        .map(Some)
    }

    #[allow(clippy::too_many_arguments)]
    async fn finish_resolution(
        &self,
        context: &VaultContext,
        target_ref: &str,
        _target_kind: &str,
        target_key: String,
        scope_ref: String,
        card_id: Option<String>,
        composed_card_id: Option<String>,
        evidence_ref_id: Option<String>,
        source_bindings: Vec<SemanticTargetSourceBinding>,
    ) -> Result<SemanticTargetResolution, StateError> {
        let target = self
            .target(context, &target_key)
            .await?
            .ok_or(StateError::IntegrityFailure)?;
        let parent_revision = if let Some(card_id) = card_id.as_deref() {
            sqlx::query_scalar::<_, i64>(
                "SELECT r.revision_number FROM semantic_memory_cards c JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id WHERE c.vault_id=? AND c.card_id=?",
            )
            .bind(context.id().to_string())
            .bind(card_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StateError::IntegrityFailure)?
        } else if let Some(card_id) = composed_card_id.as_deref() {
            sqlx::query_scalar::<_, i64>(
                "SELECT r.revision_number FROM semantic_composed_cards c JOIN semantic_composed_card_revisions r ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id WHERE c.vault_id=? AND c.composed_card_id=?",
            )
            .bind(context.id().to_string())
            .bind(card_id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(StateError::IntegrityFailure)?
        } else {
            0
        };
        Ok(SemanticTargetResolution {
            vault_id: context.id(),
            target_ref: target_ref.to_owned(),
            target_key,
            target_kind: target.target_kind,
            scope_ref,
            fingerprint_version: target.fingerprint_version,
            fingerprint: target.fingerprint,
            rules_revision: target.rules_revision,
            parent_revision,
            card_id,
            composed_card_id,
            evidence_ref_id,
            source_bindings,
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_rule(
    pool: &SqlitePool,
    context: &VaultContext,
    rule_kind: &str,
    action: &str,
    target_kind: &str,
    scope_ref: &str,
    fingerprint_version: i64,
    fingerprint: &str,
    payload: &Value,
    source_id: Option<&str>,
    source_revision_id: Option<&str>,
    observation_id: Option<&str>,
    card_id: Option<&str>,
    composed_card_id: Option<&str>,
    expected_rules_revision: Option<i64>,
    authorized_actor: &str,
    idempotency_key: &str,
) -> Result<SemanticRuleRecord, StateError> {
    if idempotency_key.is_empty()
        || scope_ref.is_empty()
        || fingerprint.is_empty()
        || authorized_actor.is_empty()
    {
        return Err(StateError::InvalidInput(
            "semantic rule identity is required",
        ));
    }
    if rule_kind == "correction" {
        let Some(object) = payload.as_object() else {
            return Err(StateError::InvalidInput(
                "correction payload must be an object",
            ));
        };
        let replace = object.get("replace");
        let remove = object.get("remove");
        let valid = object.len() == 2
            && replace
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            && remove
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
        if !valid {
            return Err(StateError::InvalidInput(
                "correction payload requires nonempty replace and remove only",
            ));
        }
        let Some((normalized_replace, normalized_remove)) = normalized_correction_lines(payload)
        else {
            return Err(StateError::InvalidInput(
                "correction lines must normalize to nonempty values",
            ));
        };
        if normalized_replace == normalized_remove {
            return Err(StateError::InvalidInput(
                "correction replacement and removal must differ",
            ));
        }
    }
    if rule_kind == "suppression"
        && !matches!(
            action,
            "suppress_read" | "suppress_regeneration" | "forget_current"
        )
    {
        return Err(StateError::InvalidInput("invalid suppression action"));
    }
    let request_hash = hash_json(&[
        rule_kind,
        action,
        target_kind,
        scope_ref,
        &fingerprint_version.to_string(),
        fingerprint,
        &payload.to_string(),
        source_id.unwrap_or_default(),
        source_revision_id.unwrap_or_default(),
        observation_id.unwrap_or_default(),
        card_id.unwrap_or_default(),
        composed_card_id.unwrap_or_default(),
    ]);
    let mut tx = pool.begin().await?;
    if let Some(existing) = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT request_hash,result_id,rules_revision FROM semantic_rule_idempotency
         WHERE vault_id=? AND rule_kind=? AND idempotency_key=?",
    )
    .bind(context.id().to_string())
    .bind(rule_kind)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if existing.0 != request_hash {
            return Err(StateError::Conflict);
        }
        let record = load_rule_tx(&mut tx, context, rule_kind, &existing.1).await?;
        tx.commit().await?;
        return Ok(record);
    }
    let target = ensure_target_tx(
        &mut tx,
        context,
        target_kind,
        scope_ref,
        fingerprint_version,
        fingerprint,
    )
    .await?;
    validate_rule_bindings(
        &mut tx,
        context,
        &target.target_key,
        target_kind,
        scope_ref,
        source_id,
        source_revision_id,
        observation_id,
        card_id,
        composed_card_id,
    )
    .await?;
    let current = current_rules_revision_tx(&mut tx, context).await?;
    if expected_rules_revision.is_some_and(|expected| expected != current) {
        return Err(StateError::Conflict);
    }
    let next = current.checked_add(1).ok_or(StateError::IntegrityFailure)?;
    let now = now_millis()?;
    sqlx::query("UPDATE semantic_rules_runtime SET rules_revision=?,updated_at=? WHERE vault_id=?")
        .bind(next)
        .bind(now)
        .bind(context.id().to_string())
        .execute(&mut *tx)
        .await?;
    let id = mcp_vault_domain::OperationId::new().to_string();
    let table = if rule_kind == "correction" {
        "semantic_corrections"
    } else {
        "semantic_suppressions"
    };
    let column = if rule_kind == "correction" {
        "correction_id"
    } else {
        "suppression_id"
    };
    let sql = format!(
        "INSERT INTO {table}
         (vault_id,{column},semantic_target_key,scope_ref,authorized_by,{} ,revision,active,created_at,updated_at,action,target_kind,source_id,source_revision_id,observation_id,card_id,composed_card_id,rules_revision,idempotency_key)
         VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        if rule_kind == "correction" { "correction_json" } else { "policy_json" }
    );
    sqlx::query(&sql)
        .bind(context.id().to_string())
        .bind(&id)
        .bind(&target.target_key)
        .bind(scope_ref)
        .bind(authorized_actor)
        .bind(payload.to_string())
        .bind(next)
        .bind(1_i64)
        .bind(now)
        .bind(now)
        .bind(action)
        .bind(target_kind)
        .bind(source_id)
        .bind(source_revision_id)
        .bind(observation_id)
        .bind(card_id)
        .bind(composed_card_id)
        .bind(next)
        .bind(idempotency_key)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO semantic_rule_idempotency
         (vault_id,rule_kind,idempotency_key,request_hash,result_id,rules_revision,created_at)
         VALUES(?,?,?,?,?,?,?)",
    )
    .bind(context.id().to_string())
    .bind(rule_kind)
    .bind(idempotency_key)
    .bind(request_hash)
    .bind(&id)
    .bind(next)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if rule_kind == "suppression"
        && source_id.is_none()
        && source_revision_id.is_none()
        && observation_id.is_none()
        && card_id.is_none()
        && composed_card_id.is_none()
        && matches!(action, "suppress_read" | "forget_current")
    {
        set_target_eligibility_tx(&mut tx, context, &target.target_key, false).await?;
    }
    tx.commit().await?;
    let mut tx = pool.begin().await?;
    let record = load_rule_tx(&mut tx, context, rule_kind, &id).await?;
    tx.commit().await?;
    Ok(record)
}

async fn revoke_rule(
    pool: &SqlitePool,
    context: &VaultContext,
    rule_kind: &str,
    id: &str,
    expected_revision: i64,
    idempotency_key: &str,
    authorized_actor: &str,
) -> Result<SemanticRuleRecord, StateError> {
    if idempotency_key.is_empty() || authorized_actor.is_empty() {
        return Err(StateError::InvalidInput(
            "semantic rule idempotency key is required",
        ));
    }
    let request_hash = hash_json(&[
        rule_kind,
        id,
        &expected_revision.to_string(),
        "revoke",
        authorized_actor,
    ]);
    let mut tx = pool.begin().await?;
    if let Some(existing) = sqlx::query_as::<_, (String, String)>(
        "SELECT request_hash,result_id FROM semantic_rule_idempotency
         WHERE vault_id=? AND rule_kind=? AND idempotency_key=?",
    )
    .bind(context.id().to_string())
    .bind(rule_kind)
    .bind(idempotency_key)
    .fetch_optional(&mut *tx)
    .await?
    {
        if existing.0 != request_hash {
            return Err(StateError::Conflict);
        }
        let record = load_rule_tx(&mut tx, context, rule_kind, &existing.1).await?;
        tx.commit().await?;
        return Ok(record);
    }
    let table = if rule_kind == "correction" {
        "semantic_corrections"
    } else {
        "semantic_suppressions"
    };
    let column = if rule_kind == "correction" {
        "correction_id"
    } else {
        "suppression_id"
    };
    let row = sqlx::query_as::<_, (String, String, i64, i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>)>(
        &format!("SELECT semantic_target_key,scope_ref,revision,active,source_id,source_revision_id,observation_id,card_id,composed_card_id FROM {table} WHERE vault_id=? AND {column}=?"),
    )
    .bind(context.id().to_string())
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StateError::Conflict)?;
    if row.2 != expected_revision || row.3 != 1 {
        return Err(StateError::Conflict);
    }
    let current = current_rules_revision_tx(&mut tx, context).await?;
    let next = current.checked_add(1).ok_or(StateError::IntegrityFailure)?;
    let now = now_millis()?;
    sqlx::query("UPDATE semantic_rules_runtime SET rules_revision=?,updated_at=? WHERE vault_id=?")
        .bind(next)
        .bind(now)
        .bind(context.id().to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query(&format!(
        "UPDATE {table} SET active=0,revision=?,rules_revision=?,updated_at=?,authorized_by=? WHERE vault_id=? AND {column}=?"
    ))
    .bind(next)
    .bind(next)
    .bind(now)
    .bind(authorized_actor)
    .bind(context.id().to_string())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO semantic_rule_idempotency
         (vault_id,rule_kind,idempotency_key,request_hash,result_id,rules_revision,created_at)
         VALUES(?,?,?,?,?,?,?)",
    )
    .bind(context.id().to_string())
    .bind(rule_kind)
    .bind(idempotency_key)
    .bind(request_hash)
    .bind(id)
    .bind(next)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    if rule_kind == "suppression"
        && row.4.is_none()
        && row.5.is_none()
        && row.6.is_none()
        && row.7.is_none()
        && row.8.is_none()
    {
        set_target_eligibility_tx(&mut tx, context, &row.0, true).await?;
    }
    tx.commit().await?;
    let mut tx = pool.begin().await?;
    let record = load_rule_tx(&mut tx, context, rule_kind, id).await?;
    tx.commit().await?;
    Ok(record)
}

async fn load_rule_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    rule_kind: &str,
    id: &str,
) -> Result<SemanticRuleRecord, StateError> {
    let table = if rule_kind == "correction" {
        "semantic_corrections"
    } else {
        "semantic_suppressions"
    };
    let column = if rule_kind == "correction" {
        "correction_id"
    } else {
        "suppression_id"
    };
    let row = sqlx::query_as::<_, (String, String, String, String, i64, i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64)>(
        &format!("SELECT {column},semantic_target_key,action,scope_ref,revision,active,source_id,source_revision_id,observation_id,card_id,composed_card_id,rules_revision FROM {table} WHERE vault_id=? AND {column}=?"),
    )
    .bind(context.id().to_string())
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(SemanticRuleRecord {
        vault_id: context.id(),
        rule_kind: rule_kind.to_owned(),
        id: row.0,
        target_key: row.1,
        action: row.2,
        scope_ref: row.3,
        revision: row.4,
        active: row.5 == 1,
        source_id: row.6,
        source_revision_id: row.7,
        observation_id: row.8,
        card_id: row.9,
        composed_card_id: row.10,
        rules_revision: row.11,
    })
}

pub(crate) async fn ensure_target_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    target_kind: &str,
    scope_ref: &str,
    fingerprint_version: i64,
    fingerprint: &str,
) -> Result<SemanticTargetRecord, StateError> {
    if target_kind.is_empty() || scope_ref.is_empty() || fingerprint.is_empty() {
        return Err(StateError::InvalidInput(
            "semantic target material is required",
        ));
    }
    if fingerprint_version < 1 {
        return Err(StateError::InvalidInput(
            "semantic target fingerprint version is invalid",
        ));
    }
    let target_key = derive_target_key(target_kind, scope_ref, fingerprint_version, fingerprint);
    let now = now_millis()?;
    sqlx::query(
        "INSERT INTO semantic_targets
         (vault_id,target_key,target_kind,scope_ref,fingerprint_version,fingerprint,created_at,updated_at)
         VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(vault_id,target_key) DO UPDATE SET updated_at=excluded.updated_at",
    )
    .bind(context.id().to_string())
    .bind(&target_key)
    .bind(target_kind)
    .bind(scope_ref)
    .bind(fingerprint_version)
    .bind(fingerprint)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(SemanticTargetRecord {
        vault_id: context.id(),
        target_key,
        target_kind: target_kind.to_owned(),
        scope_ref: scope_ref.to_owned(),
        fingerprint_version,
        fingerprint: fingerprint.to_owned(),
        rules_revision: current_rules_revision_tx(tx, context).await?,
    })
}

/// Check lifecycle rules against the *actual* inputs a generated object uses.
/// A target key alone is insufficient here: a source-bound rule must follow
/// the source/revision or observation support edge, while an unrelated M2
/// card remains publishable.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn target_blocks_generation(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    target_kind: &str,
    target_key: &str,
    scope_ref: &str,
    dependencies: &[(String, String)],
    observations: &[String],
    candidate_card_id: Option<&str>,
    candidate_composed_card_id: Option<&str>,
    candidate_material: &str,
) -> Result<bool, StateError> {
    let rows = sqlx::query_as::<_, (String, String, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, String)>(
        "SELECT 'correction',correction_json,action,semantic_target_key,source_id,source_revision_id,observation_id,card_id,composed_card_id,scope_ref
         FROM semantic_corrections WHERE vault_id=? AND active=1
         UNION ALL
         SELECT 'suppression',policy_json,action,semantic_target_key,source_id,source_revision_id,observation_id,card_id,composed_card_id,scope_ref
         FROM semantic_suppressions WHERE vault_id=? AND active=1",
    )
    .bind(context.id().to_string())
    .bind(context.id().to_string())
    .fetch_all(&mut **tx)
    .await?;
    for (
        kind,
        payload,
        action,
        rule_target,
        source_id,
        source_revision_id,
        observation_id,
        card_id,
        composed_card_id,
        rule_scope,
    ) in rows
    {
        if rule_scope != scope_ref {
            continue;
        }
        let target_match = rule_target == target_key;
        let observation_match = observation_id
            .as_deref()
            .is_some_and(|id| observations.iter().any(|candidate| candidate == id));
        let source_match = source_id.as_deref().is_some_and(|source| {
            dependencies
                .iter()
                .any(|(dependency_source, dependency_revision)| {
                    source == dependency_source
                        && source_revision_id
                            .as_deref()
                            .is_none_or(|revision| revision == dependency_revision)
                })
        });
        // A card binding is meaningful only when the target itself is the
        // bound object. It must not turn a single-source rule into a blanket
        // veto for every composed card in the Vault.
        let card_binding_match = card_id
            .as_deref()
            .is_none_or(|id| candidate_card_id == Some(id));
        let composed_binding_match = composed_card_id
            .as_deref()
            .is_none_or(|id| candidate_composed_card_id == Some(id));
        let bound = if observation_id.is_some() {
            observation_match && source_match && card_binding_match && composed_binding_match
        } else if card_id.is_some() || composed_card_id.is_some() {
            // A card-specific rule remains attached to that durable card
            // across revisions, even if its regenerated semantic fingerprint
            // changes. Requiring target_match here would let regeneration
            // evade the exact card binding.
            card_binding_match && composed_binding_match && source_id.is_none_or(|_| source_match)
        } else {
            // Once a source/revision binding is present, it is precise. A
            // matching target shared by another source must not make that
            // unrelated card fail closed.
            if source_id.is_some() || source_revision_id.is_some() {
                source_match
            } else {
                target_match
            }
        };
        if !bound {
            continue;
        }
        if kind == "suppression" {
            if matches!(action.as_str(), "suppress_read" | "forget_current")
                && target_kind == "memory_card"
                && (source_match || observation_match)
            {
                // A source-scoped read suppression is also an admission
                // fence for a fresh M1 card: its observation cannot become a
                // new support edge while the rule is active.
                return Ok(true);
            }
            if !matches!(action.as_str(), "suppress_regeneration" | "forget_current") {
                continue;
            }
            return Ok(true);
        }
        // A correction is a constrained replacement, not a permanent veto.
        // The deterministic path accepts a candidate whose canonical output
        // has an exact line equal to the persisted replacement; other shapes remain blocked
        // and are recorded by the caller's terminal safe error.
        let matches = serde_json::from_str::<Value>(&payload)
            .ok()
            .is_some_and(|value| correction_matches_material(&value, candidate_material));
        if !matches {
            return Ok(true);
        }
    }
    Ok(false)
}

fn normalize_markdown_line(line: &str) -> String {
    use unicode_normalization::UnicodeNormalization;

    let value = line.nfkc().collect::<String>();
    let mut value = value.trim();
    loop {
        let next = value
            .strip_prefix("- ")
            .or_else(|| value.strip_prefix("* "))
            .or_else(|| value.strip_prefix("+ "))
            .or_else(|| value.strip_prefix("> "))
            .or_else(|| {
                value
                    .find(['.', ')'])
                    .filter(|index| {
                        *index > 0
                            && value[..*index]
                                .chars()
                                .all(|character| character.is_ascii_digit())
                    })
                    .and_then(|index| value.get(index + 1..))
            })
            .or_else(|| value.strip_prefix("-"))
            .or_else(|| value.strip_prefix("*"))
            .or_else(|| value.strip_prefix("+"))
            .or_else(|| value.strip_prefix(">"));
        let Some(next) = next else {
            break;
        };
        value = next.trim();
    }
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_correction_lines(payload: &Value) -> Option<(String, String)> {
    let replacement = normalize_markdown_line(payload.get("replace")?.as_str()?);
    let removal = normalize_markdown_line(payload.get("remove")?.as_str()?);
    (!replacement.is_empty() && !removal.is_empty()).then_some((replacement, removal))
}

fn correction_matches_material(payload: &Value, candidate_material: &str) -> bool {
    let Some((replacement, removal)) = normalized_correction_lines(payload) else {
        return false;
    };
    let replacement_count = candidate_material
        .lines()
        .filter(|line| normalize_markdown_line(line) == replacement)
        .count();
    let removal_count = candidate_material
        .lines()
        .filter(|line| normalize_markdown_line(line) == removal)
        .count();
    replacement_count == 1 && removal_count == 0 && replacement != removal
}

pub(crate) fn derive_target_key(
    target_kind: &str,
    scope_ref: &str,
    fingerprint_version: i64,
    fingerprint: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update("semantic-target-v1|");
    hasher.update(target_kind);
    hasher.update("|");
    hasher.update(scope_ref);
    hasher.update("|");
    hasher.update(fingerprint_version.to_string());
    hasher.update("|");
    hasher.update(fingerprint);
    format!("st_{}", hex_digest(&hasher.finalize()))
}

async fn current_rules_revision_pool(
    pool: &SqlitePool,
    context: &VaultContext,
) -> Result<i64, StateError> {
    let mut tx = pool.begin().await?;
    let value = current_rules_revision_tx(&mut tx, context).await?;
    tx.commit().await?;
    Ok(value)
}

pub(crate) async fn current_rules_revision_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
) -> Result<i64, StateError> {
    let now = now_millis()?;
    sqlx::query(
        "INSERT INTO semantic_rules_runtime(vault_id,rules_revision,updated_at)
         VALUES(?,0,?) ON CONFLICT(vault_id) DO NOTHING",
    )
    .bind(context.id().to_string())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT rules_revision FROM semantic_rules_runtime WHERE vault_id=?")
            .bind(context.id().to_string())
            .fetch_one(&mut **tx)
            .await?,
    )
}

async fn ensure_current_evidence(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    observation_id: &str,
    source_id: &str,
    source_revision_id: &str,
) -> Result<(), StateError> {
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM semantic_observations o
         JOIN semantic_sources s ON s.vault_id=o.vault_id AND s.source_id=o.source_id
         JOIN semantic_observation_evidence oe ON oe.vault_id=o.vault_id AND oe.observation_id=o.observation_id AND oe.source_id=o.source_id
         JOIN semantic_evidence_refs e ON e.vault_id=oe.vault_id AND e.evidence_ref_id=oe.evidence_ref_id AND e.source_id=oe.source_id
         JOIN semantic_extraction_sets es
           ON es.vault_id=o.vault_id AND es.extraction_set_id=o.extraction_set_id
          AND es.source_id=o.source_id AND es.source_revision_id=o.source_revision_id
         WHERE o.vault_id=? AND o.observation_id=? AND o.source_id=? AND o.source_revision_id=?
           AND s.current_revision_id=? AND s.eligible=1 AND s.pending_rebuild=0
           AND es.state='success_nonempty'
           AND es.extraction_commit_sequence=s.extraction_commit_sequence
           AND e.validation_status='validated'",
    )
    .bind(context.id().to_string())
    .bind(observation_id)
    .bind(source_id)
    .bind(source_revision_id)
    .bind(source_revision_id)
    .fetch_optional(&mut **tx)
    .await?;
    if exists != Some(1) {
        return Err(StateError::Conflict);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn validate_rule_bindings(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    target_key: &str,
    target_kind: &str,
    scope_ref: &str,
    source_id: Option<&str>,
    source_revision_id: Option<&str>,
    observation_id: Option<&str>,
    card_id: Option<&str>,
    composed_card_id: Option<&str>,
) -> Result<(), StateError> {
    if source_revision_id.is_some() && source_id.is_none() {
        return Err(StateError::InvalidInput(
            "source revision binding requires source binding",
        ));
    }
    if let Some(source_id) = source_id {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM semantic_sources WHERE vault_id=? AND source_id=?")
                .bind(context.id().to_string())
                .bind(source_id)
                .fetch_optional(&mut **tx)
                .await?;
        if exists != Some(1) {
            return Err(StateError::Conflict);
        }
    }
    if let (Some(source_id), Some(revision_id)) = (source_id, source_revision_id) {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM semantic_source_revisions
             WHERE vault_id=? AND source_id=? AND source_revision_id=?",
        )
        .bind(context.id().to_string())
        .bind(source_id)
        .bind(revision_id)
        .fetch_optional(&mut **tx)
        .await?;
        if exists != Some(1) {
            return Err(StateError::Conflict);
        }
    }
    if let Some(observation_id) = observation_id {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT source_id,source_revision_id FROM semantic_observations
             WHERE vault_id=? AND observation_id=?",
        )
        .bind(context.id().to_string())
        .bind(observation_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((observation_source, observation_revision)) = row else {
            return Err(StateError::Conflict);
        };
        if source_id != Some(observation_source.as_str())
            || source_revision_id != Some(observation_revision.as_str())
        {
            return Err(StateError::Conflict);
        }
    }
    if let Some(card_id) = card_id {
        let row: Option<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT c.card_id,c.source_id,r.source_revision_id FROM semantic_memory_cards c
             LEFT JOIN semantic_card_revisions r
               ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.card_id=?",
        )
        .bind(context.id().to_string())
        .bind(card_id)
        .fetch_optional(&mut **tx)
        .await?;
        let Some((bound_card, bound_source, bound_revision)) = row else {
            return Err(StateError::Conflict);
        };
        if target_kind != "memory_card"
            || bound_card != card_id
            || source_id.is_some_and(|value| value != bound_source)
            || source_revision_id.is_some_and(|value| Some(value) != bound_revision.as_deref())
        {
            return Err(StateError::Conflict);
        }
        let target: Option<(String, String, String)> = sqlx::query_as(
            "SELECT r.semantic_target_key,r.scope_ref,r.assertion_status
             FROM semantic_memory_cards c
             JOIN semantic_card_revisions r ON r.vault_id=c.vault_id AND r.card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.card_id=?",
        )
        .bind(context.id().to_string())
        .bind(card_id)
        .fetch_optional(&mut **tx)
        .await?;
        if target
            .as_ref()
            .is_none_or(|(key, card_scope, _)| key != target_key || card_scope != scope_ref)
        {
            return Err(StateError::Conflict);
        }
        if let Some(observation_id) = observation_id {
            let belongs: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM semantic_card_items i
                 JOIN semantic_memory_cards c
                   ON c.vault_id=i.vault_id AND c.card_id=i.card_id
                 JOIN semantic_card_revisions r
                   ON r.vault_id=i.vault_id AND r.card_revision_id=i.card_revision_id
                 WHERE i.vault_id=? AND c.card_id=? AND c.current_revision_id=r.card_revision_id
                   AND r.publication_state='published' AND i.observation_id=?
                   AND i.source_id=? AND r.source_revision_id=?
                 LIMIT 1",
            )
            .bind(context.id().to_string())
            .bind(card_id)
            .bind(observation_id)
            .bind(source_id)
            .bind(source_revision_id)
            .fetch_optional(&mut **tx)
            .await?;
            if belongs != Some(1) {
                return Err(StateError::Conflict);
            }
        }
    }
    if let Some(card_id) = composed_card_id {
        let exists: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM semantic_composed_cards c
             JOIN semantic_composed_card_revisions r
               ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
             LEFT JOIN semantic_composed_card_dependencies d
               ON d.vault_id=r.vault_id AND d.composed_card_revision_id=r.composed_card_revision_id
             WHERE c.vault_id=? AND c.composed_card_id=? AND ?='composed_card'
               AND (? IS NULL OR (d.source_id=? AND (? IS NULL OR d.source_revision_id=?)))",
        )
        .bind(context.id().to_string())
        .bind(card_id)
        .bind(target_kind)
        .bind(source_id)
        .bind(source_id)
        .bind(source_revision_id)
        .bind(source_revision_id)
        .fetch_optional(&mut **tx)
        .await?;
        if exists != Some(1) {
            return Err(StateError::Conflict);
        }
        let target: Option<(String, String)> = sqlx::query_as(
            "SELECT r.semantic_target_key,r.scope_ref
             FROM semantic_composed_cards c
             JOIN semantic_composed_card_revisions r ON r.vault_id=c.vault_id AND r.composed_card_revision_id=c.current_revision_id
             WHERE c.vault_id=? AND c.composed_card_id=?",
        )
        .bind(context.id().to_string())
        .bind(card_id)
        .fetch_optional(&mut **tx)
        .await?;
        if target
            .as_ref()
            .is_none_or(|(key, card_scope)| key != target_key || card_scope != scope_ref)
        {
            return Err(StateError::Conflict);
        }
        if let Some(observation_id) = observation_id {
            let belongs: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM semantic_composed_support_members m
                 JOIN semantic_composed_support_groups g
                   ON g.vault_id=m.vault_id AND g.support_group_id=m.support_group_id
                 JOIN semantic_composed_card_items i
                   ON i.vault_id=g.vault_id AND i.composed_card_item_id=g.composed_card_item_id
                 JOIN semantic_composed_card_revisions r
                   ON r.vault_id=i.vault_id AND r.composed_card_revision_id=i.composed_card_revision_id
                 JOIN semantic_composed_cards c
                   ON c.vault_id=r.vault_id AND c.composed_card_id=r.composed_card_id
                  AND c.current_revision_id=r.composed_card_revision_id
                 WHERE c.vault_id=? AND c.composed_card_id=?
                   AND m.observation_id=? AND m.source_id=? AND m.source_revision_id=?
                 LIMIT 1",
            )
            .bind(context.id().to_string())
            .bind(card_id)
            .bind(observation_id)
            .bind(source_id)
            .bind(source_revision_id)
            .fetch_optional(&mut **tx)
            .await?;
            if belongs != Some(1) {
                return Err(StateError::Conflict);
            }
        }
    }
    Ok(())
}

async fn set_target_eligibility_tx(
    tx: &mut Transaction<'_, Sqlite>,
    context: &VaultContext,
    target_key: &str,
    restore: bool,
) -> Result<(), StateError> {
    let suppressed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM semantic_suppressions
         WHERE vault_id=? AND semantic_target_key=? AND active=1
           AND action IN ('suppress_read','forget_current')",
    )
    .bind(context.id().to_string())
    .bind(target_key)
    .fetch_one(&mut **tx)
    .await?;
    if !restore || suppressed != 0 {
        sqlx::query(
            "UPDATE semantic_memory_cards SET eligibility='invalidated',updated_at=?
             WHERE vault_id=? AND current_revision_id IN (
               SELECT card_revision_id FROM semantic_card_revisions
               WHERE vault_id=? AND semantic_target_key=?
             )",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(target_key)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_composed_cards SET eligibility='invalidated',updated_at=?
             WHERE vault_id=? AND current_revision_id IN (
               SELECT composed_card_revision_id FROM semantic_composed_card_revisions
               WHERE vault_id=? AND semantic_target_key=?
             )",
        )
        .bind(now_millis()?)
        .bind(context.id().to_string())
        .bind(context.id().to_string())
        .bind(target_key)
        .execute(&mut **tx)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE semantic_memory_cards SET eligibility='readable',updated_at=?
         WHERE vault_id=? AND current_revision_id IN (
           SELECT r.card_revision_id FROM semantic_card_revisions r
           JOIN semantic_sources s ON s.vault_id=r.vault_id AND s.source_id=r.source_id
           WHERE r.vault_id=? AND r.semantic_target_key=? AND r.publication_state='published'
             AND s.eligible=1 AND s.pending_rebuild=0
         )",
    )
    .bind(now_millis()?)
    .bind(context.id().to_string())
    .bind(context.id().to_string())
    .bind(target_key)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE semantic_composed_cards SET eligibility='readable',updated_at=?
         WHERE vault_id=? AND current_revision_id IN (
           SELECT composed_card_revision_id FROM semantic_composed_card_revisions
           WHERE vault_id=? AND semantic_target_key=? AND publication_state='published'
         )",
    )
    .bind(now_millis()?)
    .bind(context.id().to_string())
    .bind(context.id().to_string())
    .bind(target_key)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn hash_json(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hex_digest(&hasher.finalize())
}

fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{correction_matches_material, normalize_markdown_line};

    #[test]
    fn markdown_line_normalization_handles_common_markdown_prefixes() {
        assert_eq!(
            normalize_markdown_line("  1.  Keep\u{00a0}the boundary  "),
            "Keep the boundary"
        );
        assert_eq!(
            normalize_markdown_line("> - Keep the boundary"),
            "Keep the boundary"
        );
    }

    #[test]
    fn correction_matching_normalizes_quote_bullet_nfkc_and_rejects_old_and_new() {
        let correction = json!({"replace":"- ｎｅｗ", "remove":"> old"});
        assert!(correction_matches_material(&correction, "- new\n"));
        assert!(!correction_matches_material(&correction, "> old\n- new\n"));
        assert!(!correction_matches_material(&correction, "- old\n- new\n"));
        assert!(!correction_matches_material(&correction, "- new\n- new\n"));
        assert_eq!(normalize_markdown_line("- old"), "old");
        assert_eq!(normalize_markdown_line("> old"), "old");
    }
}
