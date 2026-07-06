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

//! Resolves the [`AppVersionSource`] declared in the manifest into a concrete
//! logical version string at deploy time. The `git` source shells out to the
//! `git` binary (reusing [`crate::process::which`]).

use crate::log::log_warn;
use crate::model::app_raw::{
    AppVersionSource, GitHashVersionSource, GitTagVersionSource, GitVersionSource,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::Command;
use tracing::debug;

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedAppVersionSource {
    Git(ResolvedGitVersionSource),
    Static(String),
    Env(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedGitVersionSource {
    pub mode: ResolvedGitMode,
    pub allow_dirty: bool,
    pub static_fallback: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedGitMode {
    Tag {
        tag_pattern: String,
        commit_info: bool,
        hash_fallback: bool,
    },
    Hash,
}

impl ResolvedAppVersionSource {
    pub fn from_source(source: AppVersionSource) -> Self {
        match source {
            AppVersionSource::Git { git } => ResolvedAppVersionSource::Git(Self::resolve_git(git)),
            AppVersionSource::Static(value) => ResolvedAppVersionSource::Static(value),
            AppVersionSource::Env { env } => ResolvedAppVersionSource::Env(env),
        }
    }

    fn resolve_git(git: GitVersionSource) -> ResolvedGitVersionSource {
        match git {
            GitVersionSource::Hash(GitHashVersionSource {
                hash_only: _,
                allow_dirty,
                static_fallback,
            }) => ResolvedGitVersionSource {
                mode: ResolvedGitMode::Hash,
                allow_dirty: allow_dirty.unwrap_or(false),
                static_fallback,
            },
            GitVersionSource::Tag(GitTagVersionSource {
                tag_pattern,
                commit_info,
                hash_fallback,
                allow_dirty,
                static_fallback,
            }) => ResolvedGitVersionSource {
                mode: ResolvedGitMode::Tag {
                    tag_pattern,
                    commit_info: commit_info.unwrap_or(true),
                    hash_fallback: hash_fallback.unwrap_or(false),
                },
                allow_dirty: allow_dirty.unwrap_or(false),
                static_fallback,
            },
        }
    }
}

#[derive(Debug, Error)]
pub enum AppVersionError {
    #[error("the static version is empty")]
    EmptyStaticVersion,
    #[error("environment variable '{0}' is not set")]
    EnvVarMissing(String),
    #[error("environment variable '{0}' is set but empty")]
    EnvVarEmpty(String),
    #[error(
        "the git working tree has uncommitted changes and `allowDirty` is false; commit your changes or set `allowDirty: true`"
    )]
    WorkingTreeDirty,
    #[error(
        "no git tag was found and no `staticFallback` version is configured; tag a release or add a `staticFallback` to the git version source"
    )]
    NoGitTag,
    #[error(
        "'{0}' is not a git repository and no `staticFallback` version is configured; run inside a git repo, add a `staticFallback`, or use a static/env version source"
    )]
    NotAGitRepository(PathBuf),
    #[error(
        "the git repository has no commits yet and no `staticFallback` version is configured; make a commit, add a `staticFallback`, or use a static/env version source"
    )]
    NoCommits,
    #[error(
        "the 'git' executable was not found and no `staticFallback` version is configured; install git, add a `staticFallback`, or use a static/env version source"
    )]
    GitNotInstalled,
    #[error("git command failed: {0}")]
    GitCommandFailed(String),
}

/// Compute the final logical version string. `working_dir` is the application
/// root, used as the git working directory for the `git` source.
pub async fn compute_version(
    source: &ResolvedAppVersionSource,
    working_dir: &Path,
) -> Result<String, AppVersionError> {
    match source {
        ResolvedAppVersionSource::Static(value) => {
            if value.is_empty() {
                Err(AppVersionError::EmptyStaticVersion)
            } else {
                Ok(value.clone())
            }
        }
        ResolvedAppVersionSource::Env(name) => match std::env::var(name) {
            Ok(value) if !value.is_empty() => Ok(value),
            Ok(_) => Err(AppVersionError::EnvVarEmpty(name.clone())),
            Err(_) => Err(AppVersionError::EnvVarMissing(name.clone())),
        },
        ResolvedAppVersionSource::Git(git) => compute_git_version(working_dir, git).await,
    }
}

