// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::edit::text::{apply_edits, TextEdit};
use anyhow::anyhow;
use std::collections::BTreeSet;
use tree_sitter::{Node, Parser, Tree};

pub fn split_documents(
    source: &str,
    split_root_keys: &[&str],
    split_map_keys: &[&str],
) -> anyhow::Result<Vec<String>> {
    let tree = parse_yaml(source)?;
    let root = root_mapping(&tree, source)?;
    let root_pairs = mapping_pairs(root, source)?;

    let split_root: BTreeSet<&str> = split_root_keys.iter().copied().collect();
    let split_map: BTreeSet<&str> = split_map_keys.iter().copied().collect();
    let split_all = split_root.is_empty();

    let mut docs = Vec::new();
    let mut unsplit_ranges = Vec::new();
    for (key, pair_node, value_node) in root_pairs {
        let should_split = split_all || split_root.contains(key.as_str());
        if !should_split {
            unsplit_ranges.push(pair_node.start_byte()..pair_node.end_byte());
            continue;
        }
        if split_map.contains(key.as_str()) {
            let header_end = line_end_at(source, pair_node.start_byte());
            let header = source[pair_node.start_byte()..header_end].to_string();
            let mapping_node = as_block_mapping(value_node).unwrap_or(value_node);
            if mapping_node.kind() != "block_mapping" {
                docs.push(source[pair_node.start_byte()..pair_node.end_byte()].to_string());
                continue;
            }
            for (_, child_pair, _) in mapping_pairs(mapping_node, source)? {
                let mut doc = String::new();
                doc.push_str(&header);
                if !header.ends_with('\n') {
                    doc.push('\n');
                }
                doc.push_str(&source[child_pair.start_byte()..child_pair.end_byte()]);
                docs.push(doc);
            }
        } else {
            docs.push(source[pair_node.start_byte()..pair_node.end_byte()].to_string());
        }
    }

    if !unsplit_ranges.is_empty() {
        let mut combined = String::new();
        for range in unsplit_ranges {
            combined.push_str(&source[range]);
        }
        docs.push(combined);
    }

    Ok(docs)
}

pub fn add_map_entry(
    source: &str,
    path: &[&str],
    key: &str,
    value_literal: &str,
) -> anyhow::Result<String> {
    let tree = parse_yaml(source)?;
    let root = root_mapping(&tree, source)?;
    let mapping =
        find_mapping_by_path(source, root, path)?.ok_or_else(|| anyhow!("Missing map at path"))?;

    let insert_pos = mapping.end_byte();
    let indent = child_indent(source, mapping)?;
    let mut insertion = String::new();
    if !source[..insert_pos].ends_with('\n') {
        insertion.push('\n');
    }
    insertion.push_str(&" ".repeat(indent));
    insertion.push_str(key);
    insertion.push_str(": ");
    insertion.push_str(value_literal);
    insertion.push('\n');
    apply_edits(
        source,
        vec![TextEdit::new(insert_pos, insert_pos, insertion)],
    )
}

pub fn remove_map_entry(source: &str, path: &[&str], key: &str) -> anyhow::Result<String> {
    let tree = parse_yaml(source)?;
    let root = root_mapping(&tree, source)?;
    let mapping =
        find_mapping_by_path(source, root, path)?.ok_or_else(|| anyhow!("Missing map at path"))?;
    for (pair_key, pair_node, _) in mapping_pairs(mapping, source)? {
        if pair_key == key {
            let mut end = pair_node.end_byte();
            if source[end..].starts_with('\n') {
                end += 1;
            }
            return apply_edits(source, vec![TextEdit::new(pair_node.start_byte(), end, "")]);
        }
    }
    Err(anyhow!("Missing entry {}", key))
}

pub fn set_scalar(source: &str, path: &[&str], value_literal: &str) -> anyhow::Result<String> {
    let tree = parse_yaml(source)?;
    let root = root_mapping(&tree, source)?;
    let pair = find_pair_by_path(source, root, path)?;
    let (pair_node, value_node) = if let Some(pair) = pair {
        pair
    } else if path.len() == 1 {
        find_pair_anywhere(source, tree.root_node(), path[0])?
            .ok_or_else(|| anyhow!("Missing entry"))?
    } else {
        return Err(anyhow!("Missing entry"));
    };
    let replacement_range = value_node.start_byte()..value_node.end_byte();
    let mut edits = Vec::new();
    edits.push(TextEdit::new(
        replacement_range.start,
        replacement_range.end,
        value_literal,
    ));
    if source[replacement_range.end..pair_node.end_byte()].starts_with('\n') {
        // keep newline if present
    }
    apply_edits(source, edits)
}

