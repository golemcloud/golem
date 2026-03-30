// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::app::edit::text::{TextEdit, apply_edits};
use anyhow::anyhow;
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser, Tree};

pub fn collect_value_text_by_path(source: &str, path: &[&str]) -> anyhow::Result<Option<String>> {
    let tree = parse_json(source)?;
    let root = root_object(&tree, source)?;
    let Some(value) = find_value_by_path(source, root, path)? else {
        return Ok(None);
    };
    Ok(Some(
        source[value.start_byte()..value.end_byte()]
            .trim()
            .to_string(),
    ))
}

pub fn merge_object(base_source: &str, update_source: &str) -> anyhow::Result<String> {
    let base_tree = parse_json(base_source)?;
    let update_tree = parse_json(update_source)?;
    let base_root = root_object(&base_tree, base_source)?;
    let update_root = root_object(&update_tree, update_source)?;

    let mut edits = Vec::new();
    merge_object_nodes(
        base_source,
        base_root,
        update_source,
        update_root,
        &mut edits,
    )?;
    apply_edits(base_source, edits)
}

pub fn update_object_entries(
    source: &str,
    object_key: &str,
    entries: &[(String, String)],
) -> anyhow::Result<String> {
    let tree = parse_json(source)?;
    let root = root_object(&tree, source)?;

    let mut edits = Vec::new();
    if let Some(value) = find_value_by_path(source, root, &[object_key])? {
        if value.kind() != "object" {
            return Err(anyhow!("{} is not an object", object_key));
        }
        edits.extend(merge_entries_into_object(source, value, entries)?);
    } else {
        let (pair_indent, _) = detect_object_indent(source, root)?;
        let object_literal = format_new_object_literal(pair_indent.as_str(), entries)?;
        edits.push(insert_object_pair_edit(
            source,
            root,
            object_key,
            &object_literal,
        )?);
    }
    apply_edits(source, edits)
}

pub fn collect_object_entries(
    source: &str,
    object_key: &str,
    names: &[&str],
) -> anyhow::Result<BTreeMap<String, Option<String>>> {
    let tree = parse_json(source)?;
    let root = root_object(&tree, source)?;
    let mut result = BTreeMap::new();
    for name in names {
        result.insert((*name).to_string(), None);
    }
    let Some(value) = find_value_by_path(source, root, &[object_key])? else {
        return Ok(result);
    };
    if value.kind() != "object" {
        return Ok(result);
    }
    for (key, value_node) in object_pairs(value, source)? {
        if names.contains(&key.as_str()) {
            let text = source[value_node.start_byte()..value_node.end_byte()]
                .trim()
                .to_string();
            result.insert(key, Some(text));
        }
    }
    Ok(result)
}

fn parse_json(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_json::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-json"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse JSON"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Invalid JSON"));
    }
    if contains_comment(tree.root_node()) {
        return Err(anyhow!("Comments are not allowed in JSON"));
    }
    Ok(tree)
}

fn contains_comment(root: Node<'_>) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            return true;
        }
        for idx in 0..node.child_count() {
            if let Some(child) = node.child(idx as u32) {
                stack.push(child);
            }
        }
    }
    false
}

fn root_object<'a>(tree: &'a Tree, _source: &str) -> anyhow::Result<Node<'a>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "object" {
            return Ok(node);
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    Err(anyhow!("No object found in JSON source"))
}

fn find_value_by_path<'a>(
    source: &str,
    mut object: Node<'a>,
    path: &[&str],
) -> anyhow::Result<Option<Node<'a>>> {
    for (index, key) in path.iter().enumerate() {
        let mut found = None;
        for (pair_key, value_node) in object_pairs(object, source)? {
            if pair_key == *key {
                found = Some(value_node);
                break;
            }
        }
        let Some(node) = found else {
            return Ok(None);
        };
        if index == path.len() - 1 {
            return Ok(Some(node));
        }
        if node.kind() != "object" {
            return Ok(None);
        }
        object = node;
    }
    Ok(None)
}

fn object_pairs<'a>(object: Node<'a>, source: &str) -> anyhow::Result<Vec<(String, Node<'a>)>> {
    Ok(object_pair_nodes(object, source)?
        .into_iter()
        .map(|(key, _pair, value)| (key, value))
        .collect())
}

