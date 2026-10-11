//! Explicit material contracts for private evaluation artifacts and Provider input.
//! Production MemoryPack deliberately does not carry source excerpts.

use crate::{Budget, EvalError, EvalSource, canonical_hash};
use mcp_vault_memory::MemoryPack;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceExcerpt {
    evidence_ref_id: String,
    source_id: String,
    source_revision_id: String,
    source_path: String,
    spans: Vec<EvidenceSpan>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EvidenceSpan {
    role: EvidenceRole,
    start_byte: u64,
    end_byte: u64,
    content_hash: String,
    text: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceRole {
    Body,
    Context,
}

pub(super) fn retain_pack_excerpts(
    input: &Value,
    pack: &MemoryPack,
    projected: &mut Value,
) -> Result<(), EvalError> {
    let invalid = || EvalError::LiveConfig("memory pack evidence contract is invalid");
    for (section, entries) in [
        ("current_context", &pack.current_context),
        ("relevant_experiences", &pack.relevant_experiences),
    ] {
        for (index, entry) in entries.iter().enumerate() {
            let Some(value) = input[section][index].get("evidence_excerpts") else {
                continue;
            };
            let excerpts: Vec<EvidenceExcerpt> =
                serde_json::from_value(value.clone()).map_err(|_| invalid())?;
            let mut seen = std::collections::BTreeSet::new();
            for excerpt in &excerpts {
                if !entry.evidence_refs.contains(&excerpt.evidence_ref_id)
                    || !seen.insert(&excerpt.evidence_ref_id)
                    || !entry.source_references.iter().any(|source| {
                        source.source_id == excerpt.source_id
                            && source.source_revision_id == excerpt.source_revision_id
                            && source.source_path.as_deref() == Some(excerpt.source_path.as_str())
                    })
                    || excerpt.spans.is_empty()
                {
                    return Err(invalid());
                }
                for span in &excerpt.spans {
                    if span.text.is_empty()
                        || span.end_byte.checked_sub(span.start_byte)
                            != Some(span.text.len() as u64)
                        || span.content_hash
                            != format!("{:x}", Sha256::digest(span.text.as_bytes()))
                    {
                        return Err(invalid());
                    }
                }
            }
            if entry.evidence_refs.iter().any(|id| !seen.contains(id)) {
                return Err(invalid());
            }
            projected[section][index]["evidence_excerpts"] =
                serde_json::to_value(excerpts).map_err(|_| invalid())?;
        }
    }
    Ok(())
}

pub(super) fn check_pack_budget(pack: &Value, budget: &Budget) -> Result<(), EvalError> {
    let bytes = serde_json::to_vec(pack)
        .map_err(|_| EvalError::LiveConfig("memory pack serialization failed"))?;
    if bytes.len() > budget.max_bytes as usize
        || bytes.len().div_ceil(4) > budget.max_tokens as usize
        || pack["estimated_tokens"].as_u64().unwrap_or(u64::MAX) > u64::from(budget.max_tokens)
    {
        return Err(EvalError::LiveConfig(
            "memory pack evidence budget exhausted",
        ));
    }
    Ok(())
}

// Unknown adapter fields are omitted from BOTH the dispatched input and its
// artifact. Note snippets are authorized evaluation material; credentials,
// headers, templates and gold answers are not part of this contract.
#[derive(Deserialize, Serialize)]
struct OrdinaryInput {
    retrieval_strategy: String,
    task_id: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    index_profile_id: Option<String>,
    #[serde(default)]
    sources: Vec<OrdinarySource>,
    #[serde(default)]
    entry_count: u32,
    #[serde(default)]
    token_count: u32,
    coverage: OrdinaryCoverage,
    degradation_reasons: Vec<String>,
    #[serde(default)]
    quality_events: Vec<String>,
}

#[derive(Deserialize, Serialize)]
struct OrdinarySource {
    source_id: String,
    file_id: String,
    path: String,
    file_revision: u64,
    title: Option<String>,
    snippet: String,
    matched_section: Option<String>,
    score: f64,
}

#[derive(Deserialize, Serialize)]
struct OrdinaryCoverage {
    expected_source_ids: Vec<String>,
    current_indexed_source_ids: Vec<String>,
    expected_source_count: u32,
    current_indexed_source_count: u32,
    coverage_ratio: f64,
    complete: bool,
    candidate_count: u32,
    eligible_count: u32,
    available_result_count: u32,
    returned_hit_count: u32,
    stale_hit_count: u32,
}

pub(super) fn project_ordinary_input(
    input: &Value,
    sources: &[EvalSource],
) -> Result<Value, EvalError> {
    let invalid = || EvalError::LiveConfig("ordinary retrieval material contract is invalid");
    let input: OrdinaryInput = serde_json::from_value(input.clone()).map_err(|_| invalid())?;
    for hit in &input.sources {
        if !sources.iter().any(|source| {
            hit.source_id == source.logical_id
                && hit.file_id == source.file_id
                && hit.path == source.path
                && hit.file_revision == source.file_revision
        }) {
            return Err(invalid());
        }
    }
    serde_json::to_value(input).map_err(|_| invalid())
}

pub(super) fn ordinary_input_record(input: &Value) -> Result<Value, EvalError> {
    let hash = canonical_hash(input)
        .map_err(|_| EvalError::LiveConfig("ordinary retrieval material hash failed"))?;
    Ok(json!({
        "kind": "ordinary_answer_input", "arm": "A", "task_id": input["task_id"],
        "input_hash": hash, "input": input,
    }))
}
