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
    matched_section: Option<Vec<String>>,
    score: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<OrdinaryEvidence>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrdinaryEvidence {
    kind: OrdinaryEvidenceKind,
    source_content_hash: String,
    document_bytes: u64,
    spans: Vec<EvidenceSpan>,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum OrdinaryEvidenceKind {
    CompleteDocument,
    CompleteSection,
}

/// Prefer the complete canonical document. The bounded fallback is an entire
/// top-level content section plus the document preamble, never a text window.
/// Offsets come from parsing canonical Markdown, not indexed plain text.
pub(super) fn ordinary_evidence_options(
    source: &EvalSource,
    text: &str,
    query: &str,
) -> Result<Vec<Value>, String> {
    let span = |role, start: usize, end: usize| EvidenceSpan {
        role,
        start_byte: start as u64,
        end_byte: end as u64,
        content_hash: format!("{:x}", Sha256::digest(&text.as_bytes()[start..end])),
        text: text[start..end].to_owned(),
    };
    let evidence = |kind, spans| {
        serde_json::to_value(OrdinaryEvidence {
            kind,
            source_content_hash: source.content_hash.clone(),
            document_bytes: text.len() as u64,
            spans,
        })
        .map_err(|_| "ordinary_evidence_serialization".to_owned())
    };
    let mut options = vec![evidence(
        OrdinaryEvidenceKind::CompleteDocument,
        vec![span(EvidenceRole::Body, 0, text.len())],
    )?];
    let parsed = mcp_vault_indexer::analyze_markdown(
        mcp_vault_domain::FileId::parse(&source.file_id)
            .map_err(|_| "source_file_id_invalid".to_owned())?,
        mcp_vault_domain::VaultId::parse(&source.vault_id)
            .map_err(|_| "source_vault_id_invalid".to_owned())?,
        mcp_vault_domain::VaultPath::parse(&source.path)
            .map_err(|_| "source_path_invalid".to_owned())?,
        mcp_vault_domain::Revision::new(source.file_revision),
        &source.content_hash,
        text,
    )
    .map_err(|_| "ordinary_evidence_analysis_failed".to_owned())?;
    let headings = parsed.headings;
    let paths = headings
        .iter()
        .map(|heading| {
            serde_json::from_str::<Vec<String>>(&heading.heading_path_json)
                .map_err(|_| "ordinary_evidence_heading_invalid".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let has_document_title = headings.first().is_some_and(|heading| heading.level == 1)
        && headings.iter().filter(|heading| heading.level == 1).count() == 1;
    let first_content = usize::from(has_document_title);
    // Indexed heading hints are local labels, not canonical paths or offsets.
    // Rank whole canonical sections using the original query and unchanged
    // lexical admission. No expected answer, source ID or task-specific rule.
    let owner = paths
        .iter()
        .enumerate()
        .skip(first_content)
        .filter(|(_, path)| path.len() == first_content + 1)
        .filter_map(|(index, path)| {
            let heading = &headings[index];
            let start = heading.start_byte as usize;
            let end = headings
                .iter()
                .skip(index + 1)
                .find(|next| next.level <= heading.level)
                .map_or(text.len(), |next| next.start_byte as usize);
            let section = text.get(start..end)?;
            let relevance =
                mcp_vault_indexer::relevance::lexical_relevance(query, section, &[], path);
            relevance
                .admitted
                .then_some((index, start, end, relevance.coverage))
        })
        .max_by(|left, right| {
            left.3
                .total_cmp(&right.3)
                .then_with(|| right.0.cmp(&left.0))
        });
    let Some((_, start, end, _)) = owner else {
        return Ok(options);
    };
    let preamble_end = headings[first_content].start_byte as usize;
    if start >= end
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
        || !text.is_char_boundary(preamble_end)
    {
        return Ok(options);
    }
    let mut spans = Vec::new();
    if preamble_end > 0 {
        spans.push(span(EvidenceRole::Context, 0, preamble_end));
    }
    spans.push(span(EvidenceRole::Body, start, end));
    options.push(evidence(OrdinaryEvidenceKind::CompleteSection, spans)?);
    Ok(options)
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
        let source = sources
            .iter()
            .find(|source| {
                hit.source_id == source.logical_id
                    && hit.file_id == source.file_id
                    && hit.path == source.path
                    && hit.file_revision == source.file_revision
            })
            .ok_or_else(invalid)?;
        if let Some(evidence) = &hit.evidence {
            if evidence.source_content_hash != source.content_hash || evidence.spans.is_empty() {
                return Err(invalid());
            }
            let mut previous_end = 0;
            for span in &evidence.spans {
                if span.text.is_empty()
                    || span.start_byte < previous_end
                    || span.end_byte > evidence.document_bytes
                    || span.end_byte.checked_sub(span.start_byte) != Some(span.text.len() as u64)
                    || span.content_hash != format!("{:x}", Sha256::digest(span.text.as_bytes()))
                {
                    return Err(invalid());
                }
                previous_end = span.end_byte;
            }
            if evidence.kind == OrdinaryEvidenceKind::CompleteDocument
                && (evidence.spans.len() != 1
                    || evidence.spans[0].start_byte != 0
                    || evidence.spans[0].end_byte != evidence.document_bytes
                    || evidence.spans[0].content_hash != source.content_hash)
            {
                return Err(invalid());
            }
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
