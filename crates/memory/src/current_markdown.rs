//! Deterministic canonical Markdown for the current-memory model.

use std::collections::HashSet;

use mcp_vault_domain::{
    FileId, MemoryId, MemorySetId, MemorySourceId, ModelId, ProviderId, Revision, VaultId,
    VaultPath,
};
use mcp_vault_state::{
    CurrentMemoryBundle, CurrentMemoryOwnership, CurrentMemoryRecord, CurrentMemorySourceRecord,
    MemoryNoteSetRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use yaml_rust::{Yaml, YamlLoader};

use crate::{MemoryError, MemoryType, markdown};

const MAX_EXPLICIT_BYTES: usize = 256 * 1024;
const MAX_NOTE_SET_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalNoteItem {
    id: MemoryId,
    ordinal: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    importance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    confidence: Option<f64>,
    revision: Revision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_from: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    valid_to: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    entities: Vec<String>,
    content_hash: String,
    #[serde(default)]
    metadata: Value,
    created_at: i64,
    updated_at: i64,
}

/// Stable canonical path for one explicit current memory.
pub fn explicit_path(
    reserved_root: &VaultPath,
    memory_id: MemoryId,
) -> Result<VaultPath, MemoryError> {
    reserved_root
        .join(
            &VaultPath::parse(&format!("memory/current/explicit/{memory_id}.md"))
                .map_err(|_| MemoryError::InvalidInput("memory canonical path is invalid"))?,
        )
        .map_err(|_| MemoryError::InvalidInput("memory canonical path is invalid"))
}

/// Stable canonical path for the one set owned by a source File ID.
pub fn note_set_path(
    reserved_root: &VaultPath,
    source_file_id: FileId,
) -> Result<VaultPath, MemoryError> {
    reserved_root
        .join(
            &VaultPath::parse(&format!("memory/current/sources/{source_file_id}.md"))
                .map_err(|_| MemoryError::InvalidInput("memory-set canonical path is invalid"))?,
        )
        .map_err(|_| MemoryError::InvalidInput("memory-set canonical path is invalid"))
}

/// Render one explicit item. Optional metadata stays absent in frontmatter.
pub fn render_explicit(bundle: &CurrentMemoryBundle) -> Result<Vec<u8>, MemoryError> {
    let memory = &bundle.memory;
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str("schema: \"mcp-vault-memory/v2.1\"\n");
    output.push_str("ownership: \"explicit\"\n");
    output.push_str(&format!("id: {}\n", quote(&memory.id.to_string())));
    if let Some(kind) = memory.kind.as_deref() {
        output.push_str(&format!("kind: {}\n", quote(kind)));
    }
    if let Some(importance) = memory.importance {
        output.push_str(&format!("importance: {importance:.6}\n"));
    }
    if let Some(confidence) = memory.confidence {
        output.push_str(&format!("confidence: {confidence:.6}\n"));
    }
    output.push_str(&format!("origin: {}\n", quote(&memory.origin)));
    output.push_str(&format!("revision: {}\n", memory.revision.value()));
    output.push_str(&format!("created_at: {}\n", memory.created_at));
    write_optional_integer(&mut output, "valid_from", memory.valid_from);
    write_optional_integer(&mut output, "valid_to", memory.valid_to);
    write_string_list(&mut output, "tags", &memory.tags);
    write_string_list(&mut output, "entities", &memory.entities);
    output.push_str("sources:\n");
    for source in &bundle.sources {
        output.push_str(&format!(
            "  - source_type: {}\n",
            quote(&source.source_type)
        ));
        if let Some(file_id) = source.note_file_id {
            output.push_str(&format!("    file_id: {}\n", quote(&file_id.to_string())));
        }
        if let Some(path) = source.note_path.as_ref() {
            output.push_str(&format!("    path: {}\n", quote(path.as_str())));
        }
        if let Some(revision) = source.note_revision {
            output.push_str(&format!("    revision: {}\n", revision.value()));
        }
        if let Some(hash) = source.source_content_hash.as_deref() {
            output.push_str(&format!("    content_hash: {}\n", quote(hash)));
        }
        if !source.heading_path.is_empty() {
            output.push_str("    heading:\n");
            for heading in &source.heading_path {
                output.push_str(&format!("      - {}\n", quote(heading)));
            }
        }
        write_optional_indented_integer(&mut output, "start_line", source.start_line);
        write_optional_indented_integer(&mut output, "end_line", source.end_line);
        if let Some(hash) = source.excerpt_hash.as_deref() {
            output.push_str(&format!("    excerpt_hash: {}\n", quote(hash)));
        }
        if let Some(actor_id) = source.actor_id.as_deref() {
            output.push_str(&format!("    actor_id: {}\n", quote(actor_id)));
        }
    }
    output.push_str(&format!(
        "metadata: {}\n",
        quote(
            &serde_json::to_string(&memory.metadata)
                .map_err(|_| MemoryError::InvalidInput("memory metadata is invalid"))?
        )
    ));
    output.push_str("---\n\n");
    output.push_str(memory.content.trim());
    output.push('\n');
    Ok(output.into_bytes())
}

/// Render one source-owned set as a single portable Markdown document.
pub fn render_note_set(
    set: &MemoryNoteSetRecord,
    items: &[CurrentMemoryBundle],
) -> Result<Vec<u8>, MemoryError> {
    let mut ordered = items.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|item| (item.memory.ordinal, item.memory.id));
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str("schema: \"mcp-vault-memory-set/v2.1\"\n");
    output.push_str(&format!("set_id: {}\n", quote(&set.id.to_string())));
    output.push_str(&format!(
        "source_file_id: {}\n",
        quote(&set.source_file_id.to_string())
    ));
    output.push_str(&format!(
        "source_path: {}\n",
        quote(set.source_path.as_str())
    ));
    output.push_str(&format!(
        "source_content_hash: {}\n",
        quote(&set.source_content_hash)
    ));
    output.push_str(&format!(
        "source_revision: {}\n",
        set.source_revision.value()
    ));
    output.push_str(&format!("set_revision: {}\n", set.set_revision.value()));
    output.push_str(&format!("extraction_paused: {}\n", set.extraction_paused));
    output.push_str(&format!("profile_hash: {}\n", quote(&set.profile_hash)));
    output.push_str(&format!("prompt_version: {}\n", quote(&set.prompt_version)));
    if let Some(provider_id) = set.provider_id {
        output.push_str(&format!(
            "provider_id: {}\n",
            quote(&provider_id.to_string())
        ));
    }
    if let Some(model_id) = set.model_id {
        output.push_str(&format!("model_id: {}\n", quote(&model_id.to_string())));
    }
    output.push_str(&format!("created_at: {}\n", set.created_at));
    output.push_str(&format!("updated_at: {}\n", set.updated_at));
    output.push_str("---\n\n");
    output.push_str(&format!(
        "# Current memories from `{}`\n\n",
        set.source_path.as_str().replace('`', "\\`")
    ));
    output.push_str(
        "The JSON block is the complete current set. It is deterministic and rebuildable.\n\n",
    );
    let canonical_items = ordered
        .into_iter()
        .map(|item| {
            let memory = &item.memory;
            CanonicalNoteItem {
                id: memory.id,
                ordinal: memory.ordinal.unwrap_or_default(),
                kind: memory.kind.clone(),
                content: memory.content.trim().to_owned(),
                importance: memory.importance,
                confidence: memory.confidence,
                revision: memory.revision,
                valid_from: memory.valid_from,
                valid_to: memory.valid_to,
                tags: memory.tags.clone(),
                entities: memory.entities.clone(),
                content_hash: memory.content_hash.clone(),
                metadata: memory.metadata.clone(),
                created_at: memory.created_at,
                updated_at: memory.updated_at,
            }
        })
        .collect::<Vec<_>>();
    output.push_str("```json\n");
    output.push_str(
        &serde_json::to_string_pretty(&canonical_items)
            .map_err(|_| MemoryError::InvalidInput("memory-set metadata is invalid"))?,
    );
    output.push_str("\n```\n");
    Ok(output.into_bytes())
}

