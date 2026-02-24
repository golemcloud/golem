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
    let root = root_mapping(&tree)?;
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
            if value_node.kind() != "block_mapping" {
                docs.push(source[pair_node.start_byte()..pair_node.end_byte()].to_string());
                continue;
            }
            for (_, child_pair, _) in mapping_pairs(value_node, source)? {
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
    let root = root_mapping(&tree)?;
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
    let root = root_mapping(&tree)?;
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
    let root = root_mapping(&tree)?;
    let (pair_node, value_node) =
        find_pair_by_path(source, root, path)?.ok_or_else(|| anyhow!("Missing entry"))?;
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

fn parse_yaml(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_yaml::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-yaml"))?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse YAML"))
}

fn root_mapping<'a>(tree: &'a Tree) -> anyhow::Result<Node<'a>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "document" || child.kind() == "stream" {
            if let Some(mapping) = find_first_kind(child, "block_mapping") {
                return Ok(mapping);
            }
        }
    }
    find_first_kind(root, "block_mapping").ok_or_else(|| anyhow!("Missing root mapping"))
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
        if value_node.kind() != "block_mapping" {
            return Ok(None);
        }
        mapping = value_node;
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