pub fn merge_documents(base: &str, update: &str) -> anyhow::Result<String> {
    let base_tree = parse_yaml(base)?;
    let update_tree = parse_yaml(update)?;

    let base_root = root_mapping(&base_tree, base)?;
    let update_root = root_mapping(&update_tree, update)?;

    let mut edits = Vec::new();
    merge_mapping_nodes(base, base_root, update, update_root, &mut edits)?;
    apply_edits(base, edits)
}

fn parse_yaml(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_yaml::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-yaml"))?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse YAML"))
}

fn root_mapping<'a>(tree: &'a Tree, source: &str) -> anyhow::Result<Node<'a>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut candidates = Vec::new();
    while let Some(node) = stack.pop() {
        if node.kind() == "block_mapping" {
            candidates.push(node);
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }

    let Some(first_non_ws) = tree
        .root_node()
        .utf8_text(source.as_bytes())
        .ok()
        .and_then(|text| text.find(|c: char| !c.is_whitespace()))
    else {
        return Err(anyhow!("Empty YAML"));
    };

    let mut last_non_ws = None;
    if let Ok(text) = tree.root_node().utf8_text(source.as_bytes()) {
        for (idx, ch) in text.char_indices() {
            if !ch.is_whitespace() {
                last_non_ws = Some(idx);
            }
        }
    }
    let last_non_ws = last_non_ws.unwrap_or(first_non_ws);

    for candidate in candidates.iter().rev() {
        if candidate.start_byte() <= first_non_ws && candidate.end_byte() >= last_non_ws {
            return Ok(*candidate);
        }
    }

    candidates
        .into_iter()
        .max_by_key(|node| node.end_byte().saturating_sub(node.start_byte()))
        .ok_or_else(|| anyhow!("Missing root mapping"))
}

fn find_first_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() == kind {
            return Some(current);
        }
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn as_block_mapping<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if node.kind() == "block_mapping" {
        return Some(node);
    }
    find_first_kind(node, "block_mapping")
}

fn mapping_pairs<'a>(
    mapping: Node<'a>,
    source: &str,
) -> anyhow::Result<Vec<(String, Node<'a>, Node<'a>)>> {
    let mut cursor = mapping.walk();
    let mut pairs = Vec::new();
    for child in mapping.named_children(&mut cursor) {
        if child.kind() != "block_mapping_pair" {
            continue;
        }
        let key_node = child
            .child_by_field_name("key")
            .ok_or_else(|| anyhow!("Missing key in YAML pair"))?;
        let value_node = child
            .child_by_field_name("value")
            .ok_or_else(|| anyhow!("Missing value in YAML pair"))?;
        let key_text = source[key_node.start_byte()..key_node.end_byte()]
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string();
        pairs.push((key_text, child, value_node));
    }
    Ok(pairs)
}

fn find_mapping_by_path<'a>(
    source: &str,
    mut mapping: Node<'a>,
    path: &[&str],
) -> anyhow::Result<Option<Node<'a>>> {
    for key in path {
        let mut found = None;
        for (pair_key, _, value_node) in mapping_pairs(mapping, source)? {
            if pair_key == *key {
                found = Some(value_node);
                break;
            }
        }
        let Some(value_node) = found else {
            return Ok(None);
        };
        let mapping_node = as_block_mapping(value_node).unwrap_or(value_node);
        if mapping_node.kind() != "block_mapping" {
            return Ok(None);
        }
        mapping = mapping_node;
    }
    Ok(Some(mapping))
}

fn find_pair_by_path<'a>(
    source: &str,
    mapping: Node<'a>,
    path: &[&str],
) -> anyhow::Result<Option<(Node<'a>, Node<'a>)>> {
    if path.is_empty() {
        return Ok(None);
    }
    let parent = if path.len() == 1 {
        Some(mapping)
    } else {
        find_mapping_by_path(source, mapping, &path[..path.len() - 1])?
    };
    let Some(parent) = parent else {
        return Ok(None);
    };
    for (pair_key, pair_node, value_node) in mapping_pairs(parent, source)? {
        if pair_key == path[path.len() - 1] {
            return Ok(Some((pair_node, value_node)));
        }
    }
    Ok(None)
}

