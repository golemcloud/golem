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

use crate::app::build::check::DependencyFixStep;
use crate::app::context::BuildContext;
use crate::app::template::AppTemplateRepo;
use crate::fs;
use crate::log::log_warn;
use crate::model::GuestLanguage;
use anyhow::Context;
use anyhow::bail;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(super) fn plan_skill_fix_steps(
    ctx: &BuildContext<'_>,
    selected_languages: &BTreeSet<GuestLanguage>,
    claude_skills_ctx: &ClaudeSkillsContext,
) -> anyhow::Result<Vec<DependencyFixStep>> {
    if selected_languages.is_empty() {
        return Ok(Vec::new());
    }

    let app_template_repo = AppTemplateRepo::get(ctx.application_config().dev_mode)?;
    let app_root = ctx.application().app_root_dir();

    let mut expected_files: BTreeMap<PathBuf, (GuestLanguage, String)> = BTreeMap::new();
    for &language in selected_languages {
        for (rel_path, content) in app_template_repo.common_template_skill_files(language)? {
            match expected_files.entry(rel_path.clone()) {
                Entry::Vacant(v) => {
                    v.insert((language, content));
                }
                Entry::Occupied(o) => {
                    let (prev_lang, prev_content) = o.get();
                    if *prev_content != content {
                        bail!(
                            "Conflicting embedded skill {} for {} and {}",
                            rel_path.display(),
                            prev_lang.name(),
                            language.name()
                        );
                    }
                }
            }
        }
    }

    let mut steps = Vec::new();
    collect_skill_fix_steps(app_root, &expected_files, &mut steps)?;

    match claude_skills_ctx.mode {
        ClaudeSkillsSyncMode::ClaudeSymlink => {
            // NOP: in symlink mode, `.claude` resolves to `.agents`, so syncing app root is enough.
        }
        ClaudeSkillsSyncMode::SyncAll => {
            let claude_root = app_root.join(".claude");
            match claude_skills_ctx.path_state {
                ClaudePathState::File => {
                    warn_claude_path_is_file(app_root, claude_skills_ctx);
                    // NOP: `.claude` is a regular file. We skip Claude syncing for this run.
                }
                ClaudePathState::Missing | ClaudePathState::Symlink | ClaudePathState::Directory => {
                    collect_skill_fix_steps(&claude_root, &expected_files, &mut steps)?;
                }
            }
        }
    }

    Ok(steps)
}

fn collect_skill_fix_steps(
    root: &Path,
    expected_files: &BTreeMap<PathBuf, (GuestLanguage, String)>,
    steps: &mut Vec<DependencyFixStep>,
) -> anyhow::Result<()> {
    for (rel_path, (_language, expected)) in expected_files {
        let disk_path = root.join(rel_path);
        let current = if disk_path.exists() {
            fs::read_to_string(&disk_path)?
        } else {
            String::new()
        };

        if current != *expected {
            steps.push(DependencyFixStep {
                path: disk_path,
                current,
                new: expected.clone(),
            });
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeSkillsSyncMode {
    ClaudeSymlink,
    SyncAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudePathState {
    Missing,
    Symlink,
    Directory,
    File,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeSkillsContext {
    pub mode: ClaudeSkillsSyncMode,
    pub path_state: ClaudePathState,
}

pub(crate) fn resolve_claude_skills_context(
    application_path: &Path,
) -> anyhow::Result<ClaudeSkillsContext> {
    let claude_path = application_path.join(".claude");
    let path_state = resolve_claude_path_state(&claude_path);

    let mode = if cfg!(windows) {
        ClaudeSkillsSyncMode::SyncAll
    } else {
        match path_state {
            ClaudePathState::Directory => ClaudeSkillsSyncMode::SyncAll,
            ClaudePathState::Missing | ClaudePathState::Symlink | ClaudePathState::File => {
                ClaudeSkillsSyncMode::ClaudeSymlink
            }
        }
    };

    Ok(ClaudeSkillsContext { mode, path_state })
}

fn resolve_claude_path_state(claude_path: &Path) -> ClaudePathState {
    match std::fs::symlink_metadata(claude_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                ClaudePathState::Symlink
            } else if metadata.is_dir() {
                ClaudePathState::Directory
            } else if metadata.is_file() {
                ClaudePathState::File
            } else {
                ClaudePathState::Missing
            }
        }
        Err(_) => ClaudePathState::Missing,
    }
}

fn ensure_claude_symlink_or_warn(
    application_path: &Path,
    claude_skills_ctx: &ClaudeSkillsContext,
) -> anyhow::Result<()> {
    let agents_dir = application_path.join(".agents");
    let claude_link = application_path.join(".claude");

    if !agents_dir.exists() {
        return Ok(());
    }

    match claude_skills_ctx.path_state {
        ClaudePathState::File => {
            warn_claude_path_is_file(application_path, claude_skills_ctx);
            return Ok(());
        }
        ClaudePathState::Missing => {
            // Continue to symlink creation.
        }
        ClaudePathState::Symlink | ClaudePathState::Directory => {
            // NOP: `.claude` already exists in a non-file state.
            return Ok(());
        }
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".agents", &claude_link)
            .with_context(|| format!("Failed to create Claude symlink at {}", claude_link.display()))?;
    }

    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(".agents", &claude_link)
            .with_context(|| format!("Failed to create Claude symlink at {}", claude_link.display()))?;
    }

    Ok(())
}

pub(crate) fn warn_claude_path_is_file(
    application_path: &Path,
    claude_skills_ctx: &ClaudeSkillsContext,
) {
    match claude_skills_ctx.path_state {
        ClaudePathState::File => {
            let claude_path = application_path.join(".claude");
            log_warn(format!(
                "Skipping Claude skills sync: {} is a file. Remove it and rerun build so Golem can restore the expected Claude skills layout.",
                claude_path.display()
            ));
        }
        ClaudePathState::Missing | ClaudePathState::Symlink | ClaudePathState::Directory => {
            // NOP: warning is only needed for regular files.
        }
    }
}

pub(crate) fn create_claude_symlink_if_needed(
    application_path: &Path,
    claude_skills_ctx: &ClaudeSkillsContext,
) -> anyhow::Result<()> {
    match claude_skills_ctx.mode {
        ClaudeSkillsSyncMode::ClaudeSymlink => {
            ensure_claude_symlink_or_warn(application_path, claude_skills_ctx)?;
        }
        ClaudeSkillsSyncMode::SyncAll => {
            // NOP: in sync-all mode, build dependency checks sync `.claude` independently.
        }
    }

    Ok(())
}
