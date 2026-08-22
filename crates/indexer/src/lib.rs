//! Rebuildable Markdown, FTS, link, and knowledge-map projections.
//!
//! Canonical bytes are read through Vault Core. SQL projection writes and
//! queries are delegated to the state repository.

use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use comrak::{
    Arena, Options,
    nodes::{AstNode, NodeValue},
    parse_document,
};
use mcp_vault_core::{VaultCore, VaultError};
use mcp_vault_domain::{FileId, Revision, VaultContext, VaultId, VaultPath};
use mcp_vault_state::{
    EntryType, FileRecord, HeadingProjectionInput, IndexMembershipProjectionInput,
    IndexNodeProjectionInput, IndexNodeRecord, IndexRepository, IndexStatusRecord,
    LinkProjectionInput, NoteProjectionInput, NoteSearchRecord, StateError, TagProjectionInput,
};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::AsyncReadExt;
use yaml_rust::{Yaml, YamlLoader};

/// Current projection schema version.
pub const ANALYZER_VERSION: u32 = 1;
/// Maximum canonical Markdown payload analyzed in one request/job.
pub const MAX_NOTE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum frontmatter/taxonomy payload decoded as YAML.
pub const MAX_YAML_BYTES: usize = 256 * 1024;
/// Maximum YAML nesting depth accepted by the bounded decoder.
pub const MAX_YAML_DEPTH: usize = 16;
/// Maximum YAML scalar/collection nodes decoded.
pub const MAX_YAML_NODES: usize = 4096;

/// Errors exposed by the index application boundary.
#[derive(Debug, Error)]
pub enum IndexError {
    /// Canonical Core read failed.
    #[error("index source read failed")]
    Core(#[from] VaultError),
    /// Projection state failed.
    #[error("index projection state failed")]
    State(#[from] StateError),
    /// Markdown input exceeded the analyzer bound.
    #[error("Markdown input is too large")]
    TooLarge,
    /// Markdown/index input was not valid for the requested operation.
    #[error("invalid index input: {0}")]
    InvalidInput(&'static str),
    /// Bounded taxonomy/frontmatter decoding failed.
    #[error("bounded YAML decoding failed")]
    Yaml,
}

/// A complete parsed note projection ready for state replacement.
#[derive(Clone, Debug, PartialEq)]
pub struct AnalyzedNote {
    /// Note metadata projection.
    pub note: NoteProjectionInput,
    /// Heading projections.
    pub headings: Vec<HeadingProjectionInput>,
    /// Tag projections.
    pub tags: Vec<TagProjectionInput>,
    /// Link projections with unresolved targets.
    pub links: Vec<LinkProjectionInput>,
}

/// One validated, portable manual taxonomy topic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaxonomyTopic {
    /// Stable topic identifier supplied by the taxonomy file or derived from
    /// its mapping key/title.
    pub id: String,
    /// Human-readable topic title.
    pub title: String,
    /// Optional bounded description.
    pub description: Option<String>,
    /// Note path globs included by this topic.
    pub include: Vec<String>,
    /// Note path globs excluded by this topic.
    pub exclude: Vec<String>,
    /// Pinned note path globs receiving higher deterministic relevance.
    pub pinned: Vec<String>,
    /// Alternate names retained in the node projection.
    pub aliases: Vec<String>,
    /// Nested manual topics.
    pub children: Vec<TaxonomyTopic>,
}

/// Validated `_mcp-vault/index.yaml` overlay.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Taxonomy {
    /// Top-level topics in deterministic source order.
    pub topics: Vec<TaxonomyTopic>,
}

/// Result of a complete derived-index rebuild for one Vault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexRebuildReport {
    /// Active entries considered by the rebuild.
    pub indexed_entries: u64,
    /// Markdown notes successfully analyzed.
    pub indexed_notes: u64,
    /// Canonical bytes consumed by the analyzer.
    pub indexed_bytes: u64,
    /// Notes skipped because they exceeded limits, were not UTF-8, or could
    /// not be read consistently through Core.
    pub skipped_notes: u64,
    /// Extracted links, including unresolved targets.
    pub indexed_links: u64,
    /// Whether the bounded taxonomy was valid or absent.
    pub taxonomy_valid: bool,
    /// Monotonic derived projection revision.
    pub index_revision: Revision,
}

