//! Source-preserving, deterministic candidates for memory selection.
//!
//! A section is a complete subtree. Selecting a child also carries the exact
//! introductory text of its ancestors: a project qualifier above a heading is
//! part of its meaning. Lists, code, tables and paragraphs are never rewritten.

use std::collections::{HashMap, HashSet};

use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use serde::{Deserialize, Serialize};

use crate::{MemoryError, markdown::hash_content};

/// Exact source coordinates, interpreted only against the bound source hash.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

/// A model can select this identity but cannot supply its body or provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceUnit {
    pub id: String,
    pub headings: Vec<String>,
    pub body: SourceSpan,
    pub context: Vec<SourceSpan>,
}

impl SourceUnit {
    /// Complete original input that must travel with this unit when recalled.
    pub fn complete_text(&self) -> String {
        let mut text = String::new();
        for span in &self.context {
            text.push_str(&span.text);
            if !text.ends_with('\n') && !text.ends_with('\r') {
                text.push('\n');
            }
        }
        text.push_str(&self.body.text);
        text
    }

    /// Hash exact text and context; normalization is only a retrieval concern.
    pub fn content_hash(&self) -> String {
        hash_content(&self.complete_text())
    }
}

#[derive(Clone)]
struct Heading {
    level: u8,
    start: usize,
    title: String,
}

/// Build section candidates without I/O, model input or arbitrary text slicing.
pub fn source_units(source: &str) -> Vec<SourceUnit> {
    let lines = line_starts(source.as_bytes());
    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.extension.table = true;
    options.extension.tasklist = true;
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    let mut headings = Vec::new();
    let mut content_start = 0;
    for node in root.children() {
        let data = node.data.borrow();
        match &data.value {
            NodeValue::FrontMatter(_) => {
                content_start = line_offset(&lines, source.len(), data.sourcepos.end.line + 1);
            }
            NodeValue::Heading(heading) => {
                let title = node
                    .descendants()
                    .filter_map(|child| match &child.data.borrow().value {
                        NodeValue::Text(value) => Some(value.to_string()),
                        NodeValue::Code(value) => Some(value.literal.clone()),
                        _ => None,
                    })
                    .collect::<String>();
                headings.push(Heading {
                    level: heading.level,
                    start: line_offset(&lines, source.len(), data.sourcepos.start.line),
                    title,
                });
            }
            _ => {}
        }
    }
    let mut candidates = Vec::new();
    let intro_end = headings.first().map_or(source.len(), |h| h.start);
    if !source[content_start..intro_end].trim().is_empty() {
        candidates.push(make_unit(
            source,
            content_start,
            intro_end,
            &[],
            &[],
            &lines,
        ));
    }
    let mut parents: Vec<usize> = Vec::new();
    for (index, heading) in headings.iter().enumerate() {
        while parents
            .last()
            .is_some_and(|parent| headings[*parent].level >= heading.level)
        {
            parents.pop();
        }
        let end = headings[index + 1..]
            .iter()
            .find(|next| next.level <= heading.level)
            .map_or(source.len(), |next| next.start);
        let mut scope = parents
            .iter()
            .map(|parent| headings[*parent].title.clone())
            .collect::<Vec<_>>();
        scope.push(heading.title.clone());
        let mut context = Vec::new();
        if !source[content_start..intro_end].trim().is_empty() {
            context.push((content_start, intro_end));
        }
        for parent in &parents {
            // Every ancestor contributes its whole introduction, including its
            // heading. Nested section contents are already in the selected body.
            context.push((headings[*parent].start, headings[*parent + 1].start));
        }
        candidates.push(make_unit(
            source,
            heading.start,
            end,
            &scope,
            &context,
            &lines,
        ));
        parents.push(index);
    }
    candidates
}