/// Parse one independently owned current memory from its canonical Markdown.
/// The filename and embedded ID must agree; callers supply the authoritative
/// current file identity/revision observed through Vault Core.
pub fn parse_explicit(
    bytes: &[u8],
    path: &VaultPath,
    vault_id: VaultId,
    canonical_file_id: FileId,
    canonical_revision: Revision,
) -> Result<CurrentMemoryBundle, MemoryError> {
    let (root, body) = parse_document(bytes, MAX_EXPLICIT_BYTES)?;
    if required_string(&root, "schema")? != "mcp-vault-memory/v2.1"
        || required_string(&root, "ownership")? != "explicit"
        || body.is_empty()
        || body.len() > 64 * 1024
    {
        return Err(MemoryError::Markdown);
    }
    let id = MemoryId::parse(&required_string(&root, "id")?).map_err(|_| MemoryError::Markdown)?;
    let filename_id = path
        .file_name()
        .and_then(|value| value.strip_suffix(".md"))
        .ok_or(MemoryError::Markdown)
        .and_then(|value| MemoryId::parse(value).map_err(|_| MemoryError::Markdown))?;
    if id != filename_id {
        return Err(MemoryError::Markdown);
    }
    let kind = optional_string(&root, "kind");
    if kind
        .as_deref()
        .is_some_and(|kind| MemoryType::try_from(kind).is_err())
    {
        return Err(MemoryError::Markdown);
    }
    let importance = optional_float(&root, "importance")?;
    let confidence = optional_float(&root, "confidence")?;
    validate_optional_score(importance)?;
    validate_optional_score(confidence)?;
    let origin = required_string(&root, "origin")?;
    if !matches!(
        origin.as_str(),
        "explicit_agent" | "explicit_admin" | "import"
    ) {
        return Err(MemoryError::Markdown);
    }
    let revision = required_revision(&root, "revision")?;
    let created_at = required_integer(&root, "created_at")?;
    let updated_at = optional_integer(&root, "updated_at")?.unwrap_or(created_at);
    let valid_from = optional_integer(&root, "valid_from")?;
    let valid_to = optional_integer(&root, "valid_to")?;
    if matches!((valid_from, valid_to), (Some(from), Some(to)) if from >= to) {
        return Err(MemoryError::Markdown);
    }
    let tags = string_list(&root, "tags")?;
    let entities = string_list(&root, "entities")?;
    let metadata = optional_string(&root, "metadata")
        .map(|value| serde_json::from_str(&value).map_err(|_| MemoryError::Markdown))
        .transpose()?
        .unwrap_or_else(|| Value::Object(Default::default()));
    let sources = parse_sources(&root, vault_id, id, created_at)?;
    if sources.is_empty() {
        return Err(MemoryError::Markdown);
    }
    let normalized_content = markdown::normalize_content(&body);
    let content_hash = markdown::hash_content(&normalized_content);
    Ok(CurrentMemoryBundle {
        memory: CurrentMemoryRecord {
            id,
            vault_id,
            ownership: CurrentMemoryOwnership::Explicit,
            note_set_id: None,
            ordinal: None,
            kind,
            content: body,
            normalized_content,
            content_hash,
            importance,
            confidence,
            origin,
            revision,
            canonical_file_id: Some(canonical_file_id),
            canonical_path: Some(path.clone()),
            canonical_revision: Some(canonical_revision),
            valid_from,
            valid_to,
            tags,
            entities,
            metadata,
            created_at,
            updated_at,
            last_recalled_at: None,
            recall_count: 0,
        },
        sources,
        note_set: None,
    })
}

