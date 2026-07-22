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

//! Static mapping of CLI command paths to relevant skills shipped under
//! `golem-skills/skills/{common,ts,rust,scala,moonbit}`.
//!
//! Each `SkillBinding` says: "if the user runs `--help` on this CLI command
//! and the named skill is installed under `<app_dir>/.agents/skills/`, link
//! to it from the command's long help".
//!
//! `SkillKind::Common` means the skill is language-independent and lives at
//! `.agents/skills/<basename>/SKILL.md`.
//!
//! `SkillKind::PerLanguage(langs)` means there is one variant per listed
//! language, installed as `.agents/skills/<basename>-<lang>/SKILL.md`. We use
//! the short language tag exactly as `GuestLanguage::to_string()` produces
//! it (`rust`, `ts`, `scala`, `moonbit`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Ts,
    Scala,
    MoonBit,
}

impl Lang {
    /// Skill folder suffix used in `.agents/skills/<basename>-<suffix>/`.
    pub fn suffix(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Ts => "ts",
            Lang::Scala => "scala",
            Lang::MoonBit => "moonbit",
        }
    }

    /// Human-readable name used in the help output.
    pub fn display(self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::Ts => "TypeScript",
            Lang::Scala => "Scala",
            Lang::MoonBit => "MoonBit",
        }
    }
}

/// All currently supported guest languages.
pub const ALL_LANGS: &[Lang] = &[Lang::Ts, Lang::Rust, Lang::Scala, Lang::MoonBit];

#[derive(Debug, Clone, Copy)]
pub enum SkillKind {
    /// Single skill with no language suffix.
    Common,
    /// One skill per listed language; folder name is `<basename>-<lang>`.
    PerLanguage(&'static [Lang]),
}

#[derive(Debug, Clone, Copy)]
pub struct SkillBinding {
    /// Path of the CLI command, e.g. `&["secret", "create"]`.
    pub cli_path: &'static [&'static str],
    /// Skill folder basename (without language suffix).
    pub basename: &'static str,
    /// How variants are organized on disk.
    pub kind: SkillKind,
    /// One-line summary used in the help output. Should be language-agnostic.
    pub summary: &'static str,
}

