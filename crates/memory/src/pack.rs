//! Deterministic, internal task MemoryPack construction.
//!
//! This module intentionally consumes only State's currently qualified M1/M2
//! projections. It never reads ordinary note bodies, calls a Provider, or
//! creates durable state.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
};

use mcp_vault_core::VaultCore;
use mcp_vault_domain::{FileId, VaultContext, VaultPath};
use mcp_vault_indexer::IndexService;
use mcp_vault_state::{ComposedCardRecord, SemanticCardRecord, SemanticRelationRecord, StateStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::MemoryError;

const CANDIDATE_LIMIT: u32 = 200;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryPackMode {
    #[default]
    Current,
    History,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryPackRequest {
    /// Natural-language task.
    pub task: String,
    /// Optional normalized query.
    pub query: Option<String>,
    /// Explicit source set required by the internal A/B/C comparison runner.
    /// Ordinary MemoryPack builds leave this empty.
    pub comparison_source_paths: Vec<String>,
    /// Optional allow-list for an isolated caller. When present every source
    /// reference of every returned M1/M2 candidate must match all populated
    /// dimensions. This is enforced here, at the memory boundary, rather than
    /// by an evaluation adapter that only annotates its request.
    #[serde(default)]
    pub source_scope: Option<MemoryPackSourceScope>,
    /// Exact source path filter.
    pub exact_source_path: Option<String>,
    /// VaultPath segment prefix filter.
    pub path_prefix: Option<String>,
    /// Semantic scope filter.
    pub scope_ref: Option<String>,
    /// Semantic kind filters.
    pub kinds: Vec<String>,
    /// Explicit version filter.
    pub version: Option<String>,
    /// Explicit as-of filter.
    pub as_of: Option<String>,
    /// Explicit subject terms.
    pub subject_terms: Vec<String>,
    /// Include M1 cards.
    pub include_m1: bool,
    /// Include M2 composed cards.
    pub include_m2: bool,
    /// Current or history mode.
    pub mode: MemoryPackMode,
    /// Maximum returned entries.
    #[schemars(range(min = 1, max = 200))]
    pub max_entries: u32,
    /// Maximum serialized bytes.
    #[schemars(range(min = 1))]
    pub max_bytes: u32,
    /// Maximum estimated tokens.
    #[schemars(range(min = 1))]
    pub max_tokens: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryPackSourceScope {
    /// Stable semantic source IDs permitted to appear in the pack.
    #[serde(default)]
    pub source_ids: Vec<String>,
    /// Canonical source paths permitted to appear in the pack.
    #[serde(default)]
    pub source_paths: Vec<String>,
    /// Stable source-revision IDs permitted to appear in the pack.
    #[serde(default)]
    pub source_revision_ids: Vec<String>,
}

impl Default for MemoryPackRequest {
    fn default() -> Self {
        Self {
            task: String::new(),
            query: None,
            comparison_source_paths: Vec::new(),
            source_scope: None,
            exact_source_path: None,
            path_prefix: None,
            scope_ref: None,
            kinds: Vec::new(),
            version: None,
            as_of: None,
            subject_terms: Vec::new(),
            include_m1: true,
            include_m2: true,
            mode: MemoryPackMode::Current,
            max_entries: 20,
            max_bytes: 32_000,
            max_tokens: 8_000,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPack {
    pub current_context: Vec<MemoryPackEntry>,
    pub relevant_experiences: Vec<MemoryPackEntry>,
    pub conflicts_or_checks: Vec<MemoryPackConflict>,
    pub evidence_gaps: Vec<MemoryPackGap>,
    pub related_sources: Vec<MemoryPackSource>,
    pub diagnostics: Vec<String>,
    pub estimated_tokens: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPackEntry {
    pub id: String,
    pub origin: String,
    pub title: String,
    pub kind: String,
    pub scope_ref: String,
    pub assertion_status: String,
    pub temporal_scope: Value,
    pub core_assertions: Vec<String>,
    pub conditions: Vec<String>,
    pub exceptions: Vec<String>,
    pub ordered_steps: Vec<String>,
    pub unresolved_items: Vec<String>,
    pub optional_details: Vec<String>,
    pub source_references: Vec<MemoryPackSourceReference>,
    pub evidence_refs: Vec<String>,
    #[serde(skip)]
    candidate_keys: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPackSourceReference {
    pub source_id: String,
    pub source_revision_id: String,
    pub source_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPackSource {
    pub source_id: String,
    pub source_revision_id: String,
    pub source_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPackConflict {
    pub relation_id: String,
    pub relation_kind: String,
    pub reason_code: String,
    pub source_ids: Vec<String>,
    pub left_observation_id: String,
    pub right_observation_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryPackGap {
    pub code: String,
    pub pointer: Option<String>,
}

#[derive(Clone)]
pub struct MemoryPackService {
    state: StateStore,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryPackComparisonRecord {
    pub mode: String,
    pub input_hash: String,
    pub manifest_hash: String,
    pub manifest_count: u32,
    pub candidate_count: u32,
    pub eligible_count: u32,
    pub relevant_count: u32,
    pub returned_count: u32,
    pub estimated_bytes: u32,
    pub estimated_tokens: u32,
    pub gap_codes: Vec<String>,
    pub degraded: bool,
    pub pending_real_evaluation: bool,
}

#[derive(Serialize)]
struct ComparisonArmEnvelope {
    mode: String,
    entries: Value,
    source_manifest: Vec<MemoryPackManifestEntry>,
    gaps: Vec<String>,
    diagnostics: Vec<String>,
    relations: Vec<MemoryPackConflict>,
    degraded: bool,
}

/// Canonical immutable inputs shared by the local A/B/C engineering runner.
/// Vault identity comes from the bound context, never from the request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryPackInput {
    pub vault_id: String,
    pub request: MemoryPackRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryPackManifestEntry {
    pub path: String,
    pub file_id: String,
    pub source_id: Option<String>,
    pub source_revision_id: Option<String>,
    pub revision: u64,
    pub content_hash: Option<String>,
}

#[derive(Clone)]
enum Candidate {
    M1(SemanticCardRecord),
    M2(ComposedCardRecord),
}

impl MemoryPackService {
    pub fn new(state: StateStore) -> Self {
        Self { state }
    }

    /// Local A/B/C engineering replay. A uses the real IndexService lexical
    /// path; B/C use the same pack request and budget. It is not a semantic
    /// quality or cost evaluation.
    pub async fn compare_abc(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        index: &IndexService,
        request: &MemoryPackRequest,
    ) -> Result<Vec<MemoryPackComparisonRecord>, MemoryError> {
        let effective_query = mcp_vault_state::memory_search_terms(
            [
                request.task.as_str(),
                request.query.as_deref().unwrap_or_default(),
            ],
            64,
        );
        let canonical_request = MemoryPackRequest {
            task: effective_query.clone(),
            query: Some(effective_query.clone()),
            ..request.clone()
        };
        let input = MemoryPackInput {
            vault_id: context.id().to_string(),
            request: canonical_request.clone(),
        };
        let input_hash = hash_bytes(
            serde_json::to_vec(&input)
                .map_err(|_| MemoryError::InvalidInput("memory pack comparison input invalid"))?
                .as_slice(),
        );
        if request.comparison_source_paths.is_empty() {
            return Ok(comparison_gap_records(
                &input_hash,
                "comparison_source_manifest_required",
            ));
        }
        let lexical_query = effective_query.as_str();
        let lexical_raw = index
            .search_notes(
                context,
                lexical_query,
                request
                    .exact_source_path
                    .as_deref()
                    .or(request.path_prefix.as_deref()),
                None,
                None,
                None,
                100,
                0,
            )
            .await
            .map_err(|_| {
                MemoryError::Index(mcp_vault_indexer::IndexError::InvalidInput(
                    "memory pack lexical baseline failed",
                ))
            })?;
        let raw_candidate_count = lexical_raw.len() as u32;
        let lexical = lexical_raw
            .into_iter()
            .filter(|note| {
                canonical_request
                    .exact_source_path
                    .as_deref()
                    .is_none_or(|path| note.path.as_str() == path)
                    && canonical_request
                        .path_prefix
                        .as_deref()
                        .is_none_or(|prefix| path_prefix_matches(note.path.as_str(), prefix))
            })
            .collect::<Vec<_>>();
        let mut manifest = Vec::new();
        let mut manifest_changed = false;
        let mut seen_paths = HashSet::new();
        let mut seen_files = HashSet::new();
        for raw_path in &request.comparison_source_paths {
            let Ok(path) = VaultPath::parse(raw_path) else {
                return Ok(comparison_gap_records(
                    &input_hash,
                    "comparison_source_manifest_invalid",
                ));
            };
            if !seen_paths.insert(path.as_str().to_owned()) {
                return Ok(comparison_gap_records(
                    &input_hash,
                    "comparison_source_manifest_invalid",
                ));
            }
            let Some(file) = self.state.files().get_active(context, &path).await? else {
                return Ok(comparison_gap_records(
                    &input_hash,
                    "comparison_source_manifest_invalid",
                ));
            };
            if core.is_managed_path(&path)
                || file.entry_type != mcp_vault_state::EntryType::File
                || !path.as_str().to_ascii_lowercase().ends_with(".md")
                || !seen_files.insert(file.id.to_string())
            {
                return Ok(comparison_gap_records(
                    &input_hash,
                    "comparison_source_manifest_invalid",
                ));
            }
            let source = self
                .state
                .semantic_memory()
                .get_source_by_file(context, file.id)
                .await?;
            manifest.push(MemoryPackManifestEntry {
                path: path.as_str().to_owned(),
                file_id: file.id.to_string(),
                source_id: source.as_ref().map(|source| source.source_id.to_string()),
                source_revision_id: source
                    .as_ref()
                    .and_then(|source| source.current_revision_id.map(|id| id.to_string())),
                revision: file.current_revision.value(),
                content_hash: file.content_hash,
            });
        }
        manifest.sort_by(|left, right| {
            (
                left.file_id.as_str(),
                left.revision,
                left.source_id.as_deref(),
            )
                .cmp(&(
                    right.file_id.as_str(),
                    right.revision,
                    right.source_id.as_deref(),
                ))
        });
        manifest.dedup_by(|left, right| {
            left.file_id == right.file_id
                && left.revision == right.revision
                && left.source_id == right.source_id
        });
        if !self.manifest_entries_current(context, &manifest).await? {
            manifest_changed = true;
        }
        if !self
            .current_candidates_match_manifest(context, core, &manifest)
            .await?
        {
            manifest_changed = true;
        }
        let manifest_hash = hash_manifest(&manifest)?;
        let mut ordinary_gaps = ordinary_baseline_gaps(&canonical_request);
        if raw_candidate_count == 100 {
            ordinary_gaps.push("ordinary_candidate_page_cap".to_owned());
        }
        let subject_terms = canonical_request
            .subject_terms
            .iter()
            .flat_map(|value| {
                mcp_vault_state::memory_search_terms([value.as_str()], 32)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let lexical = lexical
            .into_iter()
            .filter(|note| {
                subject_terms.iter().all(|term| {
                    let material = format!(
                        "{} {} {}",
                        note.path,
                        note.title.as_deref().unwrap_or_default(),
                        note.snippet
                    )
                    .to_lowercase();
                    material.contains(term)
                })
            })
            .collect::<Vec<_>>();
        if !subject_terms.is_empty() {
            ordinary_gaps.push("ordinary_subject_lexical_only".to_owned());
            if lexical.is_empty() {
                ordinary_gaps.push("ordinary_no_answer".to_owned());
            }
        } else if lexical.is_empty() {
            ordinary_gaps.push("ordinary_no_answer".to_owned());
        }
        if !self
            .index_hits_match_manifest(context, &lexical, &manifest)
            .await?
        {
            manifest_changed = true;
        }
        let lexical_wire = lexical
            .iter()
            .map(|note| {
                (
                    note.path.as_str(),
                    note.revision,
                    note.title.as_deref(),
                    note.snippet.as_str(),
                )
            })
            .collect::<Vec<_>>();
        let mut lexical_bytes = comparison_envelope_bytes(
            "A",
            serde_json::to_value(&lexical_wire)
                .map_err(|_| MemoryError::InvalidInput("comparison arm envelope invalid"))?,
            &manifest,
            &ordinary_gaps,
            &[],
            &[],
            !ordinary_gaps.is_empty(),
        )? as u32;
        let mut lexical_over_budget = lexical.len() as u32 > canonical_request.max_entries
            || lexical_bytes > canonical_request.max_bytes
            || lexical_bytes.div_ceil(4) as u32 > canonical_request.max_tokens;
        if lexical_over_budget {
            ordinary_gaps.push("comparison_budget_exceeded".to_owned());
            lexical_bytes = comparison_envelope_bytes(
                "A",
                serde_json::to_value(&lexical_wire)
                    .map_err(|_| MemoryError::InvalidInput("comparison arm envelope invalid"))?,
                &manifest,
                &ordinary_gaps,
                &[],
                &[],
                true,
            )? as u32;
            lexical_over_budget = true;
        }
        let lexical_tokens = lexical_bytes.div_ceil(4);
        let mut records = vec![MemoryPackComparisonRecord {
            mode: "A".to_owned(),
            input_hash: input_hash.clone(),
            manifest_hash: manifest_hash.clone(),
            manifest_count: manifest.len() as u32,
            candidate_count: raw_candidate_count,
            eligible_count: lexical.len() as u32,
            relevant_count: lexical.len() as u32,
            returned_count: if lexical_over_budget {
                0
            } else {
                lexical.len() as u32
            },
            estimated_bytes: if lexical_over_budget {
                0
            } else {
                lexical_bytes
            },
            estimated_tokens: if lexical_over_budget {
                0
            } else {
                lexical_tokens
            },
            gap_codes: ordinary_gaps.clone(),
            degraded: !ordinary_gaps.is_empty() || lexical_over_budget,
            pending_real_evaluation: true,
        }];
        let scoped_request = MemoryPackRequest {
            source_scope: Some(MemoryPackSourceScope {
                source_ids: manifest
                    .iter()
                    .filter_map(|entry| entry.source_id.clone())
                    .collect(),
                source_paths: manifest.iter().map(|entry| entry.path.clone()).collect(),
                source_revision_ids: manifest
                    .iter()
                    .filter_map(|entry| entry.source_revision_id.clone())
                    .collect(),
            }),
            ..canonical_request.clone()
        };
        for (mode, include_m2) in [("B", false), ("C", true)] {
            let pack = self
                .build(
                    context,
                    core,
                    &MemoryPackRequest {
                        include_m1: true,
                        include_m2,
                        ..scoped_request.clone()
                    },
                )
                .await;
            let pack = match pack {
                Ok(pack) => pack,
                Err(MemoryError::InvalidInput(
                    "memory pack budget cannot fit the response wrapper",
                ))
                | Err(MemoryError::InvalidInput(
                    "memory pack budget cannot fit the final response wrapper",
                )) => {
                    let mut pack = MemoryPack::default();
                    pack.diagnostics
                        .push("comparison_budget_exceeded".to_owned());
                    pack.evidence_gaps.push(MemoryPackGap {
                        code: "comparison_budget_exceeded".to_owned(),
                        pointer: None,
                    });
                    pack
                }
                Err(error) => return Err(error),
            };
            let mut pack = pack;
            if manifest_changed
                || !self
                    .pack_sources_match_manifest(context, &pack, &manifest)
                    .await?
            {
                pack.current_context.clear();
                pack.relevant_experiences.clear();
                pack.conflicts_or_checks.clear();
                pack.related_sources.clear();
                pack.diagnostics
                    .push("source_manifest_changed_before_response".to_owned());
                pack.evidence_gaps.push(MemoryPackGap {
                    code: "source_manifest_changed_before_response".to_owned(),
                    pointer: None,
                });
            }
            let mut gap_codes = pack
                .evidence_gaps
                .iter()
                .map(|gap| gap.code.clone())
                .collect::<Vec<_>>();
            let mut envelope_bytes = comparison_envelope_bytes(
                mode,
                serde_json::to_value(&pack)
                    .map_err(|_| MemoryError::InvalidInput("comparison arm envelope invalid"))?,
                &manifest,
                &gap_codes,
                &pack.diagnostics,
                &pack.conflicts_or_checks,
                !pack.diagnostics.is_empty() || !pack.evidence_gaps.is_empty(),
            )?;
            let mut over_budget = envelope_bytes as u32 > canonical_request.max_bytes
                || (envelope_bytes as u32).div_ceil(4) > canonical_request.max_tokens;
            if over_budget {
                gap_codes.push("comparison_budget_exceeded".to_owned());
                envelope_bytes = comparison_envelope_bytes(
                    mode,
                    serde_json::to_value(&pack).map_err(|_| {
                        MemoryError::InvalidInput("comparison arm envelope invalid")
                    })?,
                    &manifest,
                    &gap_codes,
                    &pack.diagnostics,
                    &pack.conflicts_or_checks,
                    true,
                )?;
                over_budget = true;
            }
            records.push(MemoryPackComparisonRecord {
                mode: mode.to_owned(),
                input_hash: input_hash.clone(),
                manifest_hash: manifest_hash.clone(),
                manifest_count: manifest.len() as u32,
                candidate_count: (pack.current_context.len() + pack.relevant_experiences.len())
                    as u32,
                eligible_count: (pack.current_context.len() + pack.relevant_experiences.len())
                    as u32,
                relevant_count: (pack.current_context.len() + pack.relevant_experiences.len())
                    as u32,
                returned_count: if over_budget {
                    0
                } else {
                    (pack.current_context.len() + pack.relevant_experiences.len()) as u32
                },
                estimated_bytes: if over_budget {
                    0
                } else {
                    envelope_bytes as u32
                },
                estimated_tokens: if over_budget {
                    0
                } else {
                    (envelope_bytes as u32).div_ceil(4)
                },
                gap_codes,
                degraded: over_budget
                    || !pack.diagnostics.is_empty()
                    || !pack.evidence_gaps.is_empty(),
                pending_real_evaluation: true,
            });
        }
        let final_manifest_hash = hash_manifest(&manifest)?;
        for record in &mut records {
            record.manifest_hash = final_manifest_hash.clone();
            record.manifest_count = manifest.len() as u32;
            if manifest_changed {
                record.candidate_count = 0;
                record.eligible_count = 0;
                record.relevant_count = 0;
                record.returned_count = 0;
                record.estimated_bytes = 0;
                record.estimated_tokens = 0;
                record.degraded = true;
                record
                    .gap_codes
                    .push("source_manifest_changed_before_response".to_owned());
            }
        }
        Ok(records)
    }

    async fn index_hits_match_manifest(
        &self,
        context: &VaultContext,
        hits: &[mcp_vault_state::NoteSearchRecord],
        manifest: &[MemoryPackManifestEntry],
    ) -> Result<bool, MemoryError> {
        for hit in hits {
            let Some(entry) = manifest.iter().find(|entry| {
                entry.file_id == hit.file_id.to_string()
                    && entry.path == hit.path.as_str()
                    && entry.revision == hit.revision.value()
            }) else {
                return Ok(false);
            };
            let Some(file) = self.state.files().get_by_id(context, hit.file_id).await? else {
                return Ok(false);
            };
            if file.current_revision.value() != entry.revision
                || file.content_hash != entry.content_hash
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn manifest_entries_current(
        &self,
        context: &VaultContext,
        manifest: &[MemoryPackManifestEntry],
    ) -> Result<bool, MemoryError> {
        for source in manifest {
            let file_id = FileId::parse(&source.file_id)
                .map_err(|_| MemoryError::InvalidInput("source manifest file identity invalid"))?;
            let Some(file) = self.state.files().get_by_id(context, file_id).await? else {
                return Ok(false);
            };
            if file.path.as_str() != source.path
                || file.current_revision.value() != source.revision
                || file.content_hash != source.content_hash
            {
                return Ok(false);
            }
            if let (Some(source_id), Some(source_revision_id)) =
                (&source.source_id, &source.source_revision_id)
            {
                let parsed_source = mcp_vault_domain::SemanticSourceId::parse(source_id)
                    .map_err(|_| MemoryError::InvalidInput("source manifest identity invalid"))?;
                let parsed_source_revision =
                    mcp_vault_domain::SourceRevisionId::parse(source_revision_id).map_err(
                        |_| MemoryError::InvalidInput("source manifest revision identity invalid"),
                    )?;
                let Some(record) = self
                    .state
                    .semantic_memory()
                    .get_source(context, parsed_source)
                    .await?
                else {
                    return Ok(false);
                };
                if record.source_path.as_str() != source.path
                    || record
                        .current_revision_id
                        .map(|id| id.to_string())
                        .as_deref()
                        != Some(source_revision_id.as_str())
                {
                    return Ok(false);
                }
                let Some(content_hash) = source.content_hash.as_deref() else {
                    return Ok(false);
                };
                if record.content_hash.trim_start_matches("sha256:")
                    != content_hash.trim_start_matches("sha256:")
                {
                    return Ok(false);
                }
                let Some(revision) = self
                    .state
                    .semantic_memory()
                    .get_source_revision(context, parsed_source_revision)
                    .await?
                else {
                    return Ok(false);
                };
                if revision.source_id != record.source_id
                    || revision.file_id != file_id
                    || revision.content_hash.trim_start_matches("sha256:")
                        != content_hash.trim_start_matches("sha256:")
                {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    async fn current_candidates_match_manifest(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        manifest: &[MemoryPackManifestEntry],
    ) -> Result<bool, MemoryError> {
        let semantic = crate::SemanticMemoryService::new(self.state.clone());
        for card in semantic.list_cards(context, core, CANDIDATE_LIMIT).await? {
            let Some(source) = manifest.iter().find(|source| {
                source.source_id.as_deref() == Some(card.source_id.to_string().as_str())
                    && source.source_revision_id.as_deref()
                        == Some(card.source_revision_id.to_string().as_str())
                    && source.path == card.source_path.as_str()
            }) else {
                return Ok(false);
            };
            if !self.manifest_file_matches(context, source).await? {
                return Ok(false);
            }
        }
        for card in crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
            .list_composed_cards(context, core, CANDIDATE_LIMIT)
            .await?
        {
            for member in card
                .items
                .iter()
                .flat_map(|item| item.support_groups.iter())
                .flat_map(|group| group.members.iter())
            {
                let Some(source) = manifest.iter().find(|source| {
                    source.source_id.as_deref() == Some(member.source_id.to_string().as_str())
                        && source.source_revision_id.as_deref()
                            == Some(member.source_revision_id.to_string().as_str())
                        && source.path == member.source_path.as_str()
                }) else {
                    return Ok(false);
                };
                if !self.manifest_file_matches(context, source).await? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    async fn manifest_file_matches(
        &self,
        context: &VaultContext,
        source: &MemoryPackManifestEntry,
    ) -> Result<bool, MemoryError> {
        let file_id = FileId::parse(&source.file_id)
            .map_err(|_| MemoryError::InvalidInput("source manifest file identity invalid"))?;
        let Some(file) = self.state.files().get_by_id(context, file_id).await? else {
            return Ok(false);
        };
        Ok(file.path.as_str() == source.path
            && file.current_revision.value() == source.revision
            && file.content_hash == source.content_hash)
    }

    async fn pack_sources_match_manifest(
        &self,
        context: &VaultContext,
        pack: &MemoryPack,
        manifest: &[MemoryPackManifestEntry],
    ) -> Result<bool, MemoryError> {
        for pack_entry in pack
            .current_context
            .iter()
            .chain(pack.relevant_experiences.iter())
        {
            for source in &pack_entry.source_references {
                let Some(manifest_entry) = manifest.iter().find(|entry| {
                    entry.source_id.as_deref() == Some(source.source_id.as_str())
                        && entry.source_revision_id.as_deref()
                            == Some(source.source_revision_id.as_str())
                        && source
                            .source_path
                            .as_deref()
                            .is_none_or(|path| entry.path == path)
                }) else {
                    return Ok(false);
                };
                if !self.manifest_file_matches(context, manifest_entry).await? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub async fn build(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        request: &MemoryPackRequest,
    ) -> Result<MemoryPack, MemoryError> {
        self.build_inner(
            context,
            core,
            request,
            None::<fn() -> std::future::Ready<Result<(), MemoryError>>>,
        )
        .await
    }

    /// Test-only seam for exercising a real State/Core mutation between the
    /// initial candidate snapshot and final revalidation.
    #[doc(hidden)]
    pub async fn build_with_test_hook<F, Fut>(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        request: &MemoryPackRequest,
        hook: F,
    ) -> Result<MemoryPack, MemoryError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), MemoryError>>,
    {
        self.build_inner(context, core, request, Some(hook)).await
    }

    async fn build_inner<F, Fut>(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        request: &MemoryPackRequest,
        hook: Option<F>,
    ) -> Result<MemoryPack, MemoryError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), MemoryError>>,
    {
        validate_request(request)?;
        let mut pack = MemoryPack::default();
        if matches!(request.mode, MemoryPackMode::History) {
            pack.diagnostics
                .push("history_mode_current_qualification_only".to_owned());
            pack.evidence_gaps.push(MemoryPackGap {
                code: "history_mode_unsupported".to_owned(),
                pointer: None,
            });
            return Ok(pack);
        }
        if request.version.is_some() || request.as_of.is_some() {
            pack.diagnostics
                .push("requested_metadata_is_not_explicitly_persisted".to_owned());
            pack.evidence_gaps.push(MemoryPackGap {
                code: "version_or_as_of_unknown".to_owned(),
                pointer: None,
            });
            return Ok(pack);
        }

        let semantic = crate::SemanticMemoryService::new(self.state.clone());
        if !semantic.ensure_current_policy(context).await? {
            pack.diagnostics
                .push("semantic_policy_unavailable".to_owned());
            pack.evidence_gaps.push(MemoryPackGap {
                code: "policy_unavailable".to_owned(),
                pointer: None,
            });
            return Ok(pack);
        }
        let m1 = semantic.list_cards(context, core, CANDIDATE_LIMIT).await?;
        let m2 = crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
            .list_composed_cards(context, core, CANDIDATE_LIMIT)
            .await?;
        if m1.len() as u32 == CANDIDATE_LIMIT || m2.len() as u32 == CANDIDATE_LIMIT {
            pack.diagnostics.push("candidate_truncated".to_owned());
        }
        let source_paths = m1
            .iter()
            .map(|card| {
                (
                    card.source_id.to_string(),
                    card.source_path.as_str().to_owned(),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut candidates = Vec::new();
        if request.include_m1 {
            candidates.extend(m1.into_iter().map(Candidate::M1));
        }
        if request.include_m2 {
            candidates.extend(m2.into_iter().map(Candidate::M2));
        }
        let task_terms = lexical_terms(request);
        let subject_terms = request
            .subject_terms
            .iter()
            .flat_map(|value| {
                mcp_vault_state::memory_search_terms([value.as_str()], 32)
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut ranked = Vec::new();
        for candidate in candidates {
            let material = candidate_material(&candidate, &source_paths);
            if !matches_filters(&candidate, request, &source_paths) {
                continue;
            }
            let score = lexical_score(&material, &task_terms, &subject_terms);
            if !task_terms.is_empty() && score.0 == 0 {
                continue;
            }
            if !subject_terms.is_empty() && score.1 == 0 {
                continue;
            }
            ranked.push((score, candidate));
        }
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| candidate_identity(&left.1).cmp(&candidate_identity(&right.1)))
        });
        if ranked.is_empty() {
            pack.diagnostics.push(if !subject_terms.is_empty() {
                "subject_mismatch".to_owned()
            } else if candidates_empty(&source_paths) {
                "no_current_candidates".to_owned()
            } else {
                "no_relevant_candidates".to_owned()
            });
            pack.evidence_gaps.push(MemoryPackGap {
                code: "no_answer".to_owned(),
                pointer: None,
            });
            return Ok(pack);
        }

        let latest_m1 = semantic.list_cards(context, core, CANDIDATE_LIMIT).await?;
        let latest_m2 =
            crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
                .list_composed_cards(context, core, CANDIDATE_LIMIT)
                .await?;
        let latest_ids = latest_m1
            .into_iter()
            .map(|card| format!("m1:{}:{}", card.id, card.revision_id))
            .chain(
                latest_m2
                    .into_iter()
                    .map(|card| format!("m2:{}:{}", card.id, card.revision_id)),
            )
            .collect::<HashSet<_>>();
        let relations = self
            .state
            .semantic_organization()
            .list_current_relations(context, 200)
            .await?;
        let relations = relations_within_source_scope(relations, request.source_scope.as_ref());
        let mut seen = BTreeMap::<String, MemoryPackEntry>::new();
        for (_, candidate) in ranked {
            if !latest_ids.contains(&candidate_identity(&candidate)) {
                pack.diagnostics
                    .push("candidate_changed_during_build".to_owned());
                continue;
            }
            let entry = candidate_to_entry(&candidate, &source_paths);
            let key = format!(
                "{}|{}",
                semantic_key(&entry),
                relation_barrier_key(&entry, &relations)
            );
            if let Some(existing) = seen.get_mut(&key) {
                merge_entry(existing, entry);
            } else {
                seen.insert(key, entry);
            }
        }
        let mut entries = seen.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        let source_ids = entries
            .iter()
            .flat_map(|entry| {
                entry
                    .source_references
                    .iter()
                    .map(|source| source.source_id.clone())
            })
            .collect::<HashSet<_>>();
        pack.conflicts_or_checks = relations
            .iter()
            .filter(|relation| {
                matches!(
                    relation.relation_kind.as_str(),
                    "conflicts" | "different_scope" | "supersedes"
                )
            })
            .filter(|relation| {
                source_ids.contains(&relation.left_source_id.to_string())
                    || source_ids.contains(&relation.right_source_id.to_string())
            })
            .map(relation_to_conflict)
            .collect();
        apply_budget(&mut pack, entries, request);
        if let Some(hook) = hook {
            hook().await?;
        }
        self.revalidate_final_entries(context, core, request, &mut pack)
            .await?;
        if !enforce_final_budget(&mut pack, request) {
            return Err(MemoryError::InvalidInput(
                "memory pack budget cannot fit the response wrapper",
            ));
        }
        self.refresh_relation_metadata(context, &mut pack, request.source_scope.as_ref())
            .await?;
        if !enforce_final_budget(&mut pack, request) {
            return Err(MemoryError::InvalidInput(
                "memory pack budget cannot fit the final response wrapper",
            ));
        }
        Ok(pack)
    }

    async fn revalidate_final_entries(
        &self,
        context: &VaultContext,
        core: &VaultCore,
        request: &MemoryPackRequest,
        pack: &mut MemoryPack,
    ) -> Result<(), MemoryError> {
        let semantic = crate::SemanticMemoryService::new(self.state.clone());
        if !semantic.ensure_current_policy(context).await? {
            pack.current_context.clear();
            pack.relevant_experiences.clear();
            pack.related_sources.clear();
            pack.conflicts_or_checks.clear();
            pack.diagnostics
                .push("candidate_changed_before_response".to_owned());
            pack.evidence_gaps.push(MemoryPackGap {
                code: "policy_unavailable".to_owned(),
                pointer: None,
            });
            return Ok(());
        }
        let m1 = semantic.list_cards(context, core, CANDIDATE_LIMIT).await?;
        let m2 = crate::semantic::organize::SemanticOrganizationService::new(self.state.clone())
            .list_composed_cards(context, core, CANDIDATE_LIMIT)
            .await?;
        let relations = self
            .state
            .semantic_organization()
            .list_current_relations(context, 200)
            .await?;
        let relations = relations_within_source_scope(relations, request.source_scope.as_ref());
        let mut current = HashMap::new();
        for card in m1 {
            current.insert(
                format!("m1:{}:{}", card.id, card.revision_id),
                Candidate::M1(card),
            );
        }
        for card in m2 {
            current.insert(
                format!("m2:{}:{}", card.id, card.revision_id),
                Candidate::M2(card),
            );
        }

        let mut rebuilt = Vec::new();
        let mut removed = false;
        for entry in pack
            .current_context
            .iter()
            .chain(pack.relevant_experiences.iter())
        {
            let candidates = entry
                .candidate_keys
                .iter()
                .filter_map(|key| current.get(key).cloned())
                .collect::<Vec<_>>();
            if candidates.len() != entry.candidate_keys.len() || candidates.is_empty() {
                removed = true;
            }
            rebuilt.extend(candidates);
        }
        if removed {
            pack.diagnostics
                .push("candidate_changed_before_response".to_owned());
            pack.evidence_gaps.push(MemoryPackGap {
                code: "candidate_changed_before_response".to_owned(),
                pointer: None,
            });
        }

        let mut source_paths = HashMap::new();
        for candidate in &rebuilt {
            if let Candidate::M1(card) = candidate {
                source_paths.insert(card.source_id.to_string(), card.source_path.to_string());
            }
        }
        let mut seen = BTreeMap::<String, MemoryPackEntry>::new();
        for candidate in rebuilt {
            let entry = candidate_to_entry(&candidate, &source_paths);
            let key = format!(
                "{}|{}",
                semantic_key(&entry),
                relation_barrier_key(&entry, &relations)
            );
            if let Some(existing) = seen.get_mut(&key) {
                merge_entry(existing, entry);
            } else {
                seen.insert(key, entry);
            }
        }
        let mut entries = seen.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        pack.current_context.clear();
        pack.relevant_experiences.clear();
        pack.related_sources.clear();
        pack.conflicts_or_checks.clear();
        apply_budget(pack, entries, request);
        self.refresh_relation_metadata(context, pack, request.source_scope.as_ref())
            .await?;
        Ok(())
    }

    async fn refresh_relation_metadata(
        &self,
        context: &VaultContext,
        pack: &mut MemoryPack,
        source_scope: Option<&MemoryPackSourceScope>,
    ) -> Result<(), MemoryError> {
        let mut entries = pack.current_context.clone();
        entries.extend(pack.relevant_experiences.clone());
        pack.related_sources = related_for_entries(&entries);
        let source_ids = entries
            .iter()
            .flat_map(|entry| {
                entry
                    .source_references
                    .iter()
                    .map(|source| source.source_id.clone())
            })
            .collect::<HashSet<_>>();
        let relations = self
            .state
            .semantic_organization()
            .list_current_relations(context, 200)
            .await?;
        let relations = relations_within_source_scope(relations, source_scope);
        pack.conflicts_or_checks = relations
            .iter()
            .filter(|relation| {
                matches!(
                    relation.relation_kind.as_str(),
                    "conflicts" | "different_scope" | "supersedes"
                )
            })
            .filter(|relation| {
                source_ids.contains(&relation.left_source_id.to_string())
                    || source_ids.contains(&relation.right_source_id.to_string())
            })
            .map(relation_to_conflict)
            .collect();
        Ok(())
    }
}

fn validate_request(request: &MemoryPackRequest) -> Result<(), MemoryError> {
    if request.task.len() > 8_000 || request.query.as_deref().is_some_and(|v| v.len() > 8_000) {
        return Err(MemoryError::InvalidInput("memory pack query is too large"));
    }
    if request.max_entries == 0
        || request.max_entries > 200
        || request.max_bytes == 0
        || request.max_tokens == 0
    {
        return Err(MemoryError::InvalidInput("memory pack budget is invalid"));
    }
    if request.exact_source_path.is_some() && request.path_prefix.is_some() {
        return Err(MemoryError::InvalidInput(
            "memory pack path filters conflict",
        ));
    }
    if let Some(scope) = &request.source_scope {
        if scope.source_ids.is_empty()
            && scope.source_paths.is_empty()
            && scope.source_revision_ids.is_empty()
        {
            return Err(MemoryError::InvalidInput(
                "memory pack source scope is empty",
            ));
        }
        if scope
            .source_paths
            .iter()
            .any(|path| VaultPath::parse(path).is_err())
            || scope
                .source_ids
                .iter()
                .any(|id| mcp_vault_domain::SemanticSourceId::parse(id).is_err())
            || scope
                .source_revision_ids
                .iter()
                .any(|id| mcp_vault_domain::SourceRevisionId::parse(id).is_err())
            || has_duplicate_strings(&scope.source_ids)
            || has_duplicate_strings(&scope.source_paths)
            || has_duplicate_strings(&scope.source_revision_ids)
        {
            return Err(MemoryError::InvalidInput(
                "memory pack source scope is invalid",
            ));
        }
    }
    Ok(())
}

fn has_duplicate_strings(values: &[String]) -> bool {
    values.iter().collect::<HashSet<_>>().len() != values.len()
}

fn relations_within_source_scope(
    relations: Vec<SemanticRelationRecord>,
    scope: Option<&MemoryPackSourceScope>,
) -> Vec<SemanticRelationRecord> {
    let Some(scope) = scope else {
        return relations;
    };
    // Filter before relation barriers participate in candidate merge/dedupe.
    // A path/revision-only scope cannot authorize relation endpoints.
    relations
        .into_iter()
        .filter(|relation| {
            !scope.source_ids.is_empty()
                && scope
                    .source_ids
                    .iter()
                    .any(|id| id == &relation.left_source_id.to_string())
                && scope
                    .source_ids
                    .iter()
                    .any(|id| id == &relation.right_source_id.to_string())
        })
        .collect()
}

fn lexical_terms(request: &MemoryPackRequest) -> Vec<String> {
    mcp_vault_state::memory_search_terms(
        [
            request.task.as_str(),
            request.query.as_deref().unwrap_or_default(),
        ],
        64,
    )
    .split_whitespace()
    .map(str::to_owned)
    .collect()
}

fn candidate_identity(candidate: &Candidate) -> String {
    match candidate {
        Candidate::M1(card) => format!("m1:{}:{}", card.id, card.revision_id),
        Candidate::M2(card) => format!("m2:{}:{}", card.id, card.revision_id),
    }
}

fn candidate_material(candidate: &Candidate, _source_paths: &HashMap<String, String>) -> String {
    match candidate {
        Candidate::M1(card) => card
            .items
            .iter()
            .map(|item| item.content.as_str())
            .chain([
                card.title.as_str(),
                card.kind.as_str(),
                card.scope_ref.as_str(),
            ])
            .collect::<Vec<_>>()
            .join(" "),
        Candidate::M2(card) => card
            .items
            .iter()
            .flat_map(|item| {
                item.support_groups
                    .iter()
                    .flat_map(|group| {
                        group.members.iter().flat_map(|member| {
                            [
                                member.source_id.to_string(),
                                member.source_path.as_str().to_owned(),
                            ]
                        })
                    })
                    .chain([item.content.clone()])
                    .collect::<Vec<_>>()
            })
            .chain([
                card.title.clone(),
                card.kind.clone(),
                card.scope_ref.clone(),
            ])
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn lexical_score(material: &str, task_terms: &[String], subject_terms: &[String]) -> (u32, u32) {
    let lowered = material.to_lowercase();
    let task_hits = task_terms
        .iter()
        .filter(|term| lowered.contains(term.as_str()))
        .count() as u32;
    let subject_hits = subject_terms
        .iter()
        .filter(|term| lowered.contains(term.as_str()))
        .count() as u32;
    (task_hits, subject_hits)
}

fn matches_filters(
    candidate: &Candidate,
    request: &MemoryPackRequest,
    _source_paths: &HashMap<String, String>,
) -> bool {
    let (kind, scope, paths) = match candidate {
        Candidate::M1(card) => (
            card.kind.as_str(),
            card.scope_ref.as_str(),
            vec![card.source_path.as_str().to_owned()],
        ),
        Candidate::M2(card) => (
            card.kind.as_str(),
            card.scope_ref.as_str(),
            card.items
                .iter()
                .flat_map(|item| item.support_groups.iter())
                .flat_map(|group| group.members.iter())
                .map(|member| member.source_path.as_str().to_owned())
                .collect(),
        ),
    };
    let source_refs = candidate_source_refs(candidate);
    let scope_matches = request
        .source_scope
        .as_ref()
        .is_none_or(|scope| source_scope_allows(scope, &source_refs));
    scope_matches
        && request
            .scope_ref
            .as_deref()
            .is_none_or(|scope_ref| scope_ref == scope)
        && (request.kinds.is_empty() || request.kinds.iter().any(|value| value == kind))
        && request
            .exact_source_path
            .as_deref()
            .is_none_or(|path| paths.iter().any(|value| value == path))
        && request
            .path_prefix
            .as_deref()
            .is_none_or(|prefix| paths.iter().any(|value| path_prefix_matches(value, prefix)))
}

fn source_scope_allows(
    scope: &MemoryPackSourceScope,
    references: &[(String, String, Option<String>)],
) -> bool {
    !references.is_empty()
        && references.iter().all(|reference| {
            (scope.source_ids.is_empty() || scope.source_ids.iter().any(|id| id == &reference.0))
                && (scope.source_paths.is_empty()
                    || reference
                        .2
                        .as_deref()
                        .is_some_and(|path| scope.source_paths.iter().any(|p| p == path)))
                && (scope.source_revision_ids.is_empty()
                    || scope
                        .source_revision_ids
                        .iter()
                        .any(|id| id == &reference.1))
        })
}

fn candidate_source_refs(candidate: &Candidate) -> Vec<(String, String, Option<String>)> {
    match candidate {
        Candidate::M1(card) => vec![(
            card.source_id.to_string(),
            card.source_revision_id.to_string(),
            Some(card.source_path.as_str().to_owned()),
        )],
        Candidate::M2(card) => card
            .items
            .iter()
            .flat_map(|item| item.support_groups.iter())
            .flat_map(|group| group.members.iter())
            .map(|member| {
                (
                    member.source_id.to_string(),
                    member.source_revision_id.to_string(),
                    Some(member.source_path.as_str().to_owned()),
                )
            })
            .collect(),
    }
}

fn path_prefix_matches(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn candidate_to_entry(
    candidate: &Candidate,
    _source_paths: &HashMap<String, String>,
) -> MemoryPackEntry {
    match candidate {
        Candidate::M1(card) => {
            let mut entry = MemoryPackEntry {
                id: card.id.to_string(),
                origin: "m1".to_owned(),
                title: card.title.clone(),
                kind: card.kind.clone(),
                scope_ref: card.scope_ref.clone(),
                assertion_status: card.assertion_status.clone(),
                temporal_scope: card.temporal_scope.clone(),
                core_assertions: Vec::new(),
                conditions: Vec::new(),
                exceptions: Vec::new(),
                ordered_steps: Vec::new(),
                unresolved_items: Vec::new(),
                optional_details: Vec::new(),
                source_references: vec![MemoryPackSourceReference {
                    source_id: card.source_id.to_string(),
                    source_revision_id: card.source_revision_id.to_string(),
                    source_path: Some(card.source_path.as_str().to_owned()),
                }],
                evidence_refs: Vec::new(),
                candidate_keys: vec![candidate_identity(candidate)],
            };
            for item in &card.items {
                match item.kind.as_str() {
                    "core_assertion" => entry.core_assertions.push(item.content.clone()),
                    "condition" => entry.conditions.push(item.content.clone()),
                    "exception" => entry.exceptions.push(item.content.clone()),
                    "ordered_step" => entry.ordered_steps.push(item.content.clone()),
                    "unresolved_item" => entry.unresolved_items.push(item.content.clone()),
                    _ => entry.optional_details.push(item.content.clone()),
                }
                entry
                    .evidence_refs
                    .extend(item.evidence_ref_ids.iter().map(ToString::to_string));
            }
            entry
        }
        Candidate::M2(card) => {
            let mut entry = MemoryPackEntry {
                id: card.id.to_string(),
                origin: "m2".to_owned(),
                title: card.title.clone(),
                kind: card.kind.clone(),
                scope_ref: card.scope_ref.clone(),
                assertion_status: card.assertion_status.clone(),
                temporal_scope: card.temporal_scope.clone(),
                core_assertions: Vec::new(),
                conditions: Vec::new(),
                exceptions: Vec::new(),
                ordered_steps: Vec::new(),
                unresolved_items: Vec::new(),
                optional_details: Vec::new(),
                source_references: Vec::new(),
                evidence_refs: Vec::new(),
                candidate_keys: vec![candidate_identity(candidate)],
            };
            for item in &card.items {
                match item.kind.as_str() {
                    "core_assertion" => entry.core_assertions.push(item.content.clone()),
                    "condition" => entry.conditions.push(item.content.clone()),
                    "exception" => entry.exceptions.push(item.content.clone()),
                    "ordered_step" => entry.ordered_steps.push(item.content.clone()),
                    "unresolved_item" => entry.unresolved_items.push(item.content.clone()),
                    _ => entry.optional_details.push(item.content.clone()),
                }
                for group in &item.support_groups {
                    for member in &group.members {
                        entry.source_references.push(MemoryPackSourceReference {
                            source_id: member.source_id.to_string(),
                            source_revision_id: member.source_revision_id.to_string(),
                            source_path: Some(member.source_path.as_str().to_owned()),
                        });
                        entry.evidence_refs.push(member.evidence_ref_id.to_string());
                    }
                }
            }
            entry
        }
    }
}

fn semantic_key(entry: &MemoryPackEntry) -> String {
    let temporal_scope =
        if entry.temporal_scope.get("status").and_then(Value::as_str) == Some("unknown") {
            Value::String("unknown".to_owned())
        } else {
            entry.temporal_scope.clone()
        };
    serde_json::to_string(&(
        &entry.kind,
        &entry.scope_ref,
        &entry.assertion_status,
        &temporal_scope,
        &entry.core_assertions,
        &entry.conditions,
        &entry.exceptions,
        &entry.ordered_steps,
        &entry.unresolved_items,
    ))
    .unwrap_or_default()
}

fn relation_barrier_key(entry: &MemoryPackEntry, relations: &[SemanticRelationRecord]) -> String {
    let sources = entry
        .source_references
        .iter()
        .map(|source| source.source_id.as_str())
        .collect::<HashSet<_>>();
    let mut barriers = relations
        .iter()
        .filter(|relation| {
            matches!(
                relation.relation_kind.as_str(),
                "conflicts" | "different_scope" | "supersedes"
            ) && (sources.contains(relation.left_source_id.to_string().as_str())
                || sources.contains(relation.right_source_id.to_string().as_str()))
        })
        .flat_map(|relation| {
            let id = relation.id.to_string();
            let left_source = relation.left_source_id.to_string();
            let right_source = relation.right_source_id.to_string();
            let mut endpoints = Vec::new();
            if sources.contains(left_source.as_str()) {
                endpoints.push(format!(
                    "{id}:left:{}:{left_source}:{}",
                    entry.id, relation.left_observation_id
                ));
            }
            if sources.contains(right_source.as_str()) {
                endpoints.push(format!(
                    "{id}:right:{}:{right_source}:{}",
                    entry.id, relation.right_observation_id
                ));
            }
            endpoints
        })
        .collect::<Vec<_>>();
    barriers.sort();
    barriers.dedup();
    barriers.join(",")
}

fn merge_entry(existing: &mut MemoryPackEntry, incoming: MemoryPackEntry) {
    if incoming.origin == "m1" {
        existing.origin = "m1".to_owned();
    }
    existing
        .source_references
        .extend(incoming.source_references);
    existing.evidence_refs.extend(incoming.evidence_refs);
    existing.source_references.sort_by(|left, right| {
        (left.source_id.as_str(), left.source_revision_id.as_str())
            .cmp(&(right.source_id.as_str(), right.source_revision_id.as_str()))
    });
    existing.source_references.dedup_by(|left, right| {
        left.source_id == right.source_id && left.source_revision_id == right.source_revision_id
    });
    existing.evidence_refs.sort();
    existing.evidence_refs.dedup();
    existing.optional_details.extend(incoming.optional_details);
    existing.optional_details.sort();
    existing.optional_details.dedup();
    existing.candidate_keys.extend(incoming.candidate_keys);
    existing.candidate_keys.sort();
    existing.candidate_keys.dedup();
}

fn relation_to_conflict(relation: &SemanticRelationRecord) -> MemoryPackConflict {
    MemoryPackConflict {
        relation_id: relation.id.to_string(),
        relation_kind: relation.relation_kind.clone(),
        reason_code: relation.reason_code.clone(),
        source_ids: vec![
            relation.left_source_id.to_string(),
            relation.right_source_id.to_string(),
        ],
        left_observation_id: relation.left_observation_id.to_string(),
        right_observation_id: relation.right_observation_id.to_string(),
    }
}

fn candidates_empty(source_paths: &HashMap<String, String>) -> bool {
    source_paths.is_empty()
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let encoded = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{encoded}")
}

fn hash_manifest(manifest: &[MemoryPackManifestEntry]) -> Result<String, MemoryError> {
    let mut entries = manifest.to_vec();
    entries.sort_by(|left, right| {
        (
            left.path.as_str(),
            left.file_id.as_str(),
            left.revision,
            left.source_id.as_deref().unwrap_or_default(),
        )
            .cmp(&(
                right.path.as_str(),
                right.file_id.as_str(),
                right.revision,
                right.source_id.as_deref().unwrap_or_default(),
            ))
    });
    serde_json::to_vec(&entries)
        .map(|bytes| hash_bytes(&bytes))
        .map_err(|_| MemoryError::InvalidInput("memory pack source manifest invalid"))
}

fn comparison_gap_records(input_hash: &str, code: &str) -> Vec<MemoryPackComparisonRecord> {
    ["A", "B", "C"]
        .into_iter()
        .map(|mode| MemoryPackComparisonRecord {
            mode: mode.to_owned(),
            input_hash: input_hash.to_owned(),
            manifest_hash: hash_bytes(code.as_bytes()),
            manifest_count: 0,
            candidate_count: 0,
            eligible_count: 0,
            relevant_count: 0,
            returned_count: 0,
            estimated_bytes: 0,
            estimated_tokens: 0,
            gap_codes: vec![code.to_owned()],
            degraded: true,
            pending_real_evaluation: true,
        })
        .collect()
}

fn comparison_envelope_bytes(
    mode: &str,
    entries: Value,
    manifest: &[MemoryPackManifestEntry],
    gaps: &[String],
    diagnostics: &[String],
    relations: &[MemoryPackConflict],
    degraded: bool,
) -> Result<usize, MemoryError> {
    serde_json::to_vec(&ComparisonArmEnvelope {
        mode: mode.to_owned(),
        entries,
        source_manifest: manifest.to_vec(),
        gaps: gaps.to_vec(),
        diagnostics: diagnostics.to_vec(),
        relations: relations.to_vec(),
        degraded,
    })
    .map(|bytes| bytes.len())
    .map_err(|_| MemoryError::InvalidInput("comparison arm envelope invalid"))
}

fn ordinary_baseline_gaps(request: &MemoryPackRequest) -> Vec<String> {
    let mut gaps = Vec::new();
    if request.scope_ref.is_some() {
        gaps.push("ordinary_scope_unavailable".to_owned());
    }
    if !request.kinds.is_empty() {
        gaps.push("ordinary_kind_unavailable".to_owned());
    }
    if request.version.is_some() {
        gaps.push("ordinary_version_unavailable".to_owned());
    }
    if request.as_of.is_some() {
        gaps.push("ordinary_as_of_unavailable".to_owned());
    }
    if matches!(request.mode, MemoryPackMode::History) {
        gaps.push("ordinary_history_unavailable".to_owned());
    }
    if !request.subject_terms.is_empty() {
        gaps.push("ordinary_subject_unavailable".to_owned());
    }
    gaps
}

fn apply_budget(pack: &mut MemoryPack, entries: Vec<MemoryPackEntry>, request: &MemoryPackRequest) {
    let mut accepted = Vec::new();
    for mut entry in entries {
        if accepted.len() as u32 >= request.max_entries {
            pack.diagnostics.push("max_entries_reached".to_owned());
            break;
        }
        let mut trial = pack.clone();
        trial.current_context = accepted.clone();
        trial.current_context.push(entry.clone());
        trial.related_sources = related_for_entries(&trial.current_context);
        finalize_estimate(&mut trial);
        if fits(&trial, request) {
            accepted.push(entry);
            continue;
        }
        entry.optional_details.clear();
        let mut reduced = pack.clone();
        reduced.current_context = accepted.clone();
        reduced.current_context.push(entry.clone());
        reduced.related_sources = related_for_entries(&reduced.current_context);
        finalize_estimate(&mut reduced);
        if fits(&reduced, request) {
            accepted.push(entry);
        } else {
            pack.evidence_gaps.push(MemoryPackGap {
                code: "core_entry_exceeds_budget".to_owned(),
                pointer: Some(entry.id),
            });
            pack.diagnostics
                .push("core_entry_omitted_atomically".to_owned());
        }
    }
    pack.current_context = accepted
        .iter()
        .filter(|entry| entry.kind != "experience")
        .cloned()
        .collect();
    pack.relevant_experiences = accepted
        .into_iter()
        .filter(|entry| entry.kind == "experience")
        .collect();
    let mut all_entries = pack.current_context.clone();
    all_entries.extend(pack.relevant_experiences.clone());
    pack.related_sources = related_for_entries(&all_entries);
    finalize_estimate(pack);
}

fn related_for_entries(entries: &[MemoryPackEntry]) -> Vec<MemoryPackSource> {
    let mut related = BTreeMap::new();
    for entry in entries {
        for source in &entry.source_references {
            related
                .entry((source.source_id.clone(), source.source_revision_id.clone()))
                .or_insert_with(|| MemoryPackSource {
                    source_id: source.source_id.clone(),
                    source_revision_id: source.source_revision_id.clone(),
                    source_path: source.source_path.clone(),
                });
        }
    }
    related.into_values().collect()
}

fn enforce_final_budget(pack: &mut MemoryPack, request: &MemoryPackRequest) -> bool {
    finalize_estimate(pack);
    while !fits(pack, request) {
        if pack.related_sources.pop().is_some() {
            continue;
        }
        let mut removed_optional = false;
        for entry in pack
            .current_context
            .iter_mut()
            .chain(pack.relevant_experiences.iter_mut())
        {
            if !entry.optional_details.is_empty() {
                entry.optional_details.clear();
                removed_optional = true;
                break;
            }
        }
        if removed_optional {
            finalize_estimate(pack);
            continue;
        }
        if let Some(entry) = pack.relevant_experiences.pop() {
            pack.evidence_gaps.push(MemoryPackGap {
                code: "core_entry_exceeds_budget".to_owned(),
                pointer: Some(entry.id),
            });
            continue;
        }
        if let Some(entry) = pack.current_context.pop() {
            pack.evidence_gaps.push(MemoryPackGap {
                code: "core_entry_exceeds_budget".to_owned(),
                pointer: Some(entry.id),
            });
            continue;
        }
        return false;
    }
    true
}

fn finalize_estimate(pack: &mut MemoryPack) {
    if let Ok(bytes) = serde_json::to_vec(pack) {
        pack.estimated_tokens = bytes.len().div_ceil(4) as u32;
    }
}

fn fits(pack: &MemoryPack, request: &MemoryPackRequest) -> bool {
    let entries = pack.current_context.len() + pack.relevant_experiences.len();
    serde_json::to_vec(pack)
        .map(|bytes| {
            entries as u32 <= request.max_entries
                && bytes.len() as u32 <= request.max_bytes
                && pack.estimated_tokens <= request.max_tokens
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(optional: &str) -> MemoryPackEntry {
        MemoryPackEntry {
            id: "card".to_owned(),
            origin: "m1".to_owned(),
            title: "different titles do not define identity".to_owned(),
            kind: "decision".to_owned(),
            scope_ref: "project".to_owned(),
            assertion_status: "source_asserted".to_owned(),
            temporal_scope: Value::Object(Default::default()),
            core_assertions: vec!["core assertion".to_owned()],
            conditions: vec!["required condition".to_owned()],
            exceptions: Vec::new(),
            ordered_steps: vec!["step".to_owned()],
            unresolved_items: Vec::new(),
            optional_details: vec![optional.to_owned()],
            source_references: vec![MemoryPackSourceReference {
                source_id: "source".to_owned(),
                source_revision_id: "revision".to_owned(),
                source_path: Some("notes/one.md".to_owned()),
            }],
            evidence_refs: vec!["evidence".to_owned()],
            candidate_keys: vec!["m1:card:revision".to_owned()],
        }
    }

    #[test]
    fn budget_drops_optional_detail_but_never_slices_core() {
        let mut baseline = MemoryPack::default();
        let mut core_only = entry("");
        core_only.optional_details.clear();
        baseline.related_sources = related_for_entries(std::slice::from_ref(&core_only));
        baseline.current_context.push(core_only);
        finalize_estimate(&mut baseline);
        let baseline_bytes = serde_json::to_vec(&baseline).unwrap().len() as u32;
        let mut pack = MemoryPack::default();
        apply_budget(
            &mut pack,
            vec![entry(&"optional detail ".repeat(100))],
            &MemoryPackRequest {
                max_bytes: baseline_bytes + 64,
                ..MemoryPackRequest::default()
            },
        );
        assert_eq!(pack.current_context.len(), 1);
        assert_eq!(pack.current_context[0].core_assertions, ["core assertion"]);
        assert!(pack.current_context[0].optional_details.is_empty());
    }

    #[test]
    fn semantic_dedupe_merges_sources_without_using_title_or_source_count() {
        let first = entry("same optional");
        let mut second = entry("same optional");
        second.id = "card-2".to_owned();
        second.title = "another title".to_owned();
        second.source_references[0].source_id = "source-2".to_owned();
        let mut entries = BTreeMap::new();
        entries.insert(semantic_key(&first), first);
        let key = semantic_key(&second);
        if let Some(existing) = entries.get_mut(&key) {
            merge_entry(existing, second);
        }
        assert_eq!(entries.len(), 1);
        assert_eq!(entries.values().next().unwrap().source_references.len(), 2);
    }

    #[test]
    fn path_prefix_uses_vault_path_segments() {
        assert!(path_prefix_matches("notes/a.md", "notes"));
        assert!(path_prefix_matches("notes/a.md", "notes/"));
        assert!(!path_prefix_matches("notes/ab.md", "notes/a"));
    }

    #[test]
    fn source_scope_requires_every_card_reference_to_match_id_path_and_revision() {
        let scope = MemoryPackSourceScope {
            source_ids: vec!["00000000-0000-0000-0000-000000000001".into()],
            source_paths: vec!["notes/a.md".into()],
            source_revision_ids: vec!["00000000-0000-0000-0000-000000000002".into()],
        };
        let allowed = vec![(
            "00000000-0000-0000-0000-000000000001".into(),
            "00000000-0000-0000-0000-000000000002".into(),
            Some("notes/a.md".into()),
        )];
        assert!(source_scope_allows(&scope, &allowed));
        assert!(!source_scope_allows(
            &scope,
            &[(
                "00000000-0000-0000-0000-000000000003".into(),
                "00000000-0000-0000-0000-000000000002".into(),
                Some("notes/a.md".into())
            )]
        ));
        assert!(!source_scope_allows(
            &scope,
            &[
                allowed[0].clone(),
                (
                    "00000000-0000-0000-0000-000000000003".into(),
                    "00000000-0000-0000-0000-000000000004".into(),
                    Some("notes/b.md".into())
                ),
            ]
        ));
    }

    #[test]
    fn out_of_scope_relation_cannot_change_scoped_merge_barrier() {
        use mcp_vault_domain::{ObservationId, RelationDecisionId, SemanticSourceId};

        let selected = SemanticSourceId::new();
        let outside = SemanticSourceId::new();
        let relation = SemanticRelationRecord {
            id: RelationDecisionId::new(),
            relation_kind: "conflicts".into(),
            reason_code: "explicit".into(),
            left_source_id: selected,
            right_source_id: outside,
            left_observation_id: ObservationId::new(),
            right_observation_id: ObservationId::new(),
        };
        let entry = MemoryPackEntry {
            source_references: vec![MemoryPackSourceReference {
                source_id: selected.to_string(),
                source_revision_id: "revision".into(),
                source_path: Some("notes/selected.md".into()),
            }],
            ..entry("")
        };
        let scope = MemoryPackSourceScope {
            source_ids: vec![selected.to_string()],
            source_paths: vec!["notes/selected.md".into()],
            source_revision_ids: vec!["00000000-0000-0000-0000-000000000001".into()],
        };
        let filtered = relations_within_source_scope(vec![relation.clone()], Some(&scope));
        assert!(filtered.is_empty());
        assert_eq!(
            relation_barrier_key(&entry, &filtered),
            relation_barrier_key(&entry, &[])
        );
        assert_ne!(
            relation_barrier_key(&entry, &[relation]),
            relation_barrier_key(&entry, &[])
        );
    }

    #[test]
    fn safety_metadata_overflow_is_not_silently_dropped() {
        let mut pack = MemoryPack {
            conflicts_or_checks: vec![MemoryPackConflict {
                relation_id: "relation".to_owned(),
                relation_kind: "conflicts".to_owned(),
                reason_code: "explicit".to_owned(),
                source_ids: vec!["left".to_owned(), "right".to_owned()],
                left_observation_id: "left-observation".to_owned(),
                right_observation_id: "right-observation".to_owned(),
            }],
            evidence_gaps: vec![MemoryPackGap {
                code: "candidate_changed_before_response".to_owned(),
                pointer: None,
            }],
            diagnostics: vec!["candidate_changed_before_response".to_owned()],
            ..MemoryPack::default()
        };
        assert!(!enforce_final_budget(
            &mut pack,
            &MemoryPackRequest {
                max_bytes: 1,
                max_tokens: 1,
                ..MemoryPackRequest::default()
            }
        ));
        assert_eq!(pack.conflicts_or_checks.len(), 1);
        assert_eq!(pack.evidence_gaps.len(), 1);
        assert_eq!(pack.diagnostics.len(), 1);
    }
}