/// Build the smallest first-round selection candidates from a Markdown source.
///
/// Full section subtrees remain available from [`source_units`] for bounded
/// completeness review and source reconstruction. The first selection pass
/// receives leaf subtrees and non-empty parent introductions only, so a
/// parent subtree cannot consume a batch and hide its children.
pub fn minimal_selection_units(source: &str) -> Vec<SourceUnit> {
    let full = source_units(source);
    let lines = line_starts(source.as_bytes());
    let heading_ends = heading_end_offsets(source, &lines);
    let mut result = Vec::new();
    for unit in &full {
        if unit.headings.is_empty() {
            result.push(unit.clone());
            continue;
        }
        let contains = |outer: &SourceUnit, inner: &SourceUnit| {
            outer.body.start_byte <= inner.body.start_byte
                && outer.body.end_byte >= inner.body.end_byte
                && outer.body.start_byte < inner.body.start_byte
        };
        let Some(first_child_start) = full
            .iter()
            .filter(|candidate| contains(unit, candidate))
            .map(|candidate| candidate.body.start_byte)
            .min()
        else {
            result.push(unit.clone());
            continue;
        };
        if first_child_start <= unit.body.start_byte {
            continue;
        }
        let heading_end = heading_ends
            .get(&unit.body.start_byte)
            .copied()
            .unwrap_or(unit.body.start_byte);
        let content_after_heading = &source[heading_end..first_child_start];
        if content_after_heading.trim().is_empty() {
            continue;
        }
        let mut intro_unit = unit.clone();
        intro_unit.id = format!("u:{}:{first_child_start}", unit.body.start_byte);
        intro_unit.body = source_span(source, unit.body.start_byte, first_child_start, &lines);
        result.push(intro_unit);
    }
    result
}

fn heading_end_offsets(source: &str, lines: &[usize]) -> HashMap<usize, usize> {
    let mut options = Options::default();
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.extension.table = true;
    options.extension.tasklist = true;
    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            if !matches!(data.value, NodeValue::Heading(_)) {
                return None;
            }
            let start = line_offset(lines, source.len(), data.sourcepos.start.line);
            let end = line_offset(lines, source.len(), data.sourcepos.end.line + 1);
            Some((start, end))
        })
        .collect()
}

fn line_starts(source: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    let mut offset = 0;
    while offset < source.len() {
        match source[offset] {
            b'\r' => {
                offset += 1;
                if source.get(offset) == Some(&b'\n') {
                    offset += 1;
                }
                starts.push(offset);
            }
            b'\n' => {
                offset += 1;
                starts.push(offset);
            }
            _ => offset += 1,
        }
    }
    starts
}

fn line_offset(lines: &[usize], source_len: usize, line: usize) -> usize {
    lines
        .get(line.saturating_sub(1))
        .copied()
        .unwrap_or(source_len)
}

fn source_span(source: &str, start: usize, end: usize, lines: &[usize]) -> SourceSpan {
    let start_line = lines.partition_point(|offset| *offset <= start);
    let last = end.saturating_sub(1).max(start);
    // `last` may point inside a multibyte final character. Line lookup uses
    // byte offsets without ever slicing a UTF-8 string at that position.
    let end_line = lines.partition_point(|offset| *offset <= last);
    SourceSpan {
        start_byte: start,
        end_byte: end,
        start_line: start_line as u32,
        end_line: end_line as u32,
        text: source[start..end].to_owned(),
    }
}

fn make_unit(
    source: &str,
    start: usize,
    end: usize,
    headings: &[String],
    context: &[(usize, usize)],
    lines: &[usize],
) -> SourceUnit {
    SourceUnit {
        id: format!("u:{start}:{end}"),
        headings: headings.to_vec(),
        body: source_span(source, start, end, lines),
        context: context
            .iter()
            .filter(|(a, b)| a < b)
            .map(|(a, b)| source_span(source, *a, *b, lines))
            .collect(),
    }
}

/// Strict untrusted selection. Descriptive fields are retrieval annotations.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnitSelection {
    pub unit_id: String,
    pub kind: Option<String>,
    pub retrieval_hint: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionOutput {
    pub selections: Vec<UnitSelection>,
}

/// Validate model-supplied descriptive metadata without accepting any model
/// supplied source identity or body.
pub fn validate_unit_selection_metadata(item: &UnitSelection) -> Result<(), MemoryError> {
    if item.retrieval_hint.len() > 2048
        || item.kind.as_deref().is_some_and(|kind| {
            !matches!(
                kind,
                "preference" | "constraint" | "decision" | "experience" | "procedure" | "state"
            )
        })
    {
        return Err(MemoryError::GeneratedOutput(
            "memory_selection_invalid_metadata",
        ));
    }
    Ok(())
}

