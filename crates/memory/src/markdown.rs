//! Deterministic canonical memory Markdown serialization and bounded parsing.

use mcp_vault_domain::{MemoryId, MemoryRelationId, Revision, VaultId, VaultPath};
use mcp_vault_state::{MemoryBundle, MemoryRecord};
use serde_json::{Value, json};
use yaml_rust::{Yaml, YamlLoader};

use crate::{MemoryError, MemoryOrigin, MemoryStatus, MemoryType};

/// Parsed managed Markdown record before projection reconciliation.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedMemoryMarkdown {
    /// Stable memory identity.
    pub id: MemoryId,
    /// Memory type.
    pub memory_type: MemoryType,
    /// Lifecycle status.
    pub status: MemoryStatus,
    /// Proposition body.
    pub content: String,
    /// Importance.
    pub importance: f64,
    /// Confidence.
    pub confidence: f64,
    /// Origin.
    pub origin: MemoryOrigin,
    /// Validity start.
    pub valid_from: Option<i64>,
    /// Validity end.
    pub valid_to: Option<i64>,
    /// Entities.
    pub entities: Vec<String>,
    /// Tags.
    pub tags: Vec<String>,
    /// Source metadata represented as JSON-compatible values.
    pub sources: Vec<Value>,
    /// Extraction metadata.
    pub extraction: Value,
    /// Explicit supersession targets.
    pub supersedes: Vec<MemoryId>,
}

/// Build a deterministic canonical path under the reserved memory namespace.
pub fn canonical_path(
    reserved_root: &VaultPath,
    memory_id: MemoryId,
    created_at_millis: i64,
) -> Result<VaultPath, MemoryError> {
    let (year, month) = utc_year_month(created_at_millis);
    reserved_root
        .join(
            &VaultPath::parse(&format!(
                "memory/records/{year:04}/{month:02}/{memory_id}.md"
            ))
            .map_err(|_| MemoryError::InvalidInput("memory canonical path is invalid"))?,
        )
        .map_err(|_| MemoryError::InvalidInput("memory canonical path is invalid"))
}

/// Render one canonical memory record with stable field ordering.
pub fn render(bundle: &MemoryBundle) -> Result<String, MemoryError> {
    let memory = &bundle.memory;
    let memory_type = MemoryType::try_from(memory.memory_type.as_str())
        .map_err(|_| MemoryError::InvalidInput("memory type is invalid"))?;
    let status = MemoryStatus::try_from(memory.status.as_str())
        .map_err(|_| MemoryError::InvalidInput("memory status is invalid"))?;
    let origin = parse_origin(&memory.origin)?;
    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("id: {}\n", yaml_quote(&memory.id.to_string())));
    output.push_str(&format!("type: {}\n", yaml_quote(memory_type.as_str())));
    output.push_str(&format!("status: {}\n", yaml_quote(status.as_str())));
    output.push_str(&format!("importance: {:.6}\n", memory.importance));
    output.push_str(&format!("confidence: {:.6}\n", memory.confidence));
    output.push_str(&format!("origin: {}\n", yaml_quote(origin.as_str())));
    output.push_str(&format!("revision: {}\n", memory.revision.value()));
    output.push_str(&format!("created_at: {}\n", memory.created_at));
    output.push_str(&format!("updated_at: {}\n", memory.updated_at));
    output.push_str(&format_option("valid_from", memory.valid_from));
    output.push_str(&format_option("valid_to", memory.valid_to));
    write_string_list(&mut output, "entities", &bundle.entities);
    write_string_list(&mut output, "tags", &bundle.tags);
    output.push_str("supersedes:\n");
    for relation in bundle
        .relations
        .iter()
        .filter(|relation| relation.relation_type == "supersedes")
    {
        output.push_str(&format!(
            "  - {}\n",
            yaml_quote(&relation.target_memory_id.to_string())
        ));
    }
    output.push_str("sources:\n");
    for source in &bundle.sources {
        output.push_str("  - source_type: ");
        output.push_str(&yaml_quote(&source.source_type));
        output.push('\n');
        if let Some(path) = &source.note_path {
            output.push_str(&format!("    path: {}\n", yaml_quote(path.as_str())));
        }
        if let Some(revision) = source.note_revision {
            output.push_str(&format!("    revision: {}\n", revision.value()));
        }
        if !source.heading_path.is_empty() {
            output.push_str("    heading:\n");
            for heading in &source.heading_path {
                output.push_str(&format!("      - {}\n", yaml_quote(heading)));
            }
        }
        if let Some(line) = source.start_line {
            output.push_str(&format!("    start_line: {line}\n"));
        }
        if let Some(line) = source.end_line {
            output.push_str(&format!("    end_line: {line}\n"));
        }
        if let Some(hash) = &source.excerpt_hash {
            output.push_str(&format!("    excerpt_hash: {}\n", yaml_quote(hash)));
        }
    }
    output.push_str("extraction: ");
    output.push_str(&yaml_quote(
        &serde_json::to_string(&memory.extraction)
            .map_err(|_| MemoryError::InvalidInput("memory extraction metadata is invalid"))?,
    ));
    output.push('\n');
    output.push_str("---\n\n");
    output.push_str(memory.content.trim());
    output.push('\n');
    Ok(output)
}