/// Parse a source-owned current set and every item from its deterministic JSON
/// block. Item content lives only in that block, so untrusted Markdown text
/// cannot forge a delimiter, identity, or additional object during rebuild.
pub fn parse_note_set(
    bytes: &[u8],
    path: &VaultPath,
    vault_id: VaultId,
    canonical_file_id: FileId,
    canonical_revision: Revision,
    now: i64,
) -> Result<(MemoryNoteSetRecord, Vec<CurrentMemoryBundle>), MemoryError> {
    let (root, body) = parse_document(bytes, MAX_NOTE_SET_BYTES)?;
    if required_string(&root, "schema")? != "mcp-vault-memory-set/v2.1" {
        return Err(MemoryError::Markdown);
    }
    let set_id = MemorySetId::parse(&required_string(&root, "set_id")?)
        .map_err(|_| MemoryError::Markdown)?;
    let source_file_id = FileId::parse(&required_string(&root, "source_file_id")?)
        .map_err(|_| MemoryError::Markdown)?;
    let filename_id = path
        .file_name()
        .and_then(|value| value.strip_suffix(".md"))
        .ok_or(MemoryError::Markdown)
        .and_then(|value| FileId::parse(value).map_err(|_| MemoryError::Markdown))?;
    if source_file_id != filename_id {
        return Err(MemoryError::Markdown);
    }
    let source_path = VaultPath::parse(&required_string(&root, "source_path")?)
        .map_err(|_| MemoryError::Markdown)?;
    let source_content_hash = required_string(&root, "source_content_hash")?;
    let source_revision = required_revision(&root, "source_revision")?;
    let set_revision = required_revision(&root, "set_revision")?;
    let extraction_paused = required_bool(&root, "extraction_paused")?;
    let profile_hash = required_string(&root, "profile_hash")?;
    let prompt_version = required_string(&root, "prompt_version")?;
    if source_content_hash.trim().is_empty()
        || profile_hash.trim().is_empty()
        || prompt_version.trim().is_empty()
    {
        return Err(MemoryError::Markdown);
    }
    let provider_id = optional_string(&root, "provider_id")
        .map(|value| ProviderId::parse(&value).map_err(|_| MemoryError::Markdown))
        .transpose()?;
    let model_id = optional_string(&root, "model_id")
        .map(|value| ModelId::parse(&value).map_err(|_| MemoryError::Markdown))
        .transpose()?;
    let created_at = optional_integer(&root, "created_at")?.unwrap_or(now);
    let updated_at = optional_integer(&root, "updated_at")?.unwrap_or(created_at);
    let set = MemoryNoteSetRecord {
        id: set_id,
        vault_id,
        source_file_id,
        source_path: source_path.clone(),
        source_content_hash: source_content_hash.clone(),
        source_revision,
        set_revision,
        extraction_paused,
        canonical_file_id,
        canonical_path: path.clone(),
        canonical_revision,
        profile_hash: profile_hash.clone(),
        prompt_version: prompt_version.clone(),
        provider_id,
        model_id,
        created_at,
        updated_at,
    };

    let json_start = body.find("```json\n").ok_or(MemoryError::Markdown)? + "```json\n".len();
    let after_start = &body[json_start..];
    let json_end = after_start.find("\n```").ok_or(MemoryError::Markdown)?;
    if !after_start[json_end + "\n```".len()..].trim().is_empty() {
        return Err(MemoryError::Markdown);
    }
    let items: Vec<CanonicalNoteItem> =
        serde_json::from_str(&after_start[..json_end]).map_err(|_| MemoryError::Markdown)?;
    if items.len() > 256 {
        return Err(MemoryError::Markdown);
    }
    let mut ids = HashSet::with_capacity(items.len());
    let mut bundles = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        if !ids.insert(item.id)
            || item.ordinal != u32::try_from(index).map_err(|_| MemoryError::Markdown)?
            || item.revision.value() == 0
            || item.content.trim().is_empty()
            || item.content.len() > 64 * 1024
            || item.tags.len() > 128
            || item.entities.len() > 128
            || item
                .kind
                .as_deref()
                .is_some_and(|kind| MemoryType::try_from(kind).is_err())
            || matches!((item.valid_from, item.valid_to), (Some(from), Some(to)) if from >= to)
        {
            return Err(MemoryError::Markdown);
        }
        validate_optional_score(item.importance)?;
        validate_optional_score(item.confidence)?;
        let content = item.content.trim().to_owned();
        let normalized_content = markdown::normalize_content(&content);
        if markdown::hash_content(&normalized_content) != item.content_hash {
            return Err(MemoryError::Markdown);
        }
        let source = CurrentMemorySourceRecord {
            id: MemorySourceId::new(),
            vault_id,
            memory_id: item.id,
            source_type: "note".to_owned(),
            note_file_id: Some(source_file_id),
            note_path: Some(source_path.clone()),
            note_revision: Some(source_revision),
            source_content_hash: Some(source_content_hash.clone()),
            heading_path: Vec::new(),
            start_line: None,
            end_line: None,
            excerpt_hash: Some(source_content_hash.clone()),
            actor_id: None,
            created_at: item.created_at,
        };
        bundles.push(CurrentMemoryBundle {
            memory: CurrentMemoryRecord {
                id: item.id,
                vault_id,
                ownership: CurrentMemoryOwnership::NoteDerived,
                note_set_id: Some(set_id),
                ordinal: Some(item.ordinal),
                kind: item.kind,
                content,
                normalized_content,
                content_hash: item.content_hash,
                importance: item.importance,
                confidence: item.confidence,
                origin: "note_extracted".to_owned(),
                revision: item.revision,
                canonical_file_id: None,
                canonical_path: None,
                canonical_revision: None,
                valid_from: item.valid_from,
                valid_to: item.valid_to,
                tags: item.tags,
                entities: item.entities,
                metadata: item.metadata,
                created_at: item.created_at,
                updated_at: item.updated_at,
                last_recalled_at: None,
                recall_count: 0,
            },
            sources: vec![source],
            note_set: Some(set.clone()),
        });
    }
    Ok((set, bundles))
}

