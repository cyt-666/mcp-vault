//! Transparent, sourced, Vault-scoped durable memory services.

mod current_markdown;
mod error;
mod markdown;
mod model;
mod service;

pub use error::MemoryError;
pub use model::{
    CurrentSourceReconcileReport, ExtractionPolicy, ExtractionPolicyState, ExtractionReadiness,
    ExtractionSourceMode, ForgetResult, MemoryEmbeddingScheduleReport, MemoryEmbeddingStatusView,
    MemoryOrigin, MemoryOwnership, MemorySemanticCalibration, MemorySemanticCalibrationView,
    MemorySourceInput, MemorySourceView, MemoryType, MemoryUpdateInput, MemoryV2MigrationResult,
    MemoryView, NoteExtractionOptions, NoteExtractionResult, RecallContext, RecallRequest,
    RecallResult, RelatedNoteView, RememberInput, RememberResult,
};
pub use service::{
    EXTRACTION_PIPELINE_VERSION, MEMORY_CONTRACT_GENERATION, MemoryRebuildReport, MemoryService,
};