/// Parse one bounded canonical Markdown record.
pub fn parse(bytes: &[u8], path: &VaultPath) -> Result<ParsedMemoryMarkdown, MemoryError> {
    if bytes.len() > 256 * 1024 {
        return Err(MemoryError::InvalidInput(
            "managed memory file is too large",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| MemoryError::Markdown)?;
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err(MemoryError::Markdown);
    };
    let Some(end) = rest.find("\n---\n") else {
        return Err(MemoryError::Markdown);
    };
    let frontmatter = &rest[..end];
    let body = rest[end + "\n---\n".len()..].trim().to_owned();
    if body.is_empty() {
        return Err(MemoryError::InvalidInput("managed memory body is empty"));
    }
    let document = YamlLoader::load_from_str(frontmatter).map_err(|_| MemoryError::Markdown)?;
    let root = document.first().ok_or(MemoryError::Markdown)?;
    let id = MemoryId::parse(required_string(root, "id")?.as_str())
        .map_err(|_| MemoryError::Markdown)?;
    let filename_id = path
        .file_name()
        .and_then(|value| value.strip_suffix(".md"))
        .ok_or(MemoryError::Markdown)
        .and_then(|value| MemoryId::parse(value).map_err(|_| MemoryError::Markdown))?;
    if id != filename_id {
        return Err(MemoryError::Markdown);
    }
    let memory_type = MemoryType::try_from(required_string(root, "type")?.as_str())
        .map_err(|_| MemoryError::Markdown)?;
    let status = MemoryStatus::try_from(required_string(root, "status")?.as_str())
        .map_err(|_| MemoryError::Markdown)?;
    let origin = parse_origin(required_string(root, "origin")?.as_str())?;
    let importance = required_float(root, "importance")?;
    let confidence = required_float(root, "confidence")?;
    if !(0.0..=1.0).contains(&importance) || !(0.0..=1.0).contains(&confidence) {
        return Err(MemoryError::Markdown);
    }
    let entities = string_list(root, "entities")?;
    let tags = string_list(root, "tags")?;
    let supersedes = memory_id_list(root, "supersedes")?;
    let sources = root
        .as_hash()
        .and_then(|hash| hash.get(&Yaml::String("sources".to_owned())))
        .and_then(Yaml::as_vec)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(yaml_source_to_json)
        .collect::<Result<Vec<_>, _>>()?;
    let extraction = root
        .as_hash()
        .and_then(|hash| hash.get(&Yaml::String("extraction".to_owned())))
        .and_then(Yaml::as_str)
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_else(|| json!({}));
    Ok(ParsedMemoryMarkdown {
        id,
        memory_type,
        status,
        content: body,
        importance,
        confidence,
        origin,
        valid_from: optional_integer(root, "valid_from")?,
        valid_to: optional_integer(root, "valid_to")?,
        entities,
        tags,
        sources,
        extraction,
        supersedes,
    })
}

/// Convert a parsed managed record into a projection skeleton.
pub fn projection(
    parsed: ParsedMemoryMarkdown,
    vault_id: VaultId,
    path: VaultPath,
    canonical_file_id: Option<mcp_vault_domain::FileId>,
    canonical_revision: Option<Revision>,
    now: i64,
) -> Result<MemoryBundle, MemoryError> {
    let normalized_content = normalize_content(&parsed.content);
    let content_hash = hash_content(&normalized_content);
    let sources = parsed
        .sources
        .iter()
        .map(|source| source_to_state(vault_id, parsed.id, source, now))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MemoryBundle {
        memory: MemoryRecord {
            id: parsed.id,
            vault_id,
            memory_type: parsed.memory_type.as_str().to_owned(),
            status: parsed.status.as_str().to_owned(),
            content: parsed.content,
            normalized_content,
            content_hash,
            importance: parsed.importance,
            confidence: parsed.confidence,
            origin: parsed.origin.as_str().to_owned(),
            revision: Revision::new(1),
            canonical_file_id,
            canonical_path: Some(path),
            canonical_revision,
            valid_from: parsed.valid_from,
            valid_to: parsed.valid_to,
            extraction: parsed.extraction,
            created_at: now,
            updated_at: now,
            last_recalled_at: None,
            recall_count: 0,
        },
        sources,
        entities: parsed.entities,
        tags: parsed.tags,
        relations: parsed
            .supersedes
            .into_iter()
            .map(|target_memory_id| mcp_vault_state::MemoryRelationRecord {
                id: MemoryRelationId::new(),
                vault_id,
                source_memory_id: parsed.id,
                target_memory_id,
                relation_type: "supersedes".to_owned(),
                confidence: 1.0,
                created_at: now,
            })
            .collect(),
    })
}

