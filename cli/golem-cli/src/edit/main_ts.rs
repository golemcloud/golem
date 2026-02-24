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

use anyhow::anyhow;
use tree_sitter::{Parser, Tree};

pub fn add_reexport(source: &str, export_stmt: &str) -> anyhow::Result<String> {
    let tree = parse_ts(source)?;
    let insert_at = last_export_end(source, &tree);
    let mut output = String::with_capacity(source.len() + export_stmt.len() + 2);
    output.push_str(&source[..insert_at]);
    if !source[..insert_at].ends_with('\n') {
        output.push('\n');
    }
    output.push_str(export_stmt);
    if !export_stmt.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&source[insert_at..]);
    Ok(output)
}

fn parse_ts(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-typescript"))?;
    parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse TypeScript"))
}

fn last_export_end(source: &str, tree: &Tree) -> usize {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut last_end = None;
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_statement" {
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