async fn compute_git_version(
    working_dir: &Path,
    source: &ResolvedGitVersionSource,
) -> Result<String, AppVersionError> {
    // Without git / outside a repo, fall back to the static version (or error).
    let git = match GitClient::new(working_dir) {
        Ok(git) => git,
        Err(_) => {
            return fallback_or(
                &source.static_fallback,
                "git is not installed",
                AppVersionError::GitNotInstalled,
            );
        }
    };
    if !git.is_inside_work_tree().await? {
        return fallback_or(
            &source.static_fallback,
            &format!("{} is not a git repository", working_dir.display()),
            AppVersionError::NotAGitRepository(working_dir.to_path_buf()),
        );
    }
    if !git.has_head().await? {
        return fallback_or(
            &source.static_fallback,
            "the git repository has no commits yet",
            AppVersionError::NoCommits,
        );
    }

    let dirty = git.is_dirty().await?;
    if dirty && !source.allow_dirty {
        return Err(AppVersionError::WorkingTreeDirty);
    }

    let base = match &source.mode {
        ResolvedGitMode::Hash => git.short_hash().await?,
        ResolvedGitMode::Tag {
            tag_pattern,
            commit_info,
            hash_fallback,
        } => match git.describe(tag_pattern, *commit_info).await? {
            Some(tag) => tag,
            None if *hash_fallback => git.short_hash().await?,
            None => match &source.static_fallback {
                Some(value) => {
                    log_warn(format!(
                        "no git tag found; using fallback version '{value}'"
                    ));
                    value.clone()
                }
                None => return Err(AppVersionError::NoGitTag),
            },
        },
    };

    Ok(if dirty { format!("{base}-dirty") } else { base })
}

/// A degraded git environment (no git binary / not a repo) falls back to the
/// configured static version with a warning, or fails with `err` if none.
fn fallback_or(
    fallback: &Option<String>,
    reason: &str,
    err: AppVersionError,
) -> Result<String, AppVersionError> {
    match fallback {
        Some(value) => {
            log_warn(format!("{reason}; using fallback version '{value}'"));
            Ok(value.clone())
        }
        None => Err(err),
    }
}

struct GitClient {
    program: PathBuf,
    working_dir: PathBuf,
}

impl GitClient {
    fn new(working_dir: &Path) -> Result<Self, AppVersionError> {
        let program = crate::process::which("git").map_err(|_| AppVersionError::GitNotInstalled)?;
        Ok(Self {
            program,
            working_dir: working_dir.to_path_buf(),
        })
    }