fn find_pair_anywhere<'a>(
    source: &str,
    root: Node<'a>,
    key: &str,
) -> anyhow::Result<Option<(Node<'a>, Node<'a>)>> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut best: Option<(usize, Node<'a>, Node<'a>)> = None;
    while let Some(node) = stack.pop() {
        if node.kind() == "block_mapping_pair" {
            let key_node = node
                .child_by_field_name("key")
                .ok_or_else(|| anyhow!("Missing key in YAML pair"))?;
            let value_node = node
                .child_by_field_name("value")
                .ok_or_else(|| anyhow!("Missing value in YAML pair"))?;
            let key_text = source[key_node.start_byte()..key_node.end_byte()]
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if key_text == key {
                let mut depth = 0usize;
                let mut current = node;
                while let Some(parent) = current.parent() {
                    if parent.kind() == "block_mapping_pair" {
                        depth += 1;
                    }
                    current = parent;
                }
                match best {
                    Some((best_depth, _, _)) if best_depth <= depth => {}
                    _ => best = Some((depth, node, value_node)),
                }
            }
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    Ok(best.map(|(_, pair, value)| (pair, value)))
}

fn child_indent(source: &str, mapping: Node<'_>) -> anyhow::Result<usize> {
    let mut cursor = mapping.walk();
    for child in mapping.named_children(&mut cursor) {
        if child.kind() == "block_mapping_pair" {
            let line_start = line_start_at(source, child.start_byte());
            return Ok(child.start_byte() - line_start);
        }
    }
    let line_start = line_start_at(source, mapping.start_byte());
    Ok(mapping.start_byte() - line_start + 2)
}

fn line_start_at(source: &str, pos: usize) -> usize {
    source[..pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
}

fn line_end_at(source: &str, pos: usize) -> usize {
    source[pos..]
        .find('\n')
        .map(|idx| pos + idx + 1)
        .unwrap_or_else(|| source.len())
}

fn merge_mapping_nodes(
    base_source: &str,
    base_mapping: Node<'_>,
    update_source: &str,
    update_mapping: Node<'_>,
    edits: &mut Vec<TextEdit>,
) -> anyhow::Result<()> {
    let base_pairs = mapping_pairs(base_mapping, base_source)?;
    let update_pairs = mapping_pairs(update_mapping, update_source)?;
    let mut base_index = std::collections::HashMap::new();
    for (idx, (key, pair_node, value_node)) in base_pairs.iter().enumerate() {
        base_index.insert(key.clone(), (idx, *pair_node, *value_node));
    }

    for (key, update_pair, update_value) in update_pairs {
        if let Some((_, _base_pair, base_value)) = base_index.get(&key) {
            merge_value_nodes(
                base_source,
                *base_value,
                update_source,
                update_value,
                edits,
            )?;
            continue;
        }

        let mut insertion = reindent_block(
            update_source,
            update_pair.start_byte(),
            update_pair.end_byte(),
            mapping_child_indent(base_source, base_mapping)?,
        );
        if !insertion.ends_with('\n') {
            insertion.push('\n');
        }
        let insert_pos = if let Some((_, pair_node, _)) = base_pairs.last() {
            pair_node.end_byte()
        } else {
            base_mapping.end_byte()
        };
        let prefix = if base_source[..insert_pos].ends_with('\n') {
            ""
        } else {
            "\n"
        };
        edits.push(TextEdit::new(
            insert_pos,
            insert_pos,
            format!("{prefix}{insertion}"),
        ));
    }
    Ok(())
}

fn merge_value_nodes(
    base_source: &str,
    base_value: Node<'_>,
    update_source: &str,
    update_value: Node<'_>,
    edits: &mut Vec<TextEdit>,
) -> anyhow::Result<()> {
    if let (Some(base_map), Some(update_map)) =
        (as_block_mapping(base_value), as_block_mapping(update_value))
    {
        return merge_mapping_nodes(base_source, base_map, update_source, update_map, edits);
    }
    if let (Some(base_seq), Some(update_seq)) =
        (as_block_sequence(base_value), as_block_sequence(update_value))
    {
        return merge_sequence_nodes(base_source, base_seq, update_source, update_seq, edits);
    }

    let replacement = update_source[update_value.start_byte()..update_value.end_byte()]
        .trim()
        .to_string();
    edits.push(TextEdit::new(
        base_value.start_byte(),
        base_value.end_byte(),
        replacement,
    ));
    Ok(())
}

fn merge_sequence_nodes(
    base_source: &str,
    base_seq: Node<'_>,
    update_source: &str,
    update_seq: Node<'_>,
    edits: &mut Vec<TextEdit>,
) -> anyhow::Result<()> {
    let base_items = sequence_items(base_seq);
    let update_items = sequence_items(update_seq);
    let mut existing = std::collections::HashSet::new();
    for item in &base_items {
        let text = normalize_block_text(base_source, item.start_byte(), item.end_byte());
        existing.insert(text);
    }

    let indent = sequence_item_indent(base_source, base_seq)?;
    let insert_pos = if let Some(last) = base_items.last() {
        last.end_byte()
    } else {
        base_seq.end_byte()
    };

    let mut inserts = String::new();
    for item in update_items {
        let text = normalize_block_text(update_source, item.start_byte(), item.end_byte());
        if existing.contains(&text) {
            continue;
        }
        existing.insert(text);
        let reindented = reindent_block(update_source, item.start_byte(), item.end_byte(), indent);
        if !inserts.is_empty() || base_source[..insert_pos].ends_with('\n') {
            inserts.push('\n');
        } else {
            inserts.push('\n');
        }
        inserts.push_str(&reindented);
    }

    if !inserts.is_empty() {
        edits.push(TextEdit::new(insert_pos, insert_pos, inserts));
    }
    Ok(())
}

fn sequence_items(sequence: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = sequence.walk();
    sequence
        .named_children(&mut cursor)
        .filter(|child| child.kind() == "block_sequence_item")
        .collect()
}

fn as_block_sequence<'a>(node: Node<'a>) -> Option<Node<'a>> {
    if node.kind() == "block_sequence" {
        return Some(node);
    }
    find_first_kind(node, "block_sequence")
}

