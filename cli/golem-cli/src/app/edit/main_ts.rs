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

pub fn add_import(source: &str, import_stmt: &str) -> anyhow::Result<String> {
    let tree = parse_ts(source)?;
    let insert_at = last_import_end(source, &tree);
    let mut output = String::with_capacity(source.len() + import_stmt.len() + 2);
    output.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        output.push('\n');
    }
    output.push_str(import_stmt);
    if !import_stmt.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&source[insert_at..]);
    Ok(output)
}

pub fn merge_imports(current: &str, update: &str) -> anyhow::Result<String> {
    let current_imports = import_statements(current)?;
    let update_imports = import_statements(update)?;

    let mut existing = BTreeSet::new();
    for import in current_imports {
        existing.insert(import);
    }

    let mut merged = current.to_string();
    for import in update_imports {
        if import.is_empty() || existing.contains(&import) {
            continue;
        }
        merged = add_import(&merged, &import)?;
        existing.insert(import);
    }

    Ok(merged)
}

pub fn validate(source: &str) -> anyhow::Result<()> {
    parse_ts(source).map(|_| ())
}

fn parse_ts(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-typescript"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse TypeScript"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Invalid TypeScript"));
    }
    Ok(tree)
}

fn import_statements(source: &str) -> anyhow::Result<Vec<String>> {
    let tree = parse_ts(source)?;
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    let mut imports = Vec::new();
    while let Some(node) = stack.pop() {
        if node.kind() == "import_statement" {
            let text = source[node.start_byte()..node.end_byte()]
                .trim()
                .to_string();
            imports.push((node.start_byte(), text));
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    imports.sort_by_key(|(start, _)| *start);
    Ok(imports.into_iter().map(|(_, text)| text).collect())
}

fn last_import_end(source: &str, tree: &Tree) -> usize {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut last_end = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_statement" {
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
    source.len()
}

fn line_end_at(source: &str, pos: usize) -> usize {
    source[pos..]
        .find('\n')
        .map(|idx| pos + idx + 1)
        .unwrap_or_else(|| source.len())
}
