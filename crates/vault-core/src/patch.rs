//! Exact UTF-8 patch and heading-section transformations.

use crate::error::VaultError;

pub(crate) fn apply_unified_diff(original: &str, patch: &str) -> Result<String, VaultError> {
    let original_lines = split_lines(original);
    let mut output = Vec::new();
    let mut cursor = 0_usize;
    let mut saw_hunk = false;
    let lines: Vec<&str> = patch.lines().collect();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            index += 1;
            continue;
        }
        if !line.starts_with("@@ ") {
            index += 1;
            continue;
        }
        let (old_start, old_count, new_count) = parse_hunk_header(line)?;
        saw_hunk = true;
        let old_index = old_start
            .checked_sub(1)
            .ok_or(VaultError::InvalidPatch("hunk starts before the file"))?;
        if old_index < cursor || old_index > original_lines.len() {
            return Err(VaultError::InvalidPatch("hunk offset is not exact"));
        }
        output.extend(original_lines[cursor..old_index].iter().cloned());
        cursor = old_index;
        let mut consumed_old = 0_usize;
        let mut produced_new = 0_usize;
        index += 1;
        while index < lines.len() && !lines[index].starts_with("@@ ") {
            let hunk_line = lines[index];
            index += 1;
            if hunk_line.starts_with("\\ No newline") {
                continue;
            }
            let (kind, payload) = hunk_line
                .split_at_checked(1)
                .ok_or(VaultError::InvalidPatch("empty hunk line"))?;
            match kind.as_bytes()[0] {
                b' ' => {
                    let source = original_lines
                        .get(cursor)
                        .ok_or(VaultError::InvalidPatch("context exceeds file"))?;
                    if line_content(source) != payload {
                        return Err(VaultError::InvalidPatch("context does not match"));
                    }
                    output.push((*source).to_owned());
                    cursor += 1;
                    consumed_old += 1;
                    produced_new += 1;
                }
                b'-' => {
                    let source = original_lines
                        .get(cursor)
                        .ok_or(VaultError::InvalidPatch("removal exceeds file"))?;
                    if line_content(source) != payload {
                        return Err(VaultError::InvalidPatch("removal does not match"));
                    }
                    cursor += 1;
                    consumed_old += 1;
                }
                b'+' => {
                    output.push(with_newline(payload));
                    produced_new += 1;
                }
                _ => return Err(VaultError::InvalidPatch("unknown hunk line")),
            }
        }
        if consumed_old != old_count || produced_new != new_count {
            return Err(VaultError::InvalidPatch("hunk line counts do not match"));
        }
    }

    if !saw_hunk {
        return Err(VaultError::InvalidPatch("patch contains no hunks"));
    }
    output.extend(original_lines[cursor..].iter().cloned());
    Ok(output.concat())
}

pub(crate) fn insert_after_heading(
    original: &str,
    heading: &str,
    insertion: &str,
) -> Result<String, VaultError> {
    let mut lines = split_lines(original);
    let index = lines
        .iter()
        .position(|line| line_content(line) == heading)
        .ok_or(VaultError::InvalidPatch("heading was not found"))?;
    let insertion = split_lines(insertion);
    lines.splice(index + 1..index + 1, insertion);
    Ok(lines.concat())
}

pub(crate) fn replace_heading_section(
    original: &str,
    heading: &str,
    replacement: &str,
) -> Result<String, VaultError> {
    let mut lines = split_lines(original);
    let start = lines
        .iter()
        .position(|line| line_content(line) == heading)
        .ok_or(VaultError::InvalidPatch("heading was not found"))?;
    let level = heading_level(heading).ok_or(VaultError::InvalidPatch("heading is invalid"))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| heading_level(line_content(line)).is_some_and(|next| next <= level))
        .map_or(lines.len(), |(index, _)| index);
    let replacement = split_lines(replacement);
    lines.splice(start + 1..end, replacement);
    Ok(lines.concat())
}

fn parse_hunk_header(line: &str) -> Result<(usize, usize, usize), VaultError> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 3 {
        return Err(VaultError::InvalidPatch("hunk header is invalid"));
    }
    let old = parse_range(
        fields[1]
            .strip_prefix('-')
            .ok_or(VaultError::InvalidPatch("old hunk range is invalid"))?,
    )?;
    let new = parse_range(
        fields[2]
            .strip_prefix('+')
            .ok_or(VaultError::InvalidPatch("new hunk range is invalid"))?,
    )?;
    Ok((old.0, old.1, new.1))
}

fn parse_range(value: &str) -> Result<(usize, usize), VaultError> {
    let mut parts = value.splitn(2, ',');
    let start = parts
        .next()
        .ok_or(VaultError::InvalidPatch("hunk range start is missing"))?
        .parse()
        .map_err(|_| VaultError::InvalidPatch("hunk range start is invalid"))?;
    let count = parts.next().map_or(Ok(1), |count| {
        count
            .parse()
            .map_err(|_| VaultError::InvalidPatch("hunk range count is invalid"))
    })?;
    Ok((start, count))
}

fn split_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        Vec::new()
    } else {
        value.split_inclusive('\n').map(str::to_owned).collect()
    }
}

fn line_content(value: &str) -> &str {
    value
        .strip_suffix('\n')
        .unwrap_or(value)
        .strip_suffix('\r')
        .unwrap_or(value.strip_suffix('\n').unwrap_or(value))
}

fn with_newline(value: &str) -> String {
    format!("{value}\n")
}

fn heading_level(value: &str) -> Option<usize> {
    let level = value
        .chars()
        .take_while(|character| *character == '#')
        .count();
    (1..=6).contains(&level).then_some(level)
}
