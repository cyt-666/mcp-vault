//! Model-facing tool DTOs. Business services and Admin retain their full records.
use serde_json::Value;

fn retain(value: &mut Value, keys: &[&str]) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, value| keys.contains(&key.as_str()) && !value.is_null());
    }
}
fn each(value: &mut Value, f: fn(&mut Value)) {
    if let Some(items) = value.as_array_mut() {
        for item in items {
            f(item);
        }
    }
}
fn note(value: &mut Value) {
    retain(
        value,
        &[
            "file_id",
            "path",
            "revision",
            "title",
            "snippet",
            "resource_uri",
            "matched_section",
            "result_granularity",
        ],
    );
}
fn memory(value: &mut Value) {
    retain(
        value,
        &[
            "id",
            "revision",
            "ownership",
            "content",
            "memory_type",
            "importance",
            "confidence",
            "valid_from",
            "valid_to",
            "sources",
            "source_count",
        ],
    );
    if let Some(sources) = value.get_mut("sources").and_then(Value::as_array_mut) {
        for source in sources.iter_mut() {
            retain(
                source,
                &[
                    "source_type",
                    "file_id",
                    "path",
                    "revision",
                    "heading",
                    "start_line",
                    "end_line",
                ],
            );
        }
        sources.retain(|source| source.get("path").is_some());
        sources.dedup();
    }
}
fn revision(value: &mut Value) {
    retain(
        value,
        &[
            "revision",
            "operation",
            "path_before",
            "path_after",
            "created_at",
        ],
    );
}
fn node(value: &mut Value) {
    retain(
        value,
        &[
            "id",
            "type",
            "title",
            "summary",
            "note_count",
            "children",
            "note_candidates",
        ],
    );
    if let Some(children) = value.get_mut("children") {
        each(children, node);
    }
    if let Some(notes) = value.get_mut("note_candidates") {
        each(notes, note);
    }
}

pub(super) fn tool_data(tool: &str, mut data: Value, details: bool) -> Value {
    // Single-record get is the intentional drill-down. Extended mode preserves
    // the prior wire fields for clients needing exact provenance or diagnostics.
    if details || tool == "get_memory" {
        return data;
    }
    match tool {
        "get_memory_overview" => {
            retain(
                &mut data,
                &[
                    "navigation_kind",
                    "generated_sections",
                    "entries",
                    "next_after_id",
                    "truncated",
                    "degraded",
                ],
            );
        }
        "recall" => {
            retain(
                &mut data,
                &[
                    "memories",
                    "pointers",
                    "related_notes",
                    "truncated",
                    "degraded",
                ],
            );
            if let Some(items) = data.get_mut("memories") {
                each(items, memory);
            }
            if let Some(items) = data.get_mut("related_notes") {
                each(items, note);
            }
        }
        "list_memories" => {
            if let Some(items) = data.get_mut("memories") {
                each(items, memory);
            }
        }
        "remember" => {
            if let Some(item) = data.get_mut("memory") {
                memory(item);
            }
        }
        "update_memory" => memory(&mut data),
        "search_notes" => {
            retain(
                &mut data,
                &[
                    "results",
                    "mode",
                    "result_granularity",
                    "next_cursor",
                    "truncated",
                    "degraded",
                    "degradation_reasons",
                    "coverage",
                ],
            );
            if let Some(items) = data.get_mut("results") {
                each(items, note);
            }
        }
        "browse_index" => {
            retain(
                &mut data,
                &[
                    "node",
                    "children",
                    "note_candidates",
                    "coverage",
                    "next_cursor",
                    "truncated",
                ],
            );
            if let Some(items) = data.get_mut("children") {
                each(items, node);
            }
            if let Some(items) = data.get_mut("note_candidates") {
                each(items, note);
            }
        }
        "vault_overview" => {
            if let Some(vault) = data.get_mut("vault") {
                retain(vault, &["slug"]);
            }
            if let Some(index) = data.get_mut("index") {
                retain(index, &["coverage", "last_error"]);
            }
            if let Some(items) = data.get_mut("topics") {
                each(items, node);
            }
            if let Some(items) = data.get_mut("recent") {
                each(items, revision);
            }
        }
        "recent_changes" => {
            if let Some(items) = data.get_mut("changes") {
                each(items, revision);
            }
        }
        "note_history" => {
            if let Some(items) = data.get_mut("revisions") {
                each(items, revision);
            }
        }
        "create_note" | "edit_note" | "move_note" | "delete_note" | "restore_note_revision" => {
            if let Some(file) = data.get_mut("file") {
                retain(file, &["file_id", "path", "revision", "active"]);
            }
            if let Some(item) = data.get_mut("revision") {
                revision(item);
            }
        }
        // Exact reads and deletion receipts are already bounded and actionable.
        "read_note" | "forget_memory" => {}
        _ => {}
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compact_recall_keeps_complete_source_coordinates_and_oversize_pointers() {
        let source = json!({"path":"notes/完整.md","file_id":"source-1","revision":4,"heading":["条件","步骤"],"start_line":8,"end_line":14});
        let pointer = json!({"id":"large","revision":2,"resource_uri":"vault://memory/large","sources":[source.clone()],"reason":"complete_unit_exceeds_remaining_budget"});
        let body = "  ## 步骤\r\n1. 前置条件满足后执行。\r\n2. 不得交换次序。\r\n";
        let result = tool_data(
            "recall",
            json!({"memories":[{"id":"current","content":body,"sources":[source.clone()],"fact_count":2}],"pointers":[pointer.clone()]}),
            false,
        );
        assert_eq!(result["memories"][0]["content"], body);
        assert_eq!(result["memories"][0]["sources"], json!([source]));
        assert_eq!(result["pointers"], json!([pointer]));
        assert!(result["memories"][0].get("fact_count").is_none());
    }
}
