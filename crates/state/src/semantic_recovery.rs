//! Cross-type semantic recovery fences.
//!
//! This lives at the State boundary so maintenance services such as backup
//! restore can apply the same source/rules preflight without depending on the
//! Memory crate or reaching into SQL repositories directly.

use mcp_vault_domain::VaultContext;

use crate::{StateError, StateStore};

impl StateStore {
    /// Validate every pending M1/M2 recovery barrier in one Vault before a
    /// caller enters generic Vault Core recovery. Stale work is terminalized
    /// as blocked while any associated Core journal remains discoverable.
    pub async fn preflight_semantic_recovery(
        &self,
        context: &VaultContext,
    ) -> Result<(), StateError> {
        let memory = self.semantic_memory();
        let pending_extractions = memory.pending_extractions(context).await?;
        let rules_revision = self
            .semantic_rules()
            .current_rules_revision(context)
            .await?;
        for extraction_id in pending_extractions {
            let snapshots = memory.pending_snapshots(context, extraction_id).await?;
            if snapshots
                .iter()
                .any(|snapshot| snapshot.rules_revision != rules_revision)
            {
                memory
                    .reject_publication(context, extraction_id, "semantic_rules_changed")
                    .await?;
                return Err(StateError::Conflict);
            }
            for snapshot in snapshots {
                let Some(source) = memory.get_source(context, snapshot.source_id).await? else {
                    memory
                        .reject_publication(
                            context,
                            extraction_id,
                            "semantic_source_fence_conflict",
                        )
                        .await?;
                    return Err(StateError::Conflict);
                };
                if source.current_revision_id != Some(snapshot.source_revision_id)
                    || source.content_hash != snapshot.source_content_hash
                    || source.authorization_revision != snapshot.authorization_revision
                    || source.source_generation != snapshot.source_generation
                    || source.extraction_commit_sequence != snapshot.extraction_commit_sequence
                {
                    memory
                        .reject_publication(
                            context,
                            extraction_id,
                            "semantic_source_fence_conflict",
                        )
                        .await?;
                    return Err(StateError::Conflict);
                }
            }
        }

        let organization = self.semantic_organization();
        let pending_jobs = organization.pending_jobs(context).await?;
        for job_id in pending_jobs {
            let Some(job) = organization.get_job(context, job_id).await? else {
                return Err(StateError::Conflict);
            };
            if job.rules_revision != rules_revision {
                organization
                    .mark_job_failed(context, job_id, "semantic_rules_changed", true)
                    .await?;
                return Err(StateError::Conflict);
            }
            if let Err(error) = organization.validate_job_write_fence(context, job_id).await {
                if !matches!(error, StateError::Conflict) {
                    return Err(error);
                }
                organization
                    .mark_job_failed(context, job_id, "semantic_source_fence_conflict", true)
                    .await?;
                return Err(StateError::Conflict);
            }
        }
        Ok(())
    }
}