/// Analyze one Markdown payload without touching the filesystem or database.
pub fn analyze_markdown(
    file_id: FileId,
    vault_id: VaultId,
    path: VaultPath,
    revision: Revision,
    content_hash: &str,
    source: &str,
) -> Result<AnalyzedNote, IndexError> {
    if source.len() > MAX_NOTE_BYTES {
        return Err(IndexError::TooLarge);
    }
    if path.is_root() || !path.as_str().to_ascii_lowercase().ends_with(".md") {
        return Err(IndexError::InvalidInput(
            "only Markdown files can be analyzed",
        ));
    }

    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.extension.wikilinks_title_after_pipe = true;
    options.extension.wikilinks_title_before_pipe = true;
    options.parse.escaped_char_spans = true;

    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    let (frontmatter, aliases, frontmatter_tags, language) = extract_frontmatter(root);

    let mut headings = Vec::new();
    let mut heading_stack: Vec<(u8, String)> = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        let NodeValue::Heading(heading) = &data.value else {
            continue;
        };
        let title = inline_text(node);
        if title.is_empty() {
            continue;
        }
        while heading_stack
            .last()
            .is_some_and(|(level, _)| *level >= heading.level)
        {
            heading_stack.pop();
        }
        heading_stack.push((heading.level, title.clone()));
        let heading_path = heading_stack
            .iter()
            .map(|(_, title)| title.clone())
            .collect::<Vec<_>>();
        let start_byte = source_position_to_byte(
            source,
            data.sourcepos.start.line,
            data.sourcepos.start.column,
        );
        let end_byte =
            source_position_to_byte(source, data.sourcepos.end.line, data.sourcepos.end.column)
                .saturating_add(1)
                .min(source.len());
        headings.push(HeadingProjectionInput {
            id: projection_id(file_id, "heading", headings.len()),
            ordinal: u32::try_from(headings.len())
                .map_err(|_| IndexError::InvalidInput("heading count overflow"))?,
            level: heading.level,
            heading_path_json: serde_json::to_string(&heading_path)
                .map_err(|_| IndexError::InvalidInput("heading path serialization failed"))?,
            title,
            start_byte: u64::try_from(start_byte)
                .map_err(|_| IndexError::InvalidInput("heading offset overflow"))?,
            end_byte: Some(
                u64::try_from(end_byte)
                    .map_err(|_| IndexError::InvalidInput("heading offset overflow"))?,
            ),
        });
    }

    let mut tags = BTreeMap::<String, TagProjectionInput>::new();
    for tag in frontmatter_tags {
        let normalized = normalize_tag(&tag);
        if !normalized.is_empty() {
            tags.entry(format!("frontmatter:{normalized}"))
                .or_insert_with(|| TagProjectionInput {
                    tag,
                    normalized_tag: normalized,
                    source: "frontmatter".to_owned(),
                });
        }
    }

    let mut links = Vec::new();
    collect_inline_metadata(root, &mut tags, &mut links, file_id)?;

    let plain_text = plain_text(root);
    let first_paragraph = first_paragraph(root);
    let title = frontmatter
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            headings
                .iter()
                .find(|heading| heading.level == 1)
                .map(|heading| heading.title.clone())
        })
        .or_else(|| {
            path.file_name()
                .map(|name| name.strip_suffix(".md").unwrap_or(name).to_owned())
        });
    let aliases_json = serde_json::to_string(&aliases)
        .map_err(|_| IndexError::InvalidInput("aliases serialization failed"))?;
    let frontmatter_json = serde_json::to_string(&frontmatter)
        .map_err(|_| IndexError::InvalidInput("frontmatter serialization failed"))?;
    let fts_tags = tags
        .values()
        .map(|tag| tag.normalized_tag.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let fts_headings = headings
        .iter()
        .map(|heading| heading.title.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let fts_aliases = aliases.join(" ");

    Ok(AnalyzedNote {
        note: NoteProjectionInput {
            file_id,
            vault_id,
            path,
            revision,
            title,
            aliases_json,
            frontmatter_json,
            plain_text: plain_text.clone(),
            first_paragraph,
            language,
            word_count: plain_text.split_whitespace().count() as u64,
            analyzed_content_hash: content_hash.to_owned(),
            analyzer_version: ANALYZER_VERSION,
            fts_aliases,
            fts_tags,
            fts_headings,
        },
        headings,
        tags: tags.into_values().collect(),
        links,
    })
}

fn extract_frontmatter<'a>(
    root: &'a AstNode<'a>,
) -> (Map<String, Value>, Vec<String>, Vec<String>, Option<String>) {
    let Some(frontmatter) = root.descendants().find_map(|node| {
        let data = node.data.borrow();
        match &data.value {
            NodeValue::FrontMatter(raw) => Some(raw.clone()),
            _ => None,
        }
    }) else {
        return (Map::new(), Vec::new(), Vec::new(), None);
    };
    let yaml = frontmatter_body(&frontmatter);
    if yaml.len() > MAX_YAML_BYTES {
        return (Map::new(), Vec::new(), Vec::new(), None);
    }
    let Ok(documents) = YamlLoader::load_from_str(&yaml) else {
        return (Map::new(), Vec::new(), Vec::new(), None);
    };
    let Some(document) = documents.first() else {
        return (Map::new(), Vec::new(), Vec::new(), None);
    };
    let mut nodes = 0;
    let Ok(value) = yaml_to_json(document, 0, &mut nodes) else {
        return (Map::new(), Vec::new(), Vec::new(), None);
    };
    let Value::Object(map) = value else {
        return (Map::new(), Vec::new(), Vec::new(), None);
    };
    let aliases = string_list(map.get("aliases"));
    let tags = string_list(map.get("tags"));
    let language = map
        .get("language")
        .or_else(|| map.get("lang"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    (map, aliases, tags, language)
}

fn frontmatter_body(raw: &str) -> String {
    let mut lines = raw.lines();
    let _ = lines.next();
    lines
        .take_while(|line| line.trim() != "---")
        .collect::<Vec<_>>()
        .join("\n")
}

fn yaml_to_json(yaml: &Yaml, depth: usize, nodes: &mut usize) -> Result<Value, IndexError> {
    if depth > MAX_YAML_DEPTH || *nodes >= MAX_YAML_NODES {
        return Err(IndexError::Yaml);
    }
    *nodes += 1;
    match yaml {
        Yaml::Null | Yaml::BadValue => Ok(Value::Null),
        Yaml::Boolean(value) => Ok(Value::Bool(*value)),
        Yaml::Integer(value) => Ok(Value::Number((*value).into())),
        Yaml::Real(value) => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or(IndexError::Yaml),
        Yaml::String(value) => {
            if value.len() > MAX_YAML_BYTES {
                return Err(IndexError::Yaml);
            }
            Ok(Value::String(value.clone()))
        }
        Yaml::Array(values) => Ok(Value::Array(
            values
                .iter()
                .map(|value| yaml_to_json(value, depth + 1, nodes))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Yaml::Hash(values) => {
            let mut object = Map::new();
            for (key, value) in values {
                let Value::String(key) = yaml_to_json(key, depth + 1, nodes)? else {
                    continue;
                };
                object.insert(key, yaml_to_json(value, depth + 1, nodes)?);
            }
            Ok(Value::Object(object))
        }
        Yaml::Alias(_) => Err(IndexError::Yaml),
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// Parse and validate the bounded portable taxonomy format.
pub fn parse_taxonomy(source: &str) -> Result<Taxonomy, IndexError> {
    if source.len() > MAX_YAML_BYTES {
        return Err(IndexError::TooLarge);
    }
    let documents = YamlLoader::load_from_str(source).map_err(|_| IndexError::Yaml)?;
    if documents.len() > 1 {
        return Err(IndexError::InvalidInput(
            "taxonomy must contain exactly one YAML document",
        ));
    }
    let Some(document) = documents.first() else {
        return Ok(Taxonomy::default());
    };
    let mut nodes = 0;
    let value = yaml_to_json(document, 0, &mut nodes)?;
    let Value::Object(root) = value else {
        return Err(IndexError::InvalidInput("taxonomy root must be a mapping"));
    };
    let mut topics = Vec::new();
    let mut seen = BTreeMap::new();
    if let Some(value) = root.get("topics") {
        parse_topic_collection(value, None, &mut topics, &mut seen)?;
    }
    Ok(Taxonomy { topics })
}

fn parse_topic_collection(
    value: &Value,
    mapping_hint: Option<&str>,
    output: &mut Vec<TaxonomyTopic>,
    seen: &mut BTreeMap<String, ()>,
) -> Result<(), IndexError> {
    match value {
        Value::Array(values) => {
            for value in values {
                output.push(parse_topic(value, mapping_hint, seen)?);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                output.push(parse_topic(value, Some(key), seen)?);
            }
        }
        _ => {
            return Err(IndexError::InvalidInput(
                "taxonomy topics must be a list or mapping",
            ));
        }
    }
    Ok(())
}

fn parse_topic(
    value: &Value,
    mapping_hint: Option<&str>,
    seen: &mut BTreeMap<String, ()>,
) -> Result<TaxonomyTopic, IndexError> {
    let Value::Object(map) = value else {
        return Err(IndexError::InvalidInput("taxonomy topic must be a mapping"));
    };
    let explicit_id = map
        .get("id")
        .or_else(|| map.get("slug"))
        .or_else(|| map.get("title"))
        .and_then(Value::as_str);
    let id = mapping_hint
        .or(explicit_id)
        .ok_or(IndexError::InvalidInput(
            "taxonomy topic needs an id or title",
        ))?;
    let id = normalize_topic_id(id)?;
    if seen.insert(id.clone(), ()).is_some() {
        return Err(IndexError::InvalidInput(
            "taxonomy topic ids must be unique",
        ));
    }
    let title = map
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| id.clone());
    validate_taxonomy_text(&title, "taxonomy topic title")?;
    let description = map
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(description) = description.as_deref() {
        validate_taxonomy_text(description, "taxonomy topic description")?;
    }
    let include = taxonomy_patterns(map.get("include"))?;
    let exclude = taxonomy_patterns(map.get("exclude"))?;
    let pinned = taxonomy_patterns(map.get("pinned"))?;
    let aliases = taxonomy_text_list(map.get("aliases"), "taxonomy aliases")?;
    let mut children = Vec::new();
    if let Some(value) = map.get("topics").or_else(|| map.get("children")) {
        parse_topic_collection(value, None, &mut children, seen)?;
    }
    Ok(TaxonomyTopic {
        id,
        title,
        description,
        include,
        exclude,
        pinned,
        aliases,
        children,
    })
}

fn normalize_topic_id(value: &str) -> Result<String, IndexError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(IndexError::InvalidInput("taxonomy topic id is invalid"));
    }
    let normalized = value
        .chars()
        .map(|character| {
            if character.is_whitespace() {
                '-'
            } else {
                character
            }
        })
        .collect::<String>();
    if normalized == "." || normalized == ".." || normalized.contains("..") {
        return Err(IndexError::InvalidInput("taxonomy topic id is invalid"));
    }
    Ok(normalized)
}

fn taxonomy_text_list(
    value: Option<&Value>,
    field: &'static str,
) -> Result<Vec<String>, IndexError> {
    let values = match value {
        None => Vec::new(),
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or(IndexError::InvalidInput(field))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(IndexError::InvalidInput(field)),
    };
    for value in &values {
        validate_taxonomy_text(value, field)?;
    }
    Ok(values)
}

fn taxonomy_patterns(value: Option<&Value>) -> Result<Vec<String>, IndexError> {
    let values = taxonomy_text_list(value, "taxonomy glob")?;
    for value in &values {
        if value.starts_with('/')
            || value.contains('\\')
            || value.contains('\0')
            || value.split('/').any(|segment| segment == "..")
        {
            return Err(IndexError::InvalidInput("taxonomy glob is unsafe"));
        }
    }
    Ok(values)
}

fn validate_taxonomy_text(value: &str, field: &'static str) -> Result<(), IndexError> {
    if value.is_empty() || value.len() > MAX_YAML_BYTES || value.chars().any(char::is_control) {
        return Err(IndexError::InvalidInput(field));
    }
    Ok(())
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star = None;
    let mut star_value_index = 0;
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == value[value_index] || pattern[pattern_index] == b'?')
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star = Some(pattern_index);
            pattern_index += 1;
            star_value_index = value_index;
        } else if let Some(star_index) = star {
            pattern_index = star_index + 1;
            star_value_index += 1;
            value_index = star_value_index;
        } else {
            return false;
        }
    }
    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn collect_inline_metadata<'a>(
    node: &'a AstNode<'a>,
    tags: &mut BTreeMap<String, TagProjectionInput>,
    links: &mut Vec<LinkProjectionInput>,
    file_id: FileId,
) -> Result<(), IndexError> {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Code(_) | NodeValue::CodeBlock(_) | NodeValue::FrontMatter(_) => return Ok(()),
        NodeValue::Text(text) => {
            for tag in scan_inline_tags(text) {
                let normalized = normalize_tag(&tag);
                if !normalized.is_empty() {
                    tags.entry(format!("inline:{normalized}"))
                        .or_insert_with(|| TagProjectionInput {
                            tag,
                            normalized_tag: normalized,
                            source: "inline".to_owned(),
                        });
                }
            }
        }
        NodeValue::Link(link) => {
            let (target, heading) = split_link_target(&link.url);
            links.push(LinkProjectionInput {
                id: projection_id(file_id, "link", links.len()),
                target_text: target,
                target_file_id: None,
                target_heading: heading,
                link_type: "markdown".to_owned(),
                ordinal: u32::try_from(links.len())
                    .map_err(|_| IndexError::InvalidInput("link count overflow"))?,
            });
        }
        NodeValue::WikiLink(link) => {
            let (target, heading) = split_link_target(&link.url);
            links.push(LinkProjectionInput {
                id: projection_id(file_id, "link", links.len()),
                target_text: target,
                target_file_id: None,
                target_heading: heading,
                link_type: "wikilink".to_owned(),
                ordinal: u32::try_from(links.len())
                    .map_err(|_| IndexError::InvalidInput("link count overflow"))?,
            });
        }
        NodeValue::Escaped => return Ok(()),
        _ => {}
    }
    drop(data);
    for child in node.children() {
        collect_inline_metadata(child, tags, links, file_id)?;
    }
    Ok(())
}

