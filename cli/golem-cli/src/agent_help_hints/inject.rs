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

//! Walks a clap [`Command`] tree and appends a `Relevant skills:` block to
//! the long help (`after_long_help`) of selected commands.

use crate::agent_help_hints::builtin_skill_map::{SKILL_BINDINGS, SkillBinding, SkillKind};
use crate::agent_help_hints::skill_discovery::{find_app_skill_root, skill_is_installed};
use clap::Command;
use std::collections::BTreeMap;
use std::mem;
use std::path::{Path, PathBuf};

/// Mutates the given clap [`Command`] tree in place by appending a
/// `Relevant skills:` block to the long help of every command that has at
/// least one matching skill installed under `<app_dir>/.agents/skills/`.
///
/// Cheap when:
///   - no application directory is found (no I/O beyond manifest walk),
///   - or no skills under that directory match any binding (one
///     `Path::is_file` per binding then early-exit with no mutation).
///
/// Always preserves any existing `after_long_help` content; the skills block
/// is appended below it with one blank line of separation.
pub fn augment_command_with_skill_links(cmd: &mut Command) {
    let Some(skill_root) = find_app_skill_root() else {
        return;
    };
    let by_path = collect_resolved_blocks(&skill_root);
    if by_path.is_empty() {
        return;
    }

    for (path, block) in by_path {
        if let Some(node) = navigate_mut(cmd, &path) {
            append_after_long_help(node, &block);
        }
    }
}

/// Resolved skill display block + a sort key per `cli_path`.
type ResolvedBlocks = BTreeMap<Vec<String>, String>;

fn collect_resolved_blocks(skill_root: &Path) -> ResolvedBlocks {
    // Group bindings by cli_path while preserving source order within a group.
    let mut groups: BTreeMap<Vec<String>, Vec<&SkillBinding>> = BTreeMap::new();
    for b in SKILL_BINDINGS {
        let key: Vec<String> = b.cli_path.iter().map(|s| s.to_string()).collect();
        groups.entry(key).or_default().push(b);
    }

    let mut blocks: ResolvedBlocks = BTreeMap::new();
    for (path, bindings) in groups {
        if let Some(block) = render_block_for(skill_root, &bindings) {
            blocks.insert(path, block);
        }
    }
    blocks
}

/// Renders the `Relevant skills:` block for one cli_path, or `None` if no
/// skill resolves on disk.
fn render_block_for(skill_root: &Path, bindings: &[&SkillBinding]) -> Option<String> {
    let mut entries = Vec::<RenderedEntry>::new();

    for b in bindings {
        match b.kind {
            SkillKind::Common => {
                if skill_is_installed(skill_root, b.basename) {
                    entries.push(RenderedEntry {
                        summary: b.summary,
                        items: vec![RenderedItem {
                            label: None,
                            url: file_url(&skill_root.join(b.basename).join("SKILL.md")),
                        }],
                    });
                }
            }
            SkillKind::PerLanguage(langs) => {
                let mut items = Vec::<RenderedItem>::new();
                for lang in langs {
                    let folder = format!("{}-{}", b.basename, lang.suffix());
                    if skill_is_installed(skill_root, &folder) {
                        items.push(RenderedItem {
                            label: Some(lang.display()),
                            url: file_url(&skill_root.join(&folder).join("SKILL.md")),
                        });
                    }
                }
                if !items.is_empty() {
                    entries.push(RenderedEntry {
                        summary: b.summary,
                        items,
                    });
                }
            }
        }
    }

    if entries.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!("Relevant skills (in {}):\n", skill_root.display()));
    for entry in &entries {
        out.push_str(&format!("  {}\n", entry.summary));
        for item in &entry.items {
            match item.label {
                Some(label) => out.push_str(&format!("    {}  ({})\n", item.url, label)),
                None => out.push_str(&format!("    {}\n", item.url)),
            }
        }
    }
    out.push_str(
        "\nThis section is shown only when an automated coding agent is detected. \
         Set GOLEM_CLI_AGENT_HINTS=0 to disable.",
    );
    Some(out)
}