    async fn run(&self, args: &[&str]) -> Result<std::process::Output, AppVersionError> {
        Command::new(&self.program)
            .args(args)
            .current_dir(&self.working_dir)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|err| AppVersionError::GitCommandFailed(err.to_string()))
    }

    async fn is_inside_work_tree(&self) -> Result<bool, AppVersionError> {
        let output = self.run(&["rev-parse", "--is-inside-work-tree"]).await?;
        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    async fn has_head(&self) -> Result<bool, AppVersionError> {
        let output = self
            .run(&["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?;
        Ok(output.status.success())
    }

    /// Tracked changes only; untracked files are ignored, like `git describe --dirty`.
    async fn is_dirty(&self) -> Result<bool, AppVersionError> {
        let output = self
            .run(&["status", "--porcelain", "--untracked-files=no"])
            .await?;
        if !output.status.success() {
            return Err(AppVersionError::GitCommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    }

    /// `Some(description)` when a matching tag is reachable, `None` when there
    /// is none (so the caller applies its fallback). With `commit_info` the
    /// description includes the `-<distance>-g<hash>` suffix when past the tag.
    async fn describe(
        &self,
        tag_pattern: &str,
        commit_info: bool,
    ) -> Result<Option<String>, AppVersionError> {
        let mut args = vec!["describe", "--tags", "--match", tag_pattern];
        if !commit_info {
            args.push("--abbrev=0");
        }
        let output = self.run(&args).await?;
        if output.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ))
        } else {
            // HEAD is guaranteed by the caller, so a non-zero exit means no matching tag.
            debug!(
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "git describe found no matching tag"
            );
            Ok(None)
        }
    }

    async fn short_hash(&self) -> Result<String, AppVersionError> {
        let output = self.run(&["rev-parse", "--short", "HEAD"]).await?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(AppVersionError::GitCommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::app_raw::{
        AppVersionSource, AppVersionSourceOverride, GitHashVersionSource, GitTagVersionSource,
        GitTagVersionSourceOverride, GitVersionSource, GitVersionSourceOverride, Marker,
    };
    use std::path::Path;
    use std::process::Command as StdCommand;
    use test_r::test;

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = StdCommand::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        git(dir, &["init", "-q"]);
        git(dir, &["config", "user.email", "test@example.com"]);
        git(dir, &["config", "user.name", "Test"]);
    }

    fn commit_file(dir: &Path, name: &str, contents: &str) {
        std::fs::write(dir.join(name), contents).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "commit"]);
    }

    fn resolved_tag(
        tag_pattern: &str,
        commit_info: bool,
        hash_fallback: bool,
        allow_dirty: bool,
        static_fallback: Option<&str>,
    ) -> ResolvedAppVersionSource {
        ResolvedAppVersionSource::Git(ResolvedGitVersionSource {
            mode: ResolvedGitMode::Tag {
                tag_pattern: tag_pattern.to_string(),
                commit_info,
                hash_fallback,
            },
            allow_dirty,
            static_fallback: static_fallback.map(str::to_string),
        })
    }

    fn resolved_hash(allow_dirty: bool, static_fallback: Option<&str>) -> ResolvedAppVersionSource {
        ResolvedAppVersionSource::Git(ResolvedGitVersionSource {
            mode: ResolvedGitMode::Hash,
            allow_dirty,
            static_fallback: static_fallback.map(str::to_string),
        })
    }

    /// Tag mode with defaults (all tags, commitInfo on, no hashFallback).
    fn git_source(allow_dirty: bool, static_fallback: Option<&str>) -> ResolvedAppVersionSource {
        resolved_tag("*", true, false, allow_dirty, static_fallback)
    }

    #[test]
    fn static_non_empty_is_used_verbatim() {
        let result = block_on(compute_version(
            &ResolvedAppVersionSource::Static("1.2.3".to_string()),
            Path::new("."),
        ))
        .unwrap();
        assert_eq!(result, "1.2.3");
    }

    #[test]
    fn static_empty_is_an_error() {
        let result = block_on(compute_version(
            &ResolvedAppVersionSource::Static(String::new()),
            Path::new("."),
        ));
        assert!(matches!(result, Err(AppVersionError::EmptyStaticVersion)));
    }

    #[test]
    fn env_missing_is_an_error() {
        let result = block_on(compute_version(
            &ResolvedAppVersionSource::Env("GOLEM_TEST_VERSION_STRATEGY_MISSING_VAR".to_string()),
            Path::new("."),
        ));
        assert!(matches!(result, Err(AppVersionError::EnvVarMissing(_))));
    }

    fn root_tag(tag_pattern: &str, static_fallback: Option<&str>) -> AppVersionSource {
        AppVersionSource::Git {
            git: GitVersionSource::Tag(GitTagVersionSource {
                tag_pattern: tag_pattern.to_string(),
                commit_info: None,
                hash_fallback: None,
                allow_dirty: None,
                static_fallback: static_fallback.map(str::to_string),
            }),
        }
    }

    #[test]
    fn from_source_defaults_options() {
        let resolved = ResolvedAppVersionSource::from_source(root_tag("v*", Some("0.0.0")));
        assert_eq!(
            resolved,
            resolved_tag("v*", true, false, false, Some("0.0.0"))
        );
    }

    #[test]
    fn override_tag_merges_over_root_tag() {
        // An env override that only sets `allowDirty` inherits the rest from the root.
        let over = AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Tag(GitTagVersionSourceOverride {
                allow_dirty: Some(true),
                ..Default::default()
            }),
        };
        let root = AppVersionSource::Git {
            git: GitVersionSource::Tag(GitTagVersionSource {
                tag_pattern: "v*".to_string(),
                commit_info: Some(false),
                hash_fallback: None,
                allow_dirty: Some(false),
                static_fallback: Some("0.0.0".to_string()),
            }),
        };
        assert_eq!(
            ResolvedAppVersionSource::from_source(over.resolve_over(Some(root)).unwrap()),
            resolved_tag("v*", false, false, true, Some("0.0.0"))
        );
    }

    #[test]
    fn override_switches_mode_over_root() {
        let over = AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Hash(GitHashVersionSource {
                hash_only: Marker,
                allow_dirty: None,
                static_fallback: None,
            }),
        };
        assert_eq!(
            ResolvedAppVersionSource::from_source(
                over.resolve_over(Some(root_tag("v*", Some("0.0.0"))))
                    .unwrap()
            ),
            resolved_hash(false, None)
        );
    }

    #[test]
    fn override_tag_without_pattern_and_no_root_errors() {
        let over = AppVersionSourceOverride::Git {
            git: GitVersionSourceOverride::Tag(GitTagVersionSourceOverride {
                allow_dirty: Some(true),
                ..Default::default()
            }),
        };
        assert!(over.resolve_over(None).is_err());
    }

    #[test]
    fn git_clean_tagged_uses_the_tag() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);

        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "v1.0.0");
    }

    #[test]
    fn git_no_tag_uses_fallback() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");

        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "0.0.0");
    }

    #[test]
    fn git_no_tag_without_fallback_errors() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");

        let result = block_on(compute_version(&git_source(false, None), dir.path()));
        assert!(matches!(result, Err(AppVersionError::NoGitTag)));
    }

    #[test]
    fn git_dirty_deny_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);
        std::fs::write(dir.path().join("a.txt"), "changed").unwrap();

        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ));
        assert!(matches!(result, Err(AppVersionError::WorkingTreeDirty)));
    }

    #[test]
    fn git_dirty_allow_appends_marker() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);
        std::fs::write(dir.path().join("a.txt"), "changed").unwrap();

        let result = block_on(compute_version(
            &git_source(true, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "v1.0.0-dirty");
    }

    #[test]
    fn git_untracked_file_is_not_dirty() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);
        std::fs::write(dir.path().join("untracked.txt"), "scratch").unwrap();

        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "v1.0.0");
    }

    #[test]
    fn git_no_commits_without_fallback_errors() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let result = block_on(compute_version(&git_source(false, None), dir.path()));
        assert!(matches!(result, Err(AppVersionError::NoCommits)));
    }

    #[test]
    fn git_no_commits_with_fallback_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "0.0.0");
    }

    #[test]
    fn git_hash_no_commits_errors() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());

        let result = block_on(compute_version(&resolved_hash(false, None), dir.path()));
        assert!(matches!(result, Err(AppVersionError::NoCommits)));
    }

    #[test]
    fn not_a_git_repo_with_fallback_uses_it() {
        let dir = tempfile::tempdir().unwrap();
        let result = block_on(compute_version(
            &git_source(false, Some("0.0.0-dev")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "0.0.0-dev");
    }

    #[test]
    fn not_a_git_repo_without_fallback_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = block_on(compute_version(&git_source(false, None), dir.path()));
        assert!(matches!(result, Err(AppVersionError::NotAGitRepository(_))));
    }

    #[test]
    fn git_hash_only_ignores_tags() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);

        let result = block_on(compute_version(&resolved_hash(false, None), dir.path())).unwrap();
        assert_ne!(result, "v1.0.0");
        assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn git_commit_info_toggles_the_suffix() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "v1.0.0"]);
        commit_file(dir.path(), "b.txt", "world"); // one commit past the tag

        let with_info = block_on(compute_version(
            &git_source(false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert!(with_info.starts_with("v1.0.0-1-g"), "got {with_info}");

        let bare = block_on(compute_version(
            &resolved_tag("*", false, false, false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(bare, "v1.0.0");
    }

    #[test]
    fn git_no_tag_hash_fallback_uses_hash() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");

        let result = block_on(compute_version(
            &resolved_tag("*", true, true, false, None),
            dir.path(),
        ))
        .unwrap();
        assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn git_tag_pattern_filters_tags() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        commit_file(dir.path(), "a.txt", "hello");
        git(dir.path(), &["tag", "other-1"]);

        let result = block_on(compute_version(
            &resolved_tag("release-*", true, false, false, Some("0.0.0")),
            dir.path(),
        ))
        .unwrap();
        assert_eq!(result, "0.0.0");
    }
}