fn scan_inline_tags(text: &str) -> Vec<String> {
    let chars = text.char_indices().collect::<Vec<_>>();
    let mut tags = Vec::new();
    for (index, (offset, character)) in chars.iter().enumerate() {
        if *character != '#' {
            continue;
        }
        let boundary = chars
            .get(index.wrapping_sub(1))
            .map(|(_, previous)| !previous.is_alphanumeric() && *previous != '_')
            .unwrap_or(true);
        if !boundary {
            continue;
        }
        let end = chars
            .iter()
            .skip(index + 1)
            .take_while(|(_, value)| value.is_alphanumeric() || matches!(value, '_' | '-' | '/'))
            .map(|(position, _)| *position)
            .last()
            .map_or(*offset + 1, |position| {
                position + text[position..].chars().next().map_or(0, char::len_utf8)
            });
        if end > *offset + 1 {
            tags.push(text[*offset + 1..end].to_owned());
        }
    }
    tags
}

fn normalize_tag(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('#')
        .chars()
        .flat_map(char::to_lowercase)
        .collect()
}

fn split_link_target(value: &str) -> (String, Option<String>) {
    let value = value.trim().trim_start_matches('!');
    let (target, heading) = value
        .split_once('#')
        .map_or((value, None), |(target, heading)| {
            (target, Some(heading.trim().to_owned()))
        });
    (target.trim().to_owned(), heading)
}