/// The full table. Order matters only for stability of help output: bindings
/// with the same `cli_path` are emitted in source order under one header.
#[rustfmt::skip]
pub const SKILL_BINDINGS: &[SkillBinding] = &[
    // ── application lifecycle ────────────────────────────────────────────
    SkillBinding { cli_path: &["new"],     basename: "golem-new-project",        kind: SkillKind::Common,                  summary: "Scaffold a new Golem application." },
    SkillBinding { cli_path: &["build"],   basename: "golem-build",              kind: SkillKind::Common,                  summary: "Build a Golem application." },
    SkillBinding { cli_path: &["build"],   basename: "golem-troubleshoot-build", kind: SkillKind::Common,                  summary: "Diagnose Golem build failures." },
    SkillBinding { cli_path: &["deploy"],  basename: "golem-deploy",             kind: SkillKind::Common,                  summary: "Deploy a Golem application." },
    SkillBinding { cli_path: &["deploy"],  basename: "golem-deployment-version", kind: SkillKind::Common,                  summary: "Configure the version attached to a deployment (git/static/env) and versionCheck." },
    SkillBinding { cli_path: &["deploy"],  basename: "golem-redeploy-agents",    kind: SkillKind::Common,                  summary: "Recreate existing agents during deploy." },
    SkillBinding { cli_path: &["deploy"],  basename: "golem-rollback",           kind: SkillKind::Common,                  summary: "Roll back a deployment to a previous revision/version." },

    // ── local server (only present when built with `server-commands`) ───
    #[cfg(feature = "server-commands")]
    SkillBinding { cli_path: &["server", "run"], basename: "golem-local-dev-server", kind: SkillKind::Common, summary: "Use the local Golem dev server." },

    // ── agents (top-level grouping) ──────────────────────────────────────
    SkillBinding { cli_path: &["agent", "new"],         basename: "golem-create-agent-instance", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Create a new agent instance." },
    SkillBinding { cli_path: &["agent", "invoke"],      basename: "golem-invoke-agent",          kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Invoke a method on an agent." },
    SkillBinding { cli_path: &["agent", "invoke"],      basename: "golem-trigger-agent",         kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Trigger a fire-and-forget invocation." },
    SkillBinding { cli_path: &["agent", "invoke"],      basename: "golem-schedule-agent",        kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Schedule a future invocation." },
    SkillBinding { cli_path: &["agent", "list"],        basename: "golem-list-and-filter-agents", kind: SkillKind::Common, summary: "List and filter agents." },
    SkillBinding { cli_path: &["agent", "get"],         basename: "golem-get-agent-metadata",    kind: SkillKind::Common, summary: "Inspect agent metadata and status." },
    SkillBinding { cli_path: &["agent", "delete"],      basename: "golem-delete-agent",          kind: SkillKind::Common, summary: "Delete an agent instance." },
    SkillBinding { cli_path: &["agent", "revert"],      basename: "golem-rollback",              kind: SkillKind::Common, summary: "Roll back agent state." },
    SkillBinding { cli_path: &["agent", "revert"],      basename: "golem-undo-agent-state",      kind: SkillKind::Common, summary: "Revert agent state by undoing operations." },
    SkillBinding { cli_path: &["agent", "oplog"],       basename: "golem-debug-agent-history",   kind: SkillKind::Common, summary: "Query the agent operation log." },
    SkillBinding { cli_path: &["agent", "files"],       basename: "golem-view-agent-files",      kind: SkillKind::Common, summary: "List files in an agent's virtual filesystem." },
    SkillBinding { cli_path: &["agent", "file-contents"], basename: "golem-view-agent-files",    kind: SkillKind::Common, summary: "Read files from an agent's virtual filesystem." },
    SkillBinding { cli_path: &["agent", "cancel-invocation"], basename: "golem-cancel-queued-invocation", kind: SkillKind::Common, summary: "Cancel a queued invocation." },
    SkillBinding { cli_path: &["agent", "interrupt"],   basename: "golem-interrupt-resume-agent", kind: SkillKind::Common, summary: "Interrupt or resume an agent." },
    SkillBinding { cli_path: &["agent", "resume"],      basename: "golem-interrupt-resume-agent", kind: SkillKind::Common, summary: "Interrupt or resume an agent." },
    SkillBinding { cli_path: &["agent", "update"],      basename: "golem-update-running-agents",  kind: SkillKind::Common, summary: "Update components used by running agents." },
    SkillBinding { cli_path: &["agent", "stream"],      basename: "golem-view-agent-logs",        kind: SkillKind::Common, summary: "Stream live agent logs." },
    SkillBinding { cli_path: &["agent", "simulate-crash"], basename: "golem-test-crash-recovery", kind: SkillKind::Common, summary: "Test crash recovery by simulating an agent crash." },
    SkillBinding { cli_path: &["agent", "activate-plugin"],   basename: "golem-manage-plugins",   kind: SkillKind::Common, summary: "Manage Golem plugins on agents." },
    SkillBinding { cli_path: &["agent", "deactivate-plugin"], basename: "golem-manage-plugins",   kind: SkillKind::Common, summary: "Manage Golem plugins on agents." },

    // ── secrets ──────────────────────────────────────────────────────────
    SkillBinding { cli_path: &["secret", "create"],       basename: "golem-add-secret", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Add a typed secret available to your agents." },
    SkillBinding { cli_path: &["secret", "update-value"], basename: "golem-add-secret", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Add or change a secret available to your agents." },

    // ── resource quotas ──────────────────────────────────────────────────
    SkillBinding { cli_path: &["resource", "create"], basename: "golem-quota", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Add resource quotas (rate limiting, capacity, concurrency)." },
    SkillBinding { cli_path: &["resource", "update"], basename: "golem-quota", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Change resource quotas (rate limiting, capacity, concurrency)." },

    // ── retry policies ───────────────────────────────────────────────────
    SkillBinding { cli_path: &["retry-policy", "create"], basename: "golem-retry-policies", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Configure semantic retry policies." },
    SkillBinding { cli_path: &["retry-policy", "update"], basename: "golem-retry-policies", kind: SkillKind::PerLanguage(ALL_LANGS), summary: "Configure semantic retry policies." },

    // ── api / domain ─────────────────────────────────────────────────────
    SkillBinding { cli_path: &["api", "domain", "register"], basename: "golem-configure-api-domain", kind: SkillKind::Common, summary: "Configure an HTTP API domain." },

    // ── plugins ──────────────────────────────────────────────────────────
    SkillBinding { cli_path: &["plugin", "register"],   basename: "golem-manage-plugins", kind: SkillKind::Common, summary: "Manage Golem plugins (register/list/configure)." },
    SkillBinding { cli_path: &["plugin", "list"],       basename: "golem-manage-plugins", kind: SkillKind::Common, summary: "Manage Golem plugins (register/list/configure)." },
    SkillBinding { cli_path: &["plugin", "get"],        basename: "golem-manage-plugins", kind: SkillKind::Common, summary: "Manage Golem plugins (register/list/configure)." },
    SkillBinding { cli_path: &["plugin", "unregister"], basename: "golem-manage-plugins", kind: SkillKind::Common, summary: "Manage Golem plugins (register/list/configure)." },

    // ── account / api-token ──────────────────────────────────────────────
    SkillBinding { cli_path: &["account"],    basename: "golem-cloud-account-setup", kind: SkillKind::Common, summary: "Set up a Golem Cloud account." },
    SkillBinding { cli_path: &["api-token"],  basename: "golem-cloud-account-setup", kind: SkillKind::Common, summary: "Set up a Golem Cloud account." },

    // ── profiles & environments ──────────────────────────────────────────
    SkillBinding { cli_path: &["profile"],     basename: "golem-profiles-and-environments", kind: SkillKind::Common, summary: "CLI profiles vs application environments." },
    SkillBinding { cli_path: &["environment"], basename: "golem-profiles-and-environments", kind: SkillKind::Common, summary: "CLI profiles vs application environments." },

    // ── repl ─────────────────────────────────────────────────────────────
    SkillBinding { cli_path: &["repl"], basename: "golem-interactive-repl", kind: SkillKind::PerLanguage(&[Lang::Ts]), summary: "Use the Golem REPL for interactive testing." },
];

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;
    use test_r::test;

    /// Every basename listed in the table must exist somewhere under
    /// `golem-skills/skills/` so that renames in the skills repo cause a CLI
    /// build failure rather than silently dropping a link.
    #[test]
    fn every_binding_basename_exists_in_golem_skills_repo() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let skills_root = repo_root.join("golem-skills").join("skills");
        assert!(
            skills_root.is_dir(),
            "expected to find golem-skills/skills/ at {}",
            skills_root.display()
        );

        let mut missing = Vec::<String>::new();
        for binding in SKILL_BINDINGS {
            match binding.kind {
                SkillKind::Common => {
                    if !skill_dir_exists_anywhere(&skills_root, binding.basename) {
                        missing.push(format!(
                            "common skill `{}` (binding for `{}`)",
                            binding.basename,
                            binding.cli_path.join(" ")
                        ));
                    }
                }
                SkillKind::PerLanguage(langs) => {
                    let mut any_found = false;
                    for lang in langs {
                        let folder = format!("{}-{}", binding.basename, lang.suffix());
                        if skill_dir_exists_anywhere(&skills_root, &folder) {
                            any_found = true;
                        }
                    }
                    if !any_found {
                        missing.push(format!(
                            "per-language skill `{}-<lang>` for langs {:?} (binding for `{}`)",
                            binding.basename,
                            langs,
                            binding.cli_path.join(" ")
                        ));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "skills referenced by SKILL_BINDINGS not found in golem-skills repo:\n  {}",
            missing.join("\n  ")
        );
    }

    fn skill_dir_exists_anywhere(skills_root: &std::path::Path, name: &str) -> bool {
        // golem-skills/skills/{common,ts,rust,scala,moonbit}/<name>/SKILL.md
        for sub in &["common", "ts", "rust", "scala", "moonbit"] {
            let candidate = skills_root.join(sub).join(name).join("SKILL.md");
            if candidate.is_file() {
                return true;
            }
        }
        false
    }
}