struct RenderedEntry {
    summary: &'static str,
    items: Vec<RenderedItem>,
}

struct RenderedItem {
    label: Option<&'static str>,
    url: String,
}

fn file_url(path: &Path) -> String {
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    // Minimal manual encoding: escape spaces only. Skill paths under
    // .agents/skills/ are normally ASCII-safe.
    let s = abs.to_string_lossy().replace(' ', "%20");
    if cfg!(windows) {
        format!("file:///{}", s.replace('\\', "/"))
    } else {
        format!("file://{}", s)
    }
}

fn navigate_mut<'a>(root: &'a mut Command, path: &[String]) -> Option<&'a mut Command> {
    let mut cur = root;
    for name in path {
        cur = cur.find_subcommand_mut(name.as_str())?;
    }
    Some(cur)
}

fn append_after_long_help(cmd: &mut Command, block: &str) {
    let existing = cmd
        .get_after_long_help()
        .map(|s| s.to_string())
        .or_else(|| cmd.get_after_help().map(|s| s.to_string()));

    let combined = match existing {
        Some(prev) if !prev.trim().is_empty() => format!("{prev}\n\n{block}"),
        _ => block.to_string(),
    };

    let taken = mem::take(cmd);
    *cmd = taken.after_long_help(combined);
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::agent_help_hints::builtin_skill_map::SKILL_BINDINGS;
    use crate::command::GolemCliCommand;
    use clap::CommandFactory;
    use std::fs;
    use test_r::test;

    /// Every binding in the table must reference a real subcommand path in
    /// the live clap tree. Catches drift if a CLI command is renamed.
    #[test]
    fn every_binding_path_resolves_in_clap_tree() {
        let mut cmd = GolemCliCommand::command();
        let mut missing = Vec::<String>::new();
        for b in SKILL_BINDINGS {
            let path: Vec<String> = b.cli_path.iter().map(|s| s.to_string()).collect();
            if navigate_mut(&mut cmd, &path).is_none() {
                missing.push(path.join(" "));
            }
        }
        assert!(
            missing.is_empty(),
            "SKILL_BINDINGS reference unknown cli paths:\n  {}",
            missing.join("\n  ")
        );
    }

    /// End-to-end: stage a fake `.agents/skills/` tree in a temp app dir,
    /// resolve from there, and verify the rendered block contains expected
    /// skill links and labels.
    #[test]
    fn resolves_only_installed_skills() {
        let tmp = tempfile::tempdir().unwrap();
        let app_dir = tmp.path().join("app");
        let skills = app_dir.join(".agents").join("skills");
        fs::create_dir_all(&skills).unwrap();
        // Minimal manifest so find_main_source_from picks app_dir.
        fs::write(app_dir.join("golem.yaml"), "components: {}\n").unwrap();
        // Install: golem-build (common), golem-add-secret-ts (per-lang TS only).
        for name in ["golem-build", "golem-add-secret-ts"] {
            let dir = skills.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        }

        let blocks = collect_resolved_blocks(&skills);

        let build_block = blocks
            .get(&vec!["build".to_string()])
            .expect("build binding should resolve");
        assert!(build_block.contains("golem-build/SKILL.md"));

        let secret_block = blocks
            .get(&vec!["secret".to_string(), "create".to_string()])
            .expect("secret create binding should resolve");
        assert!(secret_block.contains("golem-add-secret-ts/SKILL.md"));
        assert!(secret_block.contains("(TypeScript)"));
        // Non-installed languages must not show up.
        assert!(!secret_block.contains("golem-add-secret-rust"));
        assert!(!secret_block.contains("(Rust)"));

        // Bindings with nothing installed must not render.
        assert!(!blocks.contains_key(&vec!["deploy".to_string()]));
    }
}