/// Resolve only supplied identities; never accept model-written evidence.
/// A selected ancestor subsumes its descendants, so one source does not emit
/// overlapping copies of the very same bytes as separate memory units.
pub fn resolve_selection(
    candidates: &[SourceUnit],
    output: SelectionOutput,
) -> Result<Vec<(SourceUnit, UnitSelection)>, MemoryError> {
    let mut seen = HashSet::new();
    let mut selected = Vec::new();
    for item in output.selections {
        if !seen.insert(item.unit_id.clone()) {
            return Err(MemoryError::GeneratedOutput(
                "memory_selection_duplicate_id",
            ));
        }
        validate_unit_selection_metadata(&item)?;
        let unit = candidates
            .iter()
            .find(|unit| unit.id == item.unit_id)
            .ok_or(MemoryError::GeneratedOutput("memory_selection_unknown_id"))?;
        selected.push((unit.clone(), item));
    }
    selected.sort_by_key(|(unit, _)| (unit.body.start_byte, usize::MAX - unit.body.end_byte));
    let mut result: Vec<(SourceUnit, UnitSelection)> = Vec::new();
    for item in selected {
        if !result.iter().any(|(parent, _)| {
            parent.body.start_byte <= item.0.body.start_byte
                && parent.body.end_byte >= item.0.body.end_byte
        }) {
            result.push(item);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_order_default_and_all_ancestor_context() {
        let source = "Only for project A.\n\n# GPU practice\nOnly on the test host.\n\n## Procedure\n1. First check `torch.cuda.is_available()`.\n2. Then move the model with `.to(device)`.\n3. Verify its device.\n\n## TUI\nNormal TUI requests use V1.\n";
        let units = source_units(source);
        let procedure = units
            .iter()
            .find(|u| u.headings.last().is_some_and(|h| h == "Procedure"))
            .unwrap();
        assert_eq!(procedure.context.len(), 2);
        assert_eq!(
            procedure.body.text,
            source[procedure.body.start_byte..procedure.body.end_byte]
        );
        assert!(procedure.complete_text().contains("Only for project A."));
        assert!(procedure.complete_text().contains("Only on the test host."));
        assert!(procedure.body.text.contains("First check"));
        assert!(procedure.body.text.contains("Then move"));
        assert!(procedure.body.text.contains("Verify its device"));
        let tui = units
            .iter()
            .find(|u| u.headings.last().is_some_and(|h| h == "TUI"))
            .unwrap();
        assert!(tui.body.text.contains("Normal TUI requests use V1."));
    }

    #[test]
    fn code_headings_tables_unicode_and_negation_are_not_split() {
        let source = "---\ntitle: 实测\n---\n# 条件\n不要在生产执行。\n\n```sh\n# this is code\nprintf 'Ａa'\n```\n\n| 条件 | 结果 |\n| --- | --- |\n| 特例 | 禁止 |\n";
        let units = source_units(source);
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].body.text,
            &source[source.find("# 条件").unwrap()..]
        );
        assert!(units[0].body.text.contains("Ａa"));
        assert!(!units[0].complete_text().contains("title:"));
    }

    #[test]
    fn rejects_invented_evidence_and_coalesces_exact_section_containment() {
        let units = source_units("# Parent\nScope.\n## Child\nComplete procedure.\n");
        let select = |id: String| UnitSelection {
            unit_id: id,
            kind: None,
            retrieval_hint: String::new(),
        };
        assert!(
            resolve_selection(
                &units,
                SelectionOutput {
                    selections: vec![select("forged".into())]
                }
            )
            .is_err()
        );
        let result = resolve_selection(
            &units,
            SelectionOutput {
                selections: units.iter().map(|u| select(u.id.clone())).collect(),
            },
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].0.body.text,
            "# Parent\nScope.\n## Child\nComplete procedure.\n"
        );
        assert!(
            serde_json::from_str::<SelectionOutput>(r#"{"selections":[],"content":"invented"}"#)
                .is_err()
        );
    }

    #[test]
    fn unheaded_documents_and_crlf_keep_exact_bytes() {
        let source = "仅在开发环境。\r\n\r\n先检查，再执行；否则停止。\r\n";
        let units = source_units(source);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].body.text, source);
        assert_eq!(units[0].body.start_line, 1);
        assert_eq!(units[0].body.end_line, 3);
        assert!(source_units("").is_empty());
    }
    #[test]
    fn multibyte_final_characters_without_final_newline_keep_exact_spans() {
        for newline in ["\n", "\r\n", "\r"] {
            for tail in ["。", "🧪", "é", "e\u{301}"] {
                let source = format!(
                    "# 范围{newline}只在测试环境。{newline}## 实践{newline}已经按顺序完成{tail}"
                );
                let units = source_units(&source);
                assert_eq!(units.len(), 2);
                assert_eq!(units[0].body.text, source);
                assert_eq!(units[1].complete_text(), source);
                assert_eq!(units[1].body.start_line, 3);
                assert_eq!(units[1].body.end_line, 4);
                for unit in units {
                    assert_eq!(
                        unit.body.text,
                        &source[unit.body.start_byte..unit.body.end_byte]
                    );
                }
                let standalone = source_units(tail);
                assert_eq!(standalone[0].body.text, tail);
                assert_eq!(standalone[0].body.end_line, 1);
            }
        }
    }

    #[test]
    fn minimal_selection_units_keep_parent_intro_and_leaf_subtrees() {
        let source =
            "# Project\n只适用于 Alpha。\n## Deploy\n先备份。\n## Rollback\n失败则恢复。\n";
        let full = source_units(source);
        let minimal = minimal_selection_units(source);
        assert_eq!(full.len(), 3);
        assert_eq!(minimal.len(), 3);
        assert_eq!(minimal[0].headings, vec!["Project"]);
        assert_eq!(minimal[0].body.text, "# Project\n只适用于 Alpha。\n");
        assert_ne!(minimal[0].body.end_byte, full[0].body.end_byte);
        assert_eq!(minimal[1].headings, vec!["Project", "Deploy"]);
        assert_eq!(minimal[2].headings, vec!["Project", "Rollback"]);
        assert!(minimal[1].complete_text().contains("只适用于 Alpha"));
        assert!(minimal[2].complete_text().contains("只适用于 Alpha"));
        assert!(
            !minimal
                .iter()
                .any(|unit| unit.body.text == full[0].body.text)
        );
    }

    #[test]
    fn minimal_selection_units_omit_heading_only_parent_introductions() {
        let source = "# Project\n## Deploy\n先备份。\n## Rollback\n失败则恢复。\n";
        let minimal = minimal_selection_units(source);
        assert_eq!(minimal.len(), 2);
        assert!(minimal.iter().all(|unit| unit.headings.len() == 2));
        assert!(minimal.iter().all(|unit| unit.body.text != "# Project\n"));
    }

    #[test]
    fn minimal_selection_units_use_byte_nesting_for_duplicate_headings_and_jumps() {
        let source = "# A\n范围 A。\n### Same\nA 内容。\n# B\n范围 B。\n### Same\nB 内容。\n";
        let minimal = minimal_selection_units(source);
        assert_eq!(minimal.len(), 4);
        assert_eq!(minimal[0].body.text, "# A\n范围 A。\n");
        assert_eq!(minimal[1].body.text, "### Same\nA 内容。\n");
        assert_eq!(minimal[2].body.text, "# B\n范围 B。\n");
        assert_eq!(minimal[3].body.text, "### Same\nB 内容。\n");
        assert!(minimal[1].complete_text().contains("范围 A"));
        assert!(minimal[3].complete_text().contains("范围 B"));
        assert!(!minimal[1].complete_text().contains("范围 B"));
    }

    #[test]
    fn minimal_selection_units_handle_single_line_preface_and_setext_headings() {
        let preface = minimal_selection_units("只有一行前言");
        assert_eq!(preface.len(), 1);
        assert_eq!(preface[0].body.text, "只有一行前言");

        let setext_without_intro = "Parent\n======\n## Child\n内容。\n";
        let minimal = minimal_selection_units(setext_without_intro);
        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].headings, vec!["Parent", "Child"]);

        let setext_with_intro = "Parent\n======\n说明范围。\n## Child\n内容。\n";
        let minimal = minimal_selection_units(setext_with_intro);
        assert_eq!(minimal.len(), 2);
        assert_eq!(minimal[0].body.text, "Parent\n======\n说明范围。\n");
        assert_eq!(minimal[1].headings, vec!["Parent", "Child"]);
    }
}
