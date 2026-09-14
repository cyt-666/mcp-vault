//! Source-preserving memory implementation.
mod canonical;
mod model;
#[allow(dead_code)]
mod review;
mod selection;
mod service;
pub use model::*;
pub use service::{
    EXTRACTION_PIPELINE_VERSION, MEMORY_CONTRACT_GENERATION, MemoryRebuildReport, MemoryService,
};

mod overview;
pub use overview::{MemoryOverview, OverviewEntry, OverviewRequest, OverviewSection};

mod work;

mod initialization;
pub use initialization::{
    InitializationStart, MemoryInitializationReport, MemoryInitializationService,
    initialize_vault_memories, initialize_vault_memories_with_lease, preview_memory_initialization,
    preview_memory_initialization_for_admin,
};
