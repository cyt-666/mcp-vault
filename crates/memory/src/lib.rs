//! Transparent, sourced, Vault-scoped durable memory services.

mod error;
mod markdown;
mod model;
mod service;

pub use error::MemoryError;
pub use model::{
    ExtractionPolicy, ExtractionPolicyState, ExtractionReadiness, ExtractionSourceMode,
    MemoryConsolidationReport, MemoryOrigin, MemoryPipelineResetReport, MemoryRelationView,
    MemorySourceAuditPage, MemorySourceInput, MemorySourceReconcileReport,
    MemorySourceRepairReport, MemorySourceView, MemoryStatus, MemoryType, MemoryUpdateInput,
    MemoryView, NoteExtractionOptions, NoteExtractionResult, PipelineRegenerationAdmission,
    RecallContext, RecallRequest, RecallResult, RelatedNoteView, RememberInput, RememberResult,
};
pub use service::{
    EXTRACTION_PIPELINE_VERSION, MEMORY_PIPELINE_GENERATION, MemoryRebuildReport, MemoryService,
};
