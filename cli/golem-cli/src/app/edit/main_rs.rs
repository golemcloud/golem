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

use anyhow::anyhow;
use std::collections::BTreeSet;
use tree_sitter::{Parser, Tree};

pub fn add_import_and_export(
    source: &str,
    import_stmt: &str,
    export_stmt: &str,
) -> anyhow::Result<String> {
    let tree = parse_rust(source)?;
    let insert_at = last_use_end(source, &tree);
    let mut output =
        String::with_capacity(source.len() + import_stmt.len() + export_stmt.len() + 4);
    output.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        output.push('\n');
    }
    output.push_str(import_stmt);
    if !import_stmt.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(export_stmt);
    if !export_stmt.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&source[insert_at..]);
    Ok(output)
}

pub fn validate(source: &str) -> anyhow::Result<()> {
    let _ = parse_rust(source)?;
    Ok(())
}

pub fn merge_reexports(current: &str, update: &str) -> anyhow::Result<String> {
    let _ = parse_rust(current)?;
    let _ = parse_rust(update)?;

    let mut known = collect_reexport_declarations(current)
        .into_iter()
        .map(|line| line.trim().to_string())
        .collect::<BTreeSet<_>>();

    let mut has_additions = false;
    for line in collect_reexport_declarations(update) {
        let normalized = line.trim().to_string();
        if known.insert(normalized) {
            has_additions = true;
        }
    }

    if !has_additions {
        return Ok(current.to_string());
    }

    let merged_mods = merge_declaration_kind(current, update, is_mod_declaration);
    let merged_pub_uses = merge_declaration_kind(current, update, is_pub_use_declaration);

    let mut declaration_block = Vec::new();
    declaration_block.extend(merged_mods);
    if !declaration_block.is_empty() && !merged_pub_uses.is_empty() {
        declaration_block.push(String::new());
    }
    declaration_block.extend(merged_pub_uses);

    if declaration_block.is_empty() {
        return Ok(current.to_string());
    }

    let mut output_lines = Vec::new();
    let mut inserted = false;
    for line in current.lines() {
        if is_reexport_declaration(line) {
            if !inserted {
                output_lines.extend(declaration_block.iter().cloned());
                inserted = true;
            }
            continue;
        }
        output_lines.push(line.to_string());
    }

    if !inserted {
        output_lines.splice(0..0, declaration_block);
    }

    let mut output = output_lines.join("\n");
    if current.ends_with('\n') {
        output.push('\n');
    }

    Ok(output)
}

fn merge_declaration_kind(current: &str, update: &str, predicate: fn(&str) -> bool) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut merged = Vec::new();

    for source in [current, update] {
        for line in source.lines() {
            let normalized = line.trim();
            if predicate(normalized) && seen.insert(normalized.to_string()) {
                merged.push(normalized.to_string());
            }
        }
    }

    merged
}

fn parse_rust(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-rust"))?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse Rust"))
}

fn last_use_end(source: &str, tree: &Tree) -> usize {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut last_end = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "use_declaration" {
            let end = node.end_byte();
            if last_end.map(|value| end > value).unwrap_or(true) {
                last_end = Some(end);
            }
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    if let Some(end) = last_end {
        return line_end_at(source, end);
    }
    0
}

fn is_mod_declaration(line: &str) -> bool {
    line.starts_with("mod ") && line.ends_with(';')
}

fn is_pub_use_declaration(line: &str) -> bool {
    line.starts_with("pub use ") && line.ends_with(';')
}

fn is_reexport_declaration(line: &str) -> bool {
    let line = line.trim();
    !line.starts_with("//") && (is_mod_declaration(line) || is_pub_use_declaration(line))
}

fn collect_reexport_declarations(source: &str) -> Vec<String> {
    source
        .lines()
        .map(str::trim)
        .filter(|line| {
            (line.starts_with("mod ") || line.starts_with("pub use "))
                && line.ends_with(';')
                && !line.starts_with("//")
        })
        .map(ToString::to_string)
        .collect()
}

fn line_end_at(source: &str, pos: usize) -> usize {
    source[pos..]
        .find('\n')
        .map(|idx| pos + idx + 1)
        .unwrap_or_else(|| source.len())
}