/// Hash exact canonical bytes for crash-safe snapshot adoption.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn write_optional_integer(output: &mut String, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        output.push_str(&format!("{key}: {value}\n"));
    }
}

fn write_optional_indented_integer(output: &mut String, key: &str, value: Option<u32>) {
    if let Some(value) = value {
        output.push_str(&format!("    {key}: {value}\n"));
    }
}

fn write_string_list(output: &mut String, key: &str, values: &[String]) {
    output.push_str(&format!("{key}:\n"));
    for value in values {
        output.push_str(&format!("  - {}\n", quote(value)));
    }
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_document(bytes: &[u8], limit: usize) -> Result<(Yaml, String), MemoryError> {
    if bytes.len() > limit {
        return Err(MemoryError::InvalidInput(
            "managed current-memory file is too large",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| MemoryError::Markdown)?;
    let rest = text.strip_prefix("---\n").ok_or(MemoryError::Markdown)?;
    let end = rest.find("\n---\n").ok_or(MemoryError::Markdown)?;
    let documents = YamlLoader::load_from_str(&rest[..end]).map_err(|_| MemoryError::Markdown)?;
    let root = documents.first().cloned().ok_or(MemoryError::Markdown)?;
    let body = rest[end + "\n---\n".len()..].trim().to_owned();
    Ok((root, body))
}

fn field<'a>(root: &'a Yaml, key: &str) -> Option<&'a Yaml> {
    root.as_hash()
        .and_then(|hash| hash.get(&Yaml::String(key.to_owned())))
}

fn required_string(root: &Yaml, key: &str) -> Result<String, MemoryError> {
    field(root, key)
        .and_then(Yaml::as_str)
        .map(str::to_owned)
        .ok_or(MemoryError::Markdown)
}

fn optional_string(root: &Yaml, key: &str) -> Option<String> {
    field(root, key).and_then(Yaml::as_str).map(str::to_owned)
}

fn required_integer(root: &Yaml, key: &str) -> Result<i64, MemoryError> {
    field(root, key)
        .and_then(Yaml::as_i64)
        .ok_or(MemoryError::Markdown)
}

fn optional_integer(root: &Yaml, key: &str) -> Result<Option<i64>, MemoryError> {
    match field(root, key) {
        None | Some(Yaml::Null) | Some(Yaml::BadValue) => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or(MemoryError::Markdown),
    }
}

fn required_revision(root: &Yaml, key: &str) -> Result<Revision, MemoryError> {
    let value = required_integer(root, key)?;
    let value = u64::try_from(value).map_err(|_| MemoryError::Markdown)?;
    if value == 0 {
        return Err(MemoryError::Markdown);
    }
    Ok(Revision::new(value))
}

fn required_bool(root: &Yaml, key: &str) -> Result<bool, MemoryError> {
    field(root, key)
        .and_then(Yaml::as_bool)
        .ok_or(MemoryError::Markdown)
}

fn optional_float(root: &Yaml, key: &str) -> Result<Option<f64>, MemoryError> {
    match field(root, key) {
        None | Some(Yaml::Null) | Some(Yaml::BadValue) => Ok(None),
        Some(value) => value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .map(Some)
            .ok_or(MemoryError::Markdown),
    }
}

fn validate_optional_score(value: Option<f64>) -> Result<(), MemoryError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return Err(MemoryError::Markdown);
    }
    Ok(())
}