fn inline_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut output = String::new();
    for child in node.children() {
        append_inline_text(child, &mut output);
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_inline_text<'a>(node: &'a AstNode<'a>, output: &mut String) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::Text(text) => output.push_str(text),
        NodeValue::Code(code) => output.push_str(&code.literal),
        NodeValue::WikiLink(link) => output.push_str(&link.url),
        NodeValue::SoftBreak | NodeValue::LineBreak => output.push(' '),
        NodeValue::CodeBlock(_) | NodeValue::FrontMatter(_) => {}
        _ => {
            drop(data);
            for child in node.children() {
                append_inline_text(child, output);
            }
        }
    }
}

fn plain_text<'a>(root: &'a AstNode<'a>) -> String {
    let mut output = String::new();
    for child in root.children() {
        append_plain_text(child, &mut output);
    }
    output.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn append_plain_text<'a>(node: &'a AstNode<'a>, output: &mut String) {
    let data = node.data.borrow();
    match &data.value {
        NodeValue::FrontMatter(_) => {}
        NodeValue::Text(text) => output.push_str(text),
        NodeValue::Code(code) => output.push_str(&code.literal),
        NodeValue::CodeBlock(code) => output.push_str(&code.literal),
        NodeValue::WikiLink(link) => output.push_str(&link.url),
        NodeValue::SoftBreak | NodeValue::LineBreak => output.push(' '),
        NodeValue::HtmlBlock(_) | NodeValue::HtmlInline(_) => {}
        _ => {
            drop(data);
            for child in node.children() {
                append_plain_text(child, output);
            }
        }
    }
    let is_block = matches!(
        node.data.borrow().value,
        NodeValue::Paragraph | NodeValue::Heading(_)
    );
    if is_block {
        output.push(' ');
    }
}

fn first_paragraph<'a>(root: &'a AstNode<'a>) -> Option<String> {
    root.descendants()
        .find(|node| matches!(node.data.borrow().value, NodeValue::Paragraph))
        .map(inline_text)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(1000).collect())
}

fn source_position_to_byte(source: &str, line: usize, column: usize) -> usize {
    let mut current_line = 1;
    let mut line_start = 0;
    for (offset, character) in source.char_indices() {
        if current_line >= line {
            line_start = line_start.max(offset);
            break;
        }
        if character == '\n' {
            current_line += 1;
            line_start = offset + 1;
        }
    }
    line_start
        .saturating_add(column.saturating_sub(1))
        .min(source.len())
}

fn projection_id(file_id: FileId, kind: &str, ordinal: usize) -> String {
    format!("{file_id}:{kind}:{ordinal}")
}

/// Convert a parsed source into a stable SHA-256 content address.
pub fn content_hash(source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.as_bytes());
    format!("sha256:{:x}", hasher.finalize())
}

