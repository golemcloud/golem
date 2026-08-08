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
use std::collections::BTreeMap;
use tree_sitter::{Parser, Tree};

pub fn validate(source: &str) -> anyhow::Result<()> {
    let _ = parse_go(source)?;
    Ok(())
}

/// Merge the imports of `update`'s `main.go` into `current`'s, rewriting the
/// import block into gofmt-canonical form: a single, lexically sorted,
/// tab-indented `import (…)` group. `func main` and any comments around the
/// import block are left untouched.
///
/// Composing Go agent templates means unioning the blank imports of the agent
/// packages into the component's single `main.go`. Go's `gofmt` is strict about import
/// grouping and ordering, so rather than text-inserting (which would leave an
/// unsorted block that compiles but fails a format check) we rebuild the block
/// canonically — clean by construction, mirroring how `main_rs.rs` rewrites its
/// `mod`/`pub use` block.
pub fn merge_imports(current: &str, update: &str) -> anyhow::Result<String> {
    let current_tree = parse_go(current)?;
    let update_tree = parse_go(update)?;

    // Keyed by import path (quoted, as written) so a repeated import dedups; the
    // value is the full spec text (`_ "path"`, `alias "path"`, or `"path"`).
    let mut specs: BTreeMap<String, String> = BTreeMap::new();
    collect_import_specs(current, &current_tree, &mut specs);
    let before = specs.len();
    collect_import_specs(update, &update_tree, &mut specs);
    if specs.len() == before {
        // update contributes no new import — nothing to merge.
        return Ok(current.to_string());
    }

    let (start, end) = import_span(&current_tree)
        .ok_or_else(|| anyhow!("main.go has no import declaration to merge into"))?;

    let mut block = String::from("import (\n");
    for spec in specs.values() {
        block.push('\t');
        block.push_str(spec);
        block.push('\n');
    }
    block.push(')');

    let mut output = String::with_capacity(current.len() + block.len());
    output.push_str(&current[..start]);
    output.push_str(&block);
    output.push_str(&current[end..]);
    Ok(output)
}

fn parse_go(source: &str) -> anyhow::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|_| anyhow!("Failed to load tree-sitter-go"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("Failed to parse Go"))?;
    if tree.root_node().has_error() {
        return Err(anyhow!("Invalid Go"));
    }
    Ok(tree)
}

/// Walks every `import_spec` in the tree, recording (path → spec text). The path
/// is the quoted string as written (a stable dedup key that also sorts by path,
/// since all our imports are blank); the spec preserves any name/alias so a
/// non-blank import in main.go survives a merge.
fn collect_import_specs(source: &str, tree: &Tree, out: &mut BTreeMap<String, String>) {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_spec" {
            if let Some(path_node) = node.child_by_field_name("path") {
                let path = &source[path_node.start_byte()..path_node.end_byte()];
                let spec = match node.child_by_field_name("name") {
                    Some(name_node) => {
                        let name = &source[name_node.start_byte()..name_node.end_byte()];
                        format!("{name} {path}")
                    }
                    None => path.to_string(),
                };
                out.insert(path.to_string(), spec);
            }
        }
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Byte span covering the import declaration(s) at the top level, from the first
/// `import` keyword to the end of the last import declaration. `main.go` carries
/// a single block, but spanning first→last keeps a stray second declaration from
/// surviving the rewrite.
fn import_span(tree: &Tree) -> Option<(usize, usize)> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut start = None;
    let mut end = None;
    for child in root.named_children(&mut cursor) {
        if child.kind() == "import_declaration" {
            if start.is_none() {
                start = Some(child.start_byte());
            }
            end = Some(child.end_byte());
        }
    }
    start.zip(end)
}