fn string_list(root: &Yaml, key: &str) -> Result<Vec<String>, MemoryError> {
    let Some(value) = field(root, key) else {
        return Ok(Vec::new());
    };
    let values = value.as_vec().ok_or(MemoryError::Markdown)?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or(MemoryError::Markdown)
        })
        .collect()
}

fn parse_sources(
    root: &Yaml,
    vault_id: VaultId,
    memory_id: MemoryId,
    created_at: i64,
) -> Result<Vec<CurrentMemorySourceRecord>, MemoryError> {
    let values = field(root, "sources")
        .and_then(Yaml::as_vec)
        .ok_or(MemoryError::Markdown)?;
    if values.len() > 128 {
        return Err(MemoryError::Markdown);
    }
    values
        .iter()
        .map(|value| {
            let source_type = required_string(value, "source_type")?;
            if !matches!(
                source_type.as_str(),
                "note" | "explicit_agent" | "explicit_admin" | "import"
            ) {
                return Err(MemoryError::Markdown);
            }
            let note_file_id = optional_string(value, "file_id")
                .map(|value| FileId::parse(&value).map_err(|_| MemoryError::Markdown))
                .transpose()?;
            let note_path = optional_string(value, "path")
                .map(|value| VaultPath::parse(&value).map_err(|_| MemoryError::Markdown))
                .transpose()?;
            let note_revision = optional_integer(value, "revision")?
                .map(|value| {
                    u64::try_from(value)
                        .map(Revision::new)
                        .map_err(|_| MemoryError::Markdown)
                })
                .transpose()?;
            let start_line = optional_integer(value, "start_line")?
                .map(|value| u32::try_from(value).map_err(|_| MemoryError::Markdown))
                .transpose()?;
            let end_line = optional_integer(value, "end_line")?
                .map(|value| u32::try_from(value).map_err(|_| MemoryError::Markdown))
                .transpose()?;
            Ok(CurrentMemorySourceRecord {
                id: MemorySourceId::new(),
                vault_id,
                memory_id,
                source_type,
                note_file_id,
                note_path,
                note_revision,
                source_content_hash: optional_string(value, "content_hash"),
                heading_path: string_list(value, "heading")?,
                start_line,
                end_line,
                excerpt_hash: optional_string(value, "excerpt_hash"),
                actor_id: optional_string(value, "actor_id"),
                created_at,
            })
        })
        .collect()
}