/// Quote user words for a safe FTS5 AND query.
pub fn quote_fts_query(query: &str) -> Result<String, IndexError> {
    let terms = query
        .split_whitespace()
        .map(|term| term.trim_matches(|character: char| character.is_ascii_punctuation()))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() || terms.len() > 64 {
        return Err(IndexError::InvalidInput(
            "search query is empty or too broad",
        ));
    }
    Ok(terms
        .into_iter()
        .map(|term| format!("\"{}\"", term.replace('\"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}

/// Deterministic lexical/index query service.
#[derive(Clone)]
pub struct IndexService {
    state: mcp_vault_state::StateStore,
}

impl IndexService {
    /// Bind the service to operational state.
    pub fn new(state: mcp_vault_state::StateStore) -> Self {
        Self { state }
    }

    /// Return the underlying Vault-scoped projection repository.
    pub fn repository(&self) -> IndexRepository {
        self.state.index()
    }

    /// Search indexed notes with a safe FTS query and Vault scope.
    #[allow(clippy::too_many_arguments)]
    pub async fn search_notes(
        &self,
        context: &VaultContext,
        query: &str,
        path_prefix: Option<&str>,
        tag: Option<&str>,
        modified_after: Option<i64>,
        modified_before: Option<i64>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, IndexError> {
        let tags = tag.map(str::to_owned).into_iter().collect::<Vec<_>>();
        self.search_notes_scoped(
            context,
            query,
            path_prefix,
            &tags,
            &[],
            modified_after,
            modified_before,
            limit,
            offset,
        )
        .await
    }

    /// Search indexed notes with optional deterministic taxonomy constraints.
    #[allow(clippy::too_many_arguments)]
    pub async fn search_notes_scoped(
        &self,
        context: &VaultContext,
        query: &str,
        path_prefix: Option<&str>,
        tags: &[String],
        topic_keys: &[String],
        modified_after: Option<i64>,
        modified_before: Option<i64>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, IndexError> {
        if topic_keys.len() > 20 {
            return Err(IndexError::InvalidInput("too many topic filters"));
        }
        let tags = tags
            .iter()
            .map(|tag| normalize_tag(tag))
            .filter(|tag| !tag.is_empty())
            .collect::<Vec<_>>();
        let fts_query = quote_fts_query(query)?;
        self.repository()
            .search_notes(
                context,
                &fts_query,
                path_prefix,
                &tags,
                topic_keys,
                modified_after,
                modified_before,
                limit,
                offset,
            )
            .await
            .map_err(IndexError::State)
    }

    /// Rebuild all derived Markdown/index projections for one Vault.
    pub async fn rebuild_vault(
        &self,
        core: &VaultCore,
        context: &VaultContext,
    ) -> Result<IndexRebuildReport, IndexError> {
        let repository = self.repository();
        let previous_status = repository.status(context).await?;
        let index_revision = previous_status
            .as_ref()
            .map(|status| status.index_revision.next())
            .transpose()
            .map_err(|_| IndexError::InvalidInput("index revision overflow"))?
            .unwrap_or_else(|| Revision::new(1));
        let entries = self
            .state
            .files()
            .list_active_entries(context)
            .await?
            .into_iter()
            .filter(|entry| !core.is_managed_path(&entry.path))
            .collect::<Vec<_>>();
        let taxonomy = match load_taxonomy(core, context).await {
            Ok(taxonomy) => (taxonomy, true),
            Err(IndexError::Core(VaultError::NotFound)) => (Taxonomy::default(), true),
            Err(IndexError::Yaml | IndexError::TooLarge | IndexError::InvalidInput(_)) => {
                (Taxonomy::default(), false)
            }
            Err(error) => return Err(error),
        };

        repository.clear_vault(context).await?;

        let path_index = entries
            .iter()
            .filter(|entry| entry.entry_type == EntryType::File)
            .map(|entry| (path_key(&entry.path), entry.id))
            .collect::<BTreeMap<_, _>>();
        let mut indexed_notes = Vec::new();
        let mut skipped_notes = 0_u64;
        let mut indexed_bytes = 0_u64;
        let mut indexed_links = 0_u64;
        for entry in &entries {
            if entry.entry_type != EntryType::File
                || !entry.path.as_str().to_ascii_lowercase().ends_with(".md")
            {
                continue;
            }
            let analyzed = match analyze_file(core, context, entry).await {
                Ok(analyzed) => analyzed,
                Err(error) if is_skippable_note_error(&error) => {
                    skipped_notes = skipped_notes.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let mut analyzed = analyzed;
            resolve_links(&mut analyzed, &entry.path, entry.id, &path_index);
            repository
                .replace_note(
                    context,
                    &analyzed.note,
                    &analyzed.headings,
                    &analyzed.tags,
                    &analyzed.links,
                )
                .await?;
            indexed_bytes = indexed_bytes.saturating_add(entry.size);
            indexed_links = indexed_links.saturating_add(analyzed.links.len() as u64);
            indexed_notes.push(IndexedNote {
                file: entry.clone(),
                analyzed,
            });
        }

        let nodes_and_memberships =
            build_knowledge_map(context, &indexed_notes, &taxonomy.0, current_millis());
        let complete = skipped_notes == 0 && taxonomy.1;
        let status = IndexStatusRecord {
            vault_id: context.id(),
            index_revision,
            indexed_entries: entries.len() as u64,
            indexed_notes: indexed_notes.len() as u64,
            indexed_bytes,
            analyzer_version: ANALYZER_VERSION,
            coverage: serde_json::json!({
                "complete": complete,
                "taxonomy": if taxonomy.1 { "valid_or_absent" } else { "invalid" },
                "skipped_notes": skipped_notes,
                "indexed_links": indexed_links,
                "analyzer_version": ANALYZER_VERSION,
            }),
            last_rebuilt_at: complete.then_some(current_millis()),
            last_error: if skipped_notes != 0 {
                Some("note_projection_skipped".to_owned())
            } else if !taxonomy.1 {
                Some("taxonomy_invalid".to_owned())
            } else {
                None
            },
        };
        repository
            .replace_knowledge_map(
                context,
                &nodes_and_memberships.0,
                &nodes_and_memberships.1,
                &status,
            )
            .await?;

        Ok(IndexRebuildReport {
            indexed_entries: entries.len() as u64,
            indexed_notes: indexed_notes.len() as u64,
            indexed_bytes,
            skipped_notes,
            indexed_links,
            taxonomy_valid: taxonomy.1,
            index_revision,
        })
    }

    /// Rebuild the derived view after one external or canonical note change.
    ///
    /// A full rebuild keeps folder/tag/topic memberships and coverage status
    /// coherent. The operation is still derived-only and can be retried after
    /// a process restart.
    pub async fn index_file(
        &self,
        core: &VaultCore,
        context: &VaultContext,
        _path: &VaultPath,
    ) -> Result<IndexRebuildReport, IndexError> {
        self.rebuild_vault(core, context).await
    }

    /// Remove one note projection without touching canonical content.
    pub async fn remove_file(
        &self,
        context: &VaultContext,
        file_id: FileId,
    ) -> Result<(), IndexError> {
        self.repository()
            .remove_note(context, file_id)
            .await
            .map_err(IndexError::State)
    }

    /// Return deterministic map nodes under a stable key.
    pub async fn list_nodes(
        &self,
        context: &VaultContext,
        parent_key: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<IndexNodeRecord>, IndexError> {
        self.repository()
            .list_nodes(context, parent_key, limit, offset)
            .await
            .map_err(IndexError::State)
    }

    /// Return one node's indexed note members.
    pub async fn list_node_notes(
        &self,
        context: &VaultContext,
        node_key: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, IndexError> {
        self.repository()
            .list_node_notes(context, node_key, limit, offset)
            .await
            .map_err(IndexError::State)
    }

    /// Return deterministic related notes using shared tags and direct links.
    pub async fn related_notes(
        &self,
        context: &VaultContext,
        file_id: FileId,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<NoteSearchRecord>, IndexError> {
        self.repository()
            .related_notes(context, file_id, limit, offset)
            .await
            .map_err(IndexError::State)
    }

    /// Return current coverage/status without scanning canonical files.
    pub async fn status(
        &self,
        context: &VaultContext,
    ) -> Result<Option<IndexStatusRecord>, IndexError> {
        self.repository()
            .status(context)
            .await
            .map_err(IndexError::State)
    }
}

#[derive(Clone, Debug)]
struct IndexedNote {
    file: FileRecord,
    analyzed: AnalyzedNote,
}

async fn load_taxonomy(core: &VaultCore, context: &VaultContext) -> Result<Taxonomy, IndexError> {
    let path = VaultPath::parse("_mcp-vault/index.yaml")
        .map_err(|_| IndexError::InvalidInput("managed taxonomy path is invalid"))?;
    let managed = core.read_managed(context, &path).await?;
    let mut reader = managed.reader;
    let mut bytes = Vec::new();
    let mut limited = (&mut reader).take((MAX_YAML_BYTES + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| IndexError::InvalidInput("taxonomy could not be read"))?;
    if bytes.len() > MAX_YAML_BYTES {
        return Err(IndexError::TooLarge);
    }
    let source =
        String::from_utf8(bytes).map_err(|_| IndexError::InvalidInput("taxonomy must be UTF-8"))?;
    parse_taxonomy(&source)
}

async fn analyze_file(
    core: &VaultCore,
    context: &VaultContext,
    file: &FileRecord,
) -> Result<AnalyzedNote, IndexError> {
    let read = core.read(context, &file.path).await?;
    let mut reader = read.reader;
    let mut bytes = Vec::new();
    let mut limited = (&mut reader).take((MAX_NOTE_BYTES + 1) as u64);
    limited
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| IndexError::InvalidInput("note could not be read"))?;
    if bytes.len() > MAX_NOTE_BYTES {
        return Err(IndexError::TooLarge);
    }
    let source = String::from_utf8(bytes)
        .map_err(|_| IndexError::InvalidInput("Markdown note must be UTF-8"))?;
    let hash = file
        .content_hash
        .clone()
        .unwrap_or_else(|| content_hash(&source));
    analyze_markdown(
        file.id,
        context.id(),
        file.path.clone(),
        file.current_revision,
        &hash,
        &source,
    )
}

fn is_skippable_note_error(error: &IndexError) -> bool {
    matches!(
        error,
        IndexError::Core(VaultError::NotFound)
            | IndexError::Core(VaultError::ExternalMismatch)
            | IndexError::Core(VaultError::BinaryTextOperation)
            | IndexError::TooLarge
            | IndexError::InvalidInput(_)
    )
}

fn resolve_links(
    note: &mut AnalyzedNote,
    source_path: &VaultPath,
    source_file_id: FileId,
    path_index: &BTreeMap<String, FileId>,
) {
    for link in &mut note.links {
        let target = link.target_text.trim();
        if target.is_empty() {
            link.target_file_id = Some(source_file_id);
            continue;
        }
        if target.contains("://") || target.starts_with("mailto:") {
            continue;
        }
        let target = target.trim_start_matches("./");
        let mut candidates = Vec::new();
        if let Ok(target_path) = VaultPath::parse(target) {
            if let Some(parent) = source_path.parent()
                && let Ok(relative) = parent.join(&target_path)
            {
                candidates.push(relative);
            }
            candidates.push(target_path);
        }
        if !target.to_ascii_lowercase().ends_with(".md") {
            let with_extension = format!("{target}.md");
            if let Ok(target_path) = VaultPath::parse(&with_extension) {
                if let Some(parent) = source_path.parent()
                    && let Ok(relative) = parent.join(&target_path)
                {
                    candidates.push(relative);
                }
                candidates.push(target_path);
            }
        }
        link.target_file_id = candidates
            .iter()
            .find_map(|candidate| path_index.get(&path_key(candidate)).copied());
    }
}

fn path_key(path: &VaultPath) -> String {
    path.as_str().chars().flat_map(char::to_lowercase).collect()
}

fn build_knowledge_map(
    context: &VaultContext,
    notes: &[IndexedNote],
    taxonomy: &Taxonomy,
    now: i64,
) -> (
    Vec<IndexNodeProjectionInput>,
    Vec<IndexMembershipProjectionInput>,
) {
    let vault_key = context.id().to_string();
    let root_id = format!("{vault_key}:index:root");
    let mut nodes = vec![IndexNodeProjectionInput {
        id: root_id.clone(),
        parent_id: None,
        node_type: "root".to_owned(),
        stable_key: "root".to_owned(),
        title: "Vault".to_owned(),
        summary: None,
        source_type: "derived".to_owned(),
        source_ref: None,
        confidence: Some(1.0),
        sort_key: String::new(),
        content_version: ANALYZER_VERSION.to_string(),
        created_at: now,
        updated_at: now,
    }];
    let mut memberships = Vec::new();

    let mut folder_paths = BTreeMap::<String, VaultPath>::new();
    for note in notes {
        let mut parent = note.file.path.parent();
        while let Some(path) = parent {
            if path.is_root() {
                break;
            }
            folder_paths.insert(path.as_str().to_owned(), path.clone());
            parent = path.parent();
        }
    }
    let mut folder_ids = BTreeMap::<String, String>::new();
    let mut sorted_folders = folder_paths.into_values().collect::<Vec<_>>();
    sorted_folders.sort_by_key(|path| (path.depth(), path.as_str().to_owned()));
    for path in sorted_folders {
        let id = format!("{vault_key}:index:folder:{}", path.as_str());
        let parent_id = path
            .parent()
            .filter(|parent| !parent.is_root())
            .and_then(|parent| folder_ids.get(parent.as_str()).cloned())
            .or_else(|| Some(root_id.clone()));
        nodes.push(IndexNodeProjectionInput {
            id: id.clone(),
            parent_id,
            node_type: "folder".to_owned(),
            stable_key: format!("folder:{}", path.as_str()),
            title: path.file_name().unwrap_or("Folder").to_owned(),
            summary: None,
            source_type: "derived".to_owned(),
            source_ref: Some(path.as_str().to_owned()),
            confidence: Some(1.0),
            sort_key: format!("folder:{}", path.as_str()),
            content_version: ANALYZER_VERSION.to_string(),
            created_at: now,
            updated_at: now,
        });
        folder_ids.insert(path.as_str().to_owned(), id);
    }

    let mut tag_display = BTreeMap::<String, String>::new();
    for note in notes {
        for tag in &note.analyzed.tags {
            tag_display
                .entry(tag.normalized_tag.clone())
                .or_insert_with(|| tag.tag.clone());
        }
    }
    let mut tag_ids = BTreeMap::new();
    for (normalized, display) in tag_display {
        let id = format!("{vault_key}:index:tag:{normalized}");
        nodes.push(IndexNodeProjectionInput {
            id: id.clone(),
            parent_id: Some(root_id.clone()),
            node_type: "tag".to_owned(),
            stable_key: format!("tag:{normalized}"),
            title: display,
            summary: None,
            source_type: "derived".to_owned(),
            source_ref: Some(normalized.clone()),
            confidence: Some(1.0),
            sort_key: format!("tag:{normalized}"),
            content_version: ANALYZER_VERSION.to_string(),
            created_at: now,
            updated_at: now,
        });
        tag_ids.insert(normalized, id);
    }

    for note in notes {
        if let Some(parent) = note.file.path.parent().filter(|parent| !parent.is_root())
            && let Some(node_id) = folder_ids.get(parent.as_str())
        {
            memberships.push(IndexMembershipProjectionInput {
                node_id: node_id.clone(),
                file_id: note.file.id,
                relevance: 1.0,
                source_type: "folder".to_owned(),
            });
        }
        for tag in &note.analyzed.tags {
            if let Some(node_id) = tag_ids.get(&tag.normalized_tag) {
                memberships.push(IndexMembershipProjectionInput {
                    node_id: node_id.clone(),
                    file_id: note.file.id,
                    relevance: 1.0,
                    source_type: "tag".to_owned(),
                });
            }
        }
    }

    for topic in &taxonomy.topics {
        add_taxonomy_topic(
            topic,
            &root_id,
            None,
            &vault_key,
            notes,
            &mut nodes,
            &mut memberships,
            now,
        );
    }
    (nodes, memberships)
}

#[allow(clippy::too_many_arguments)]
fn add_taxonomy_topic(
    topic: &TaxonomyTopic,
    parent_id: &str,
    parent_key: Option<&str>,
    vault_key: &str,
    notes: &[IndexedNote],
    nodes: &mut Vec<IndexNodeProjectionInput>,
    memberships: &mut Vec<IndexMembershipProjectionInput>,
    now: i64,
) {
    let qualified = parent_key
        .map(|parent| format!("{parent}/{}", topic.id))
        .unwrap_or_else(|| topic.id.clone());
    let stable_key = format!("topic:{qualified}");
    let id = format!("{vault_key}:index:{stable_key}");
    nodes.push(IndexNodeProjectionInput {
        id: id.clone(),
        parent_id: Some(parent_id.to_owned()),
        node_type: "manual_topic".to_owned(),
        stable_key,
        title: topic.title.clone(),
        summary: topic.description.clone(),
        source_type: "taxonomy".to_owned(),
        source_ref: Some(format!("_mcp-vault/index.yaml:{qualified}")),
        confidence: Some(1.0),
        sort_key: format!("topic:{qualified}"),
        content_version: ANALYZER_VERSION.to_string(),
        created_at: now,
        updated_at: now,
    });
    for note in notes {
        let included = topic.include.is_empty()
            || topic
                .include
                .iter()
                .any(|pattern| glob_matches(pattern, note.file.path.as_str()));
        let excluded = topic
            .exclude
            .iter()
            .any(|pattern| glob_matches(pattern, note.file.path.as_str()));
        if included && !excluded {
            let pinned = topic
                .pinned
                .iter()
                .any(|pattern| glob_matches(pattern, note.file.path.as_str()));
            memberships.push(IndexMembershipProjectionInput {
                node_id: id.clone(),
                file_id: note.file.id,
                relevance: if pinned { 2.0 } else { 1.0 },
                source_type: "taxonomy".to_owned(),
            });
        }
    }
    for child in &topic.children {
        add_taxonomy_topic(
            child,
            &id,
            Some(&qualified),
            vault_key,
            notes,
            nodes,
            memberships,
            now,
        );
    }
}

fn current_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        IndexService, Taxonomy, analyze_markdown, content_hash, parse_taxonomy, quote_fts_query,
    };
    use mcp_vault_core::VaultCore;
    use mcp_vault_domain::{
        Actor, Revision, SourcePlane, VaultContext, VaultId, VaultPath, VaultPathPolicy, VaultSlug,
    };
    use mcp_vault_state::{StateStore, VaultStatus};
    use mcp_vault_storage_fs::StorageOptions;
    use tempfile::tempdir;

    fn fixture() -> (mcp_vault_domain::FileId, VaultId, VaultPath) {
        (
            mcp_vault_domain::FileId::new(),
            VaultId::new(),
            VaultPath::parse("知识/设计.md").unwrap(),
        )
    }

    #[test]
    fn analyzes_multilingual_markdown_without_code_false_positives() {
        let (file_id, vault_id, path) = fixture();
        let note = analyze_markdown(
            file_id,
            vault_id,
            path,
            Revision::new(3),
            &content_hash("# fallback"),
            "---\ntitle: 架构\ntags: [Rust, 中文]\naliases: [Design]\n---\n# 设计\n\n正文 #实际 [[另一个.md#部分]]。\n\n~~~text\n#假标签 [[假链接]]\n~~~\n",
        )
        .unwrap();
        assert_eq!(note.note.title.as_deref(), Some("架构"));
        assert!(note.note.plain_text.contains("正文"));
        assert!(note.tags.iter().any(|tag| tag.normalized_tag == "实际"));
        assert!(note.tags.iter().any(|tag| tag.normalized_tag == "rust"));
        assert!(!note.tags.iter().any(|tag| tag.normalized_tag == "假标签"));
        assert_eq!(note.links.len(), 1);
        assert_eq!(note.links[0].target_text, "另一个.md");
        assert_eq!(note.links[0].target_heading.as_deref(), Some("部分"));
        assert_eq!(note.headings[0].title, "设计");
    }

    #[test]
    fn malformed_frontmatter_does_not_make_plain_markdown_unsafe() {
        let (file_id, vault_id, path) = fixture();
        let note = analyze_markdown(
            file_id,
            vault_id,
            path,
            Revision::new(1),
            "sha256:test",
            "---\ntitle: [broken\n---\n# 标题\n\n内容",
        )
        .unwrap();
        assert_eq!(note.note.title.as_deref(), Some("标题"));
        assert!(note.note.plain_text.contains("内容"));
    }

    #[test]
    fn fts_query_quotes_operators_and_rejects_empty_input() {
        assert_eq!(
            quote_fts_query("WebDAV conflict").unwrap(),
            "\"WebDAV\" AND \"conflict\""
        );
        assert!(quote_fts_query("   !!! ").is_err());
    }

    #[test]
    fn taxonomy_parser_validates_and_preserves_manual_overlay() {
        let taxonomy = parse_taxonomy(
            "topics:\n  - id: architecture\n    title: Architecture\n    include: [docs/**]\n    exclude: [docs/private/**]\n    pinned: [docs/architecture.md]\n    aliases: [design]\n    description: Stable design notes\n    children:\n      - id: decisions\n        title: Decisions\n",
        )
        .unwrap();
        assert_eq!(
            taxonomy,
            Taxonomy {
                topics: vec![super::TaxonomyTopic {
                    id: "architecture".to_owned(),
                    title: "Architecture".to_owned(),
                    description: Some("Stable design notes".to_owned()),
                    include: vec!["docs/**".to_owned()],
                    exclude: vec!["docs/private/**".to_owned()],
                    pinned: vec!["docs/architecture.md".to_owned()],
                    aliases: vec!["design".to_owned()],
                    children: vec![super::TaxonomyTopic {
                        id: "decisions".to_owned(),
                        title: "Decisions".to_owned(),
                        description: None,
                        include: Vec::new(),
                        exclude: Vec::new(),
                        pinned: Vec::new(),
                        aliases: Vec::new(),
                        children: Vec::new(),
                    }],
                }],
            }
        );
        assert!(parse_taxonomy("topics:\n  - id: bad\n    include: [../secret]\n").is_err());
    }

    #[test]
    fn context_types_remain_available_to_index_services() {
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("test").unwrap(),
            "/tmp/test-indexer".into(),
            Revision::new(1),
        )
        .unwrap();
        assert_eq!(context.slug().as_str(), "test");
    }

    #[tokio::test]
    async fn rebuilds_projection_after_deletion_and_keeps_vaults_isolated() {
        let root = tempdir().unwrap();
        let content_root = root.path().join("vault");
        let context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("work").unwrap(),
            PathBuf::from(&content_root),
            Revision::new(1),
        )
        .unwrap();
        let other_root = root.path().join("other");
        let other_context = VaultContext::new(
            VaultId::new(),
            VaultSlug::new("other").unwrap(),
            PathBuf::from(&other_root),
            Revision::new(1),
        )
        .unwrap();
        let state = StateStore::connect_and_migrate("sqlite::memory:")
            .await
            .unwrap();
        state
            .vaults()
            .insert(&context, "Work", VaultStatus::Active)
            .await
            .unwrap();
        state
            .vaults()
            .insert(&other_context, "Other", VaultStatus::Active)
            .await
            .unwrap();
        let core = VaultCore::new(
            state.clone(),
            root.path().join("history"),
            VaultPathPolicy::default(),
            StorageOptions::default(),
            Default::default(),
        );
        let path = VaultPath::parse("docs/architecture.md").unwrap();
        let mut staged = core
            .begin_put(
                &context,
                &path,
                true,
                true,
                Actor::system(),
                SourcePlane::System,
            )
            .await
            .unwrap();
        staged
            .write_chunk(
                b"---\ntags: [Rust]\n---\n# Architecture\n\nConflict handling and [[other.md]].\n",
            )
            .await
            .unwrap();
        let created = staged.commit().await.unwrap();
        let other_path = VaultPath::parse("other.md").unwrap();
        let mut other_staged = core
            .begin_put(
                &context,
                &other_path,
                true,
                true,
                Actor::system(),
                SourcePlane::System,
            )
            .await
            .unwrap();
        other_staged
            .write_chunk(b"# Other\n\nRelated note.")
            .await
            .unwrap();
        let other = other_staged.commit().await.unwrap();

        std::fs::create_dir_all(content_root.join("_mcp-vault")).unwrap();
        std::fs::write(
            content_root.join("_mcp-vault/index.yaml"),
            "topics:\n  architecture:\n    title: Architecture\n    include: [docs/**]\n    pinned: [docs/architecture.md]\n",
        )
        .unwrap();

        let service = IndexService::new(state.clone());
        let report = service.rebuild_vault(&core, &context).await.unwrap();
        assert_eq!(report.indexed_entries, 2);
        assert_eq!(report.indexed_notes, 2);
        assert_eq!(report.indexed_links, 1);
        assert!(report.taxonomy_valid);
        let search = service
            .search_notes(&context, "conflict", None, None, None, None, 10, 0)
            .await
            .unwrap();
        assert_eq!(search.len(), 1);
        assert_eq!(
            search[0].outgoing_links[0].target_file_id,
            Some(other.file.id)
        );
        assert_eq!(search[0].backlink_count, 0);
        assert_eq!(
            service
                .related_notes(&context, created.file.id, 10, 0)
                .await
                .unwrap()
                .first()
                .map(|note| note.file_id),
            Some(other.file.id)
        );
        let topics = service
            .list_nodes(&context, Some("root"), 20, 0)
            .await
            .unwrap();
        assert!(
            topics
                .iter()
                .any(|node| node.stable_key == "topic:architecture")
        );
        assert!(
            service
                .search_notes(&other_context, "conflict", None, None, None, None, 10, 0)
                .await
                .unwrap()
                .is_empty()
        );

        service.repository().clear_vault(&context).await.unwrap();
        assert!(service.status(&context).await.unwrap().is_none());
        service.rebuild_vault(&core, &context).await.unwrap();
        assert_eq!(
            service
                .search_notes(&context, "architecture", None, None, None, None, 10, 0)
                .await
                .unwrap()
                .first()
                .map(|note| note.file_id),
            Some(created.file.id)
        );
    }
}
