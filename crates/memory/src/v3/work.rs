//! Job admission and pause control for source selection and derived navigation.
use super::{EXTRACTION_PIPELINE_VERSION, MEMORY_CONTRACT_GENERATION, MemoryService};
use crate::MemoryError;
use mcp_vault_domain::{EventId, VaultContext};
use serde_json::{Value, json};

impl MemoryService {
    pub async fn ensure_initialized(&self, context: &VaultContext) -> Result<(), MemoryError> {
        if self
            .state
            .memory_units()
            .initialization_required(context)
            .await?
        {
            return Err(MemoryError::Configuration("memory_initialization_required"));
        }
        Ok(())
    }
    pub async fn generation_status(&self, context: &VaultContext) -> Result<Value, MemoryError> {
        Ok(
            json!({"initialization_required":self.state.memory_units().initialization_required(context).await?,"runtime":self.state.memory_units().runtime(context).await?,"source_progress":self.state.memory_units().selection_progress(context).await?,"counts":self.state.memory_units().counts(context).await?,"extraction_job":self.state.jobs().find_active_by_type(context,"memory.extract").await?.map(job_status),"overview_job":self.state.jobs().find_active_by_type(context,"memory.overview").await?.map(job_status)}),
        )
    }
    pub async fn control_generation(
        &self,
        context: &VaultContext,
        action: &str,
    ) -> Result<Value, MemoryError> {
        self.ensure_initialized(context).await?;
        match action {
            "pause" => self.state.memory_units().set_paused(context, true).await?,
            "resume" | "run" => {
                self.state.memory_units().set_paused(context, false).await?;
                self.state
                    .jobs()
                    .expedite_active(context, "memory.extract")
                    .await?;
                self.state
                    .jobs()
                    .expedite_active(context, "memory.overview")
                    .await?;
                if self
                    .state
                    .jobs()
                    .find_active_by_type(context, "memory.overview")
                    .await?
                    .is_none()
                    && let Some(failed) = self
                        .state
                        .jobs()
                        .list(
                            context,
                            Some(mcp_vault_state::JobStatus::Failed),
                            Some("memory.overview"),
                            1,
                            0,
                        )
                        .await?
                        .first()
                {
                    self.state.jobs().request_retry(context, failed.id).await?;
                }
                if self.extraction_policy(context).await?.policy.enabled {
                    self.state.jobs().enqueue_singleton(context,"memory.extract",&format!("vault:{}:source-unit-backfill:{}",context.id(),EventId::new()),&json!({"memory_contract_generation":MEMORY_CONTRACT_GENERATION,"pipeline_version":EXTRACTION_PIPELINE_VERSION,"scope":"all","reason":"generation_control","include_evaluated":false}),4,5,0).await?;
                }
                self.ensure_memory_jobs_scheduled(context).await?;
            }
            _ => {
                return Err(MemoryError::InvalidInput(
                    "memory generation action is invalid",
                ));
            }
        }
        self.generation_status(context).await
    }
    /// Compensation admits only stale navigation, never repeats unchanged model
    /// calls. Source file events and explicit backfill own extraction admission.
    pub async fn ensure_memory_jobs_scheduled(
        &self,
        context: &VaultContext,
    ) -> Result<(), MemoryError> {
        if self
            .state
            .memory_units()
            .initialization_required(context)
            .await?
        {
            return Ok(());
        }
        if self
            .state
            .jobs()
            .find_active_by_type(context, "memory.extract")
            .await?
            .is_some()
        {
            return Ok(());
        }
        let runtime = self.state.memory_units().runtime(context).await?;
        if runtime.paused || self.state.memory_units().counts(context).await?.total == 0 {
            return Ok(());
        }
        let Some(signature) = self.overview_signature(context).await? else {
            return Ok(());
        };
        let key = format!(
            "vault:{}:memory-overview:{}:{}",
            context.id(),
            runtime.generation,
            signature
        );
        self.state.jobs().enqueue_singleton(context,"memory.overview",&key,&json!({"memory_contract_generation":MEMORY_CONTRACT_GENERATION,"generation":runtime.generation,"model_signature":signature}),1,5,0).await?;
        Ok(())
    }
}

fn job_status(job: mcp_vault_state::JobRecord) -> Value {
    json!({"id":job.id,"status":job.status.as_str(),"progress":job.progress,"error_code":job.last_error})
}