fn merge_object_nodes(
    base_source: &str,
    base_object: Node<'_>,
    update_source: &str,
    update_object: Node<'_>,
    edits: &mut Vec<TextEdit>,
) -> anyhow::Result<()> {
    for (key, update_value) in object_pairs(update_object, update_source)? {
        let mut base_value = None;
        for (base_key, value) in object_pairs(base_object, base_source)? {
            if base_key == key {
                base_value = Some(value);
                break;
            }
        }
        if let Some(base_value) = base_value {
            if base_value.kind() == "object" && update_value.kind() == "object" {
                merge_object_nodes(base_source, base_value, update_source, update_value, edits)?;
            } else {
                let replacement =
                    update_source[update_value.start_byte()..update_value.end_byte()].to_string();
                edits.push(TextEdit::new(
                    base_value.start_byte(),
                    base_value.end_byte(),
                    replacement,
                ));
            }
        } else {
            let update_text =
                update_source[update_value.start_byte()..update_value.end_byte()].to_string();
            edits.push(insert_object_pair_edit(
                base_source,
                base_object,
                &key,
                &update_text,
            )?);
        }
    }
    Ok(())
}

fn merge_entries_into_object(
    source: &str,
    object: Node<'_>,
    entries: &[(String, String)],
) -> anyhow::Result<Vec<TextEdit>> {
    let mut edits = Vec::new();
    let pairs = object_pair_nodes(object, source)?;

    let mut existing = BTreeMap::new();
    for (key, _pair, value_node) in &pairs {
        existing.insert(key.clone(), *value_node);
    }

    let mut missing = Vec::<(String, String)>::new();
    for (key, value) in entries {
        if let Some(value_node) = existing.get(key) {
            edits.push(TextEdit::new(
                value_node.start_byte(),
                value_node.end_byte(),
                format!("\"{}\"", escape_json_string(value)),
            ));
        } else {
            missing.push((key.clone(), format!("\"{}\"", escape_json_string(value))));
        }
    }

    if !missing.is_empty() {
        let (indent, multiline) = detect_object_indent(source, object)?;
        if pairs.is_empty() {
            let insert_pos = object.end_byte() - 1;
            let insertion = if multiline {
                let closing_indent = indent.strip_suffix("  ").unwrap_or("");
                let body = missing
                    .iter()
                    .map(|(key, value)| {
                        format!("{}\"{}\": {}", indent, escape_json_string(key), value)
                    })
                    .collect::<Vec<_>>()
                    .join(",\n");
                format!("\n{}\n{}", body, closing_indent)
            } else {
                let body = missing
                    .iter()
                    .map(|(key, value)| format!("\"{}\": {}", escape_json_string(key), value))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" {}", body)
            };
            edits.push(TextEdit::new(insert_pos, insert_pos, insertion));
        } else {
            let last_pair_end = pairs.last().unwrap().1.end_byte();

            let rendered = if multiline {
                missing
                    .iter()
                    .map(|(key, value)| {
                        format!("\n{}\"{}\": {}", indent, escape_json_string(key), value)
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            } else {
                missing
                    .iter()
                    .map(|(key, value)| format!("\"{}\": {}", escape_json_string(key), value))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            if let Some(comma_pos) = find_trailing_comma(source, last_pair_end, object.end_byte()) {
                let insertion = if multiline {
                    rendered
                } else {
                    format!(" {}", rendered)
                };
                edits.push(TextEdit::new(comma_pos + 1, comma_pos + 1, insertion));
            } else {
                let insertion = if multiline {
                    format!(",{}", rendered)
                } else {
                    format!(", {}", rendered)
                };
                edits.push(TextEdit::new(last_pair_end, last_pair_end, insertion));
            }
        }
    }

    Ok(edits)
}

fn format_new_object_literal(
    pair_indent: &str,
    entries: &[(String, String)],
) -> anyhow::Result<String> {
    let value_indent = format!("{}  ", pair_indent);
    let mut parts = Vec::new();
    for (key, value) in entries {
        parts.push(format!(
            "{}\"{}\": \"{}\"",
            value_indent,
            escape_json_string(key),
            escape_json_string(value)
        ));
    }
    if parts.is_empty() {
        return Ok("{}".to_string());
    }
    Ok(format!("{{\n{}\n{}}}", parts.join(",\n"), pair_indent))
}

fn insert_object_pair_edit(
    source: &str,
    object: Node<'_>,
    key: &str,
    value_literal: &str,
) -> anyhow::Result<TextEdit> {
    let (indent, multiline) = detect_object_indent(source, object)?;
    let pairs = object_pair_nodes(object, source)?;
    if pairs.is_empty() {
        let insert_pos = object.end_byte() - 1;
        let insertion = if multiline {
            format!(
                "\n{}\"{}\": {}",
                indent,
                escape_json_string(key),
                value_literal
            )
        } else {
            format!(" \"{}\": {}", escape_json_string(key), value_literal)
        };
        return Ok(TextEdit::new(insert_pos, insert_pos, insertion));
    }

    let last_pair_end = pairs.last().unwrap().1.end_byte();
    if let Some(comma_pos) = find_trailing_comma(source, last_pair_end, object.end_byte()) {
        let insert_pos = comma_pos + 1;
        let insertion = if multiline {
            format!(
                "\n{}\"{}\": {}",
                indent,
                escape_json_string(key),
                value_literal
            )
        } else {
            format!(" \"{}\": {}", escape_json_string(key), value_literal)
        };
        return Ok(TextEdit::new(insert_pos, insert_pos, insertion));
    }

    let insert_pos = last_pair_end;
    let insertion = if multiline {
        format!(
            ",\n{}\"{}\": {}",
            indent,
            escape_json_string(key),
            value_literal
        )
    } else {
        format!(", \"{}\": {}", escape_json_string(key), value_literal)
    };
    Ok(TextEdit::new(insert_pos, insert_pos, insertion))
}

fn detect_object_indent(source: &str, object: Node<'_>) -> anyhow::Result<(String, bool)> {
    let mut cursor = object.walk();
    let mut first_pair = None;
    for child in object.named_children(&mut cursor) {
        if child.kind() == "pair" {
            first_pair = Some(child);
            break;
        }
    }
    if let Some(pair) = first_pair {
        let line_start = line_start_at(source, pair.start_byte());
        let before_pair = &source[line_start..pair.start_byte()];
        let indent = if let Some(brace_pos) = before_pair.rfind('{') {
            let leading = &before_pair[..brace_pos];
            if leading.trim().is_empty() {
                format!("{}  ", leading)
            } else {
                before_pair.to_string()
            }
        } else {
            before_pair.to_string()
        };
        return Ok((
            indent,
            source[object.start_byte()..object.end_byte()].contains('\n'),
        ));
    }
    let line_start = line_start_at(source, object.start_byte());
    let line = &source[line_start..];
    let base_indent = line
        .chars()
        .take_while(|ch| ch.is_whitespace() && *ch != '\n' && *ch != '\r')
        .collect::<String>();
    let object_text = &source[object.start_byte()..object.end_byte()];
    let multiline = if object_text.contains('\n') {
        true
    } else {
        let trimmed = object_text.trim();
        let inner = trimmed
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or(trimmed);
        inner.trim().is_empty()
    };
    Ok((format!("{}  ", base_indent), multiline))
}

fn object_pair_nodes<'a>(
    object: Node<'a>,
    source: &str,
) -> anyhow::Result<Vec<(String, Node<'a>, Node<'a>)>> {
    let mut cursor = object.walk();
    let mut pairs = Vec::new();
    for child in object.named_children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        let key_node = child
            .child_by_field_name("key")
            .ok_or_else(|| anyhow!("Missing key in JSON pair"))?;
        let value_node = child
            .child_by_field_name("value")
            .ok_or_else(|| anyhow!("Missing value in JSON pair"))?;
        let key_text = source[key_node.start_byte()..key_node.end_byte()].trim();
        let key = unquote_json_string(key_text)?;
        pairs.push((key, child, value_node));
    }
    Ok(pairs)
}

fn find_trailing_comma(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut idx = start;
    while idx < end {
        let ch = bytes[idx] as char;
        if ch.is_whitespace() {
            idx += 1;
            continue;
        }
        if ch == '/' && idx + 1 < end {
            let next = bytes[idx + 1] as char;
            if next == '/' {
                idx += 2;
                while idx < end && bytes[idx] as char != '\n' {
                    idx += 1;
                }
                continue;
            }
            if next == '*' {
                idx += 2;
                while idx + 1 < end {
                    let c = bytes[idx] as char;
                    let c_next = bytes[idx + 1] as char;
                    if c == '*' && c_next == '/' {
                        idx += 2;
                        break;
                    }
                    idx += 1;
                }
                continue;
            }
        }
        if ch == ',' {
            return Some(idx);
        }
        break;
    }
    None
}

fn line_start_at(source: &str, pos: usize) -> usize {
    source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
}

fn unquote_json_string(value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    if !trimmed.starts_with('"') || !trimmed.ends_with('"') {
        return Err(anyhow!("Expected JSON string, got {}", value));
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    Ok(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
