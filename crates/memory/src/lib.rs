//! Transparent, sourced, Vault-scoped durable memory services.

mod error;
mod markdown;
mod model;
mod service;

pub use error::MemoryError;
pub use model::{
    ExtractedCandidate, MemoryOrigin, MemoryRelationView, MemorySourceInput, MemorySourceView,
    MemoryStatus, MemoryType, MemoryUpdateInput, MemoryView, RecallContext, RecallRequest,
    RecallResult, RememberInput, RememberResult,
};
pub use service::{MemoryRebuildReport, MemoryService};