/// Normalize proposition text for duplicate identity.
pub fn normalize_content(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Return a stable content hash with the project prefix.
pub fn hash_content(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

fn source_to_state(
    vault_id: VaultId,
    memory_id: MemoryId,
    value: &Value,
    now: i64,
) -> Result<mcp_vault_state::MemorySourceRecord, MemoryError> {
    let object = value.as_object().ok_or(MemoryError::Markdown)?;
    let source_type = object
        .get("source_type")
        .and_then(Value::as_str)
        .unwrap_or("direct_markdown")
        .to_owned();
    let note_path = object
        .get("path")
        .and_then(Value::as_str)
        .map(VaultPath::parse)
        .transpose()
        .map_err(|_| MemoryError::Markdown)?;
    let heading_path = object
        .get("heading")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(mcp_vault_state::MemorySourceRecord {
        id: mcp_vault_domain::MemorySourceId::new(),
        vault_id,
        memory_id,
        source_type,
        note_file_id: None,
        note_path,
        note_revision: object
            .get("revision")
            .and_then(Value::as_u64)
            .map(Revision::new),
        heading_path,
        start_line: object
            .get("start_line")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        end_line: object
            .get("end_line")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok()),
        excerpt_hash: object
            .get("excerpt_hash")
            .and_then(Value::as_str)
            .map(str::to_owned),
        actor_id: None,
        created_at: now,
    })
}

fn required_string(root: &Yaml, key: &str) -> Result<String, MemoryError> {
    root.as_hash()
        .and_then(|hash| hash.get(&Yaml::String(key.to_owned())))
        .and_then(Yaml::as_str)
        .map(str::to_owned)
        .ok_or(MemoryError::Markdown)
}

fn required_float(root: &Yaml, key: &str) -> Result<f64, MemoryError> {
    root.as_hash()
        .and_then(|hash| hash.get(&Yaml::String(key.to_owned())))
        .and_then(|value| value.as_f64().or_else(|| value.as_i64().map(|v| v as f64)))
        .ok_or(MemoryError::Markdown)
}

fn optional_integer(root: &Yaml, key: &str) -> Result<Option<i64>, MemoryError> {
    Ok(root
        .as_hash()
        .and_then(|hash| hash.get(&Yaml::String(key.to_owned())))
        .and_then(|value| value.as_i64()))
}

fn string_list(root: &Yaml, key: &str) -> Result<Vec<String>, MemoryError> {
    Ok(root
        .as_hash()
        .and_then(|hash| hash.get(&Yaml::String(key.to_owned())))
        .and_then(Yaml::as_vec)
        .map(|values| {
            values
                .iter()
                .filter_map(Yaml::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default())
}

fn memory_id_list(root: &Yaml, key: &str) -> Result<Vec<MemoryId>, MemoryError> {
    string_list(root, key)?
        .into_iter()
        .map(|value| MemoryId::parse(&value).map_err(|_| MemoryError::Markdown))
        .collect()
}

fn yaml_source_to_json(value: Yaml) -> Result<Value, MemoryError> {
    let hash = value.as_hash().ok_or(MemoryError::Markdown)?;
    let mut object = serde_json::Map::new();
    for (key, value) in hash {
        let Some(key) = key.as_str() else {
            continue;
        };
        if let Some(value) = value.as_str() {
            object.insert(key.to_owned(), Value::String(value.to_owned()));
        } else if let Some(value) = value.as_i64() {
            object.insert(key.to_owned(), Value::Number(value.into()));
        } else if let Some(values) = value.as_vec() {
            object.insert(
                key.to_owned(),
                Value::Array(
                    values
                        .iter()
                        .filter_map(Yaml::as_str)
                        .map(|value| Value::String(value.to_owned()))
                        .collect(),
                ),
            );
        }
    }
    Ok(Value::Object(object))
}

fn write_string_list(output: &mut String, key: &str, values: &[String]) {
    output.push_str(&format!("{key}:\n"));
    for value in values {
        output.push_str(&format!("  - {}\n", yaml_quote(value)));
    }
}

fn format_option(key: &str, value: Option<i64>) -> String {
    value.map_or_else(|| format!("{key}:\n"), |value| format!("{key}: {value}\n"))
}

fn yaml_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_origin(value: &str) -> Result<MemoryOrigin, MemoryError> {
    match value {
        "extracted" => Ok(MemoryOrigin::Extracted),
        "explicit_agent" => Ok(MemoryOrigin::ExplicitAgent),
        "explicit_admin" => Ok(MemoryOrigin::ExplicitAdmin),
        "direct_markdown" => Ok(MemoryOrigin::DirectMarkdown),
        "import" => Ok(MemoryOrigin::Import),
        _ => Err(MemoryError::Markdown),
    }
}

fn utc_year_month(millis: i64) -> (i64, i64) {
    let days = millis.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era.div_euclid(1_460) + day_of_era.div_euclid(36_524)
        - day_of_era.div_euclid(146_096))
    .div_euclid(365);
    let year = year_of_era + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era.div_euclid(4) - year_of_era.div_euclid(100));
    let month = (5 * day_of_year + 2).div_euclid(153);
    let year = year + if month >= 10 { 1 } else { 0 };
    let month = month + if month < 10 { 3 } else { -9 };
    (year, month)
}