fn mapping_child_indent(source: &str, mapping: Node<'_>) -> anyhow::Result<usize> {
    let mut cursor = mapping.walk();
    for child in mapping.named_children(&mut cursor) {
        if child.kind() == "block_mapping_pair" {
            return Ok(value_indent(source, child.start_byte()));
        }
    }
    let line_start = line_start_at(source, mapping.start_byte());
    Ok(mapping.start_byte() - line_start + 2)
}

fn sequence_item_indent(source: &str, sequence: Node<'_>) -> anyhow::Result<usize> {
    let items = sequence_items(sequence);
    if let Some(item) = items.first() {
        return Ok(value_indent(source, item.start_byte()));
    }
    let line_start = line_start_at(source, sequence.start_byte());
    Ok(sequence.start_byte() - line_start + 2)
}

fn value_indent(source: &str, pos: usize) -> usize {
    pos - line_start_at(source, pos)
}

fn reindent_block(source: &str, start: usize, end: usize, new_indent: usize) -> String {
    let text = &source[start..end];
    let old_indent = line_indent_at(source, start);
    let new_indent_str = " ".repeat(new_indent);
    let mut out = String::new();
    for (idx, line) in text.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if line.starts_with(&old_indent) {
            out.push_str(&new_indent_str);
            out.push_str(&line[old_indent.len()..]);
        } else if old_indent.trim().is_empty() {
            let leading = line.chars().take_while(|ch| *ch == ' ').count();
            let trimmed = &line[leading..];
            out.push_str(&new_indent_str);
            out.push_str(&" ".repeat(leading));
            out.push_str(trimmed);
        } else {
            out.push_str(&new_indent_str);
            out.push_str(line);
        }
    }
    out
}

fn normalize_block_text(source: &str, start: usize, end: usize) -> String {
    source[start..end].trim().replace(['\r', '\n'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn line_indent_at(source: &str, pos: usize) -> String {
    let line_start = line_start_at(source, pos);
    source[line_start..pos]
        .chars()
        .take_while(|ch| *ch == ' ')
        .collect()
}
