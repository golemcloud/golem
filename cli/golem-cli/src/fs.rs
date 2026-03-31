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

use crate::log::{LogColorize, log_warn_action};
use anyhow::{Context, Error, anyhow, bail};
use serde_json::Value as JsonValue;
use std::cmp::PartialEq;
use std::collections::BTreeSet;
use std::fs::{Metadata, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;
use wax::{Glob, LinkBehavior, WalkBehavior};

pub fn parent_or_err(path: &Path) -> anyhow::Result<&Path> {
    path.parent()
        .ok_or_else(|| anyhow::anyhow!("Path {} has no parent", path.display()))
}

pub fn strip_prefix_or_err(path: &Path, prefix: impl AsRef<Path>) -> anyhow::Result<&Path> {
    let prefix = prefix.as_ref();
    path.strip_prefix(prefix).map_err(|_| {
        anyhow!(
            "Path {} does not start with prefix {}",
            path.display(),
            prefix.display()
        )
    })
}

pub fn path_to_str(path: &Path) -> anyhow::Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("Path {} cannot be converted to string", path.display()))
}

pub fn path_to_unix_str(path: &Path) -> anyhow::Result<String> {
    Ok(normalize_str_path_as_unix(path_to_str(path)?))
}

pub fn file_name_to_str(path: &Path) -> anyhow::Result<&str> {
    path.file_name()
        .ok_or_else(|| anyhow!("Path {} has no filename", path.display()))?
        .to_str()
        .ok_or_else(|| anyhow!("Filename {} cannot be converted to string", path.display()))
}

pub fn canonicalize_path(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .map_err(|err| anyhow!("Failed to canonicalize path ({}): {}", path.display(), err))
}

pub fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None => {
                    if !normalized.has_root() {
                        normalized.push("..");
                    }
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) | Some(Component::CurDir) => {
                }
            },
            Component::Normal(name) => normalized.push(name),
        }
    }

    normalized
}

pub fn create_dir_all<P: AsRef<Path>>(path: P) -> anyhow::Result<bool> {
    let path = path.as_ref();
    if path.exists() {
        Ok(false)
    } else {
        std::fs::create_dir_all(path).with_context(|| {
            anyhow!("Failed to create directory {}", path.log_color_highlight())
        })?;
        Ok(true)
    }
}

// Differences compared to std::fs::copy
//  - ensures that the target dir exists
//  - updated the modtime after copy, which is not guaranteed to happen, making it not usable for
//    modtime based up-to-date checks (see https://github.com/rust-lang/rust/issues/115982 for more info)
//  - uses anyhow error with added context
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<u64> {
    let from = from.as_ref();
    let to = to.as_ref();

    let context = || format!("Failed to copy from {} to {}", from.display(), to.display());

    create_dir_all(parent_or_err(to)?)
        .context("Failed to create target dir")
        .with_context(context)?;

    let bytes = std::fs::copy(from, to).with_context(context)?;

    OpenOptions::new()
        .write(true)
        .open(to)
        .and_then(|file| file.set_modified(SystemTime::now()))
        .context("Failed to update target modification time")
        .with_context(context)?;

    Ok(bytes)
}

// See copy above, but also loads and transforms the source contest as String using the provided function
pub fn copy_transformed<P: AsRef<Path>, Q: AsRef<Path>, T: Fn(String) -> anyhow::Result<String>>(
    from: P,
    to: Q,
    transform: T,
) -> anyhow::Result<u64> {
    let from = from.as_ref();
    let to = to.as_ref();

    let context = || {
        format!(
            "Failed to copy (and transform) from {} to {}",
            from.display(),
            to.display()
        )
    };

    create_dir_all(parent_or_err(from)?)
        .context("Failed to create target dir")
        .with_context(context)?;

    let content = read_to_string(from).with_context(context)?;

    let transformed_content = transform(content)
        .context("Failed to transform source content")
        .with_context(context)?;

    let bytes_count = transformed_content.len();

    write(to, transformed_content.as_bytes())
        .context("Failed to write transformed content")
        .with_context(context)?;

    Ok(bytes_count as u64)
}

pub fn read_to_string<P: AsRef<Path>>(path: P) -> anyhow::Result<String> {
    let path = path.as_ref();
    fs_extra::file::read_to_string(path).with_context(|| {
        anyhow!(
            "Failed to read to string, file: {}",
            path.log_color_highlight()
        )
    })
}

pub fn read<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<u8>> {
    let path = path.as_ref();
    std::fs::read(path)
        .with_context(|| anyhow!("Failed to read file: {}", path.log_color_highlight()))
}

// Creates all missing parent directories if necessary and writes str to path.
pub fn write_str<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<()> {
    let path = path.as_ref();
    let str = str.as_ref();

    let context = || anyhow!("Failed to write string to {}", path.log_color_highlight());

    let target_parent = path.parent().with_context(context)?;
    create_dir_all(target_parent).with_context(context)?;
    std::fs::write(path, str.as_bytes()).with_context(context)
}

pub fn append_str<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<()> {
    let path = path.as_ref();
    let str = str.as_ref();

    let context = || anyhow!("Failed to write string to {}", path.log_color_highlight());
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(context)?;
    file.write(str.as_bytes()).with_context(context).map(|_| ())
}

pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> anyhow::Result<()> {
    let path = path.as_ref();

    let context = || anyhow!("Failed to write to {}", path.log_color_highlight());

    let target_parent = path.parent().with_context(context)?;
    create_dir_all(target_parent).with_context(context)?;
    std::fs::write(path, contents).with_context(context)
}

pub fn rename<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<()> {
    let from = from.as_ref();
    let to = to.as_ref();

    let context = || {
        anyhow!(
            "Failed to rename {} to {}",
            from.log_color_highlight(),
            to.log_color_highlight()
        )
    };

    create_dir_all(parent_or_err(to)?).with_context(context)?;
    std::fs::rename(from, to).with_context(context)
}

pub fn remove<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    let path = path.as_ref();
    if path.exists() {
        if path.is_dir() {
            std::fs::remove_dir_all(path).with_context(|| {
                anyhow!("Failed to delete directory {}", path.log_color_highlight())
            })?;
        } else {
            std::fs::remove_file(path)
                .with_context(|| anyhow!("Failed to delete file {}", path.log_color_highlight()))?;
        }
    }
    Ok(())
}

pub fn has_str_content<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<bool> {
    let path = path.as_ref();
    let str = str.as_ref();

    let context = || {
        anyhow!(
            "Failed to compare content to string for {}",
            path.log_color_highlight()
        )
    };

    let content = read_to_string(path)
        .with_context(|| anyhow!("Failed to read as string: {}", path.log_color_highlight()))
        .with_context(context)?;

    Ok(content == str)
}

pub fn has_same_string_content<P: AsRef<Path>, Q: AsRef<Path>>(a: P, b: Q) -> anyhow::Result<bool> {
    let a = a.as_ref();
    let b = b.as_ref();

    let context = || {
        anyhow!(
            "Failed to compare string contents of {} and {}",
            a.log_color_highlight(),
            b.log_color_highlight()
        )
    };

    let content_a = read_to_string(a).with_context(context)?;
    let content_b = read_to_string(b).with_context(context)?;

    Ok(content_a == content_b)
}

pub fn metadata<P: AsRef<Path>>(path: P) -> anyhow::Result<Metadata> {
    let path = path.as_ref();
    std::fs::metadata(path)
        .with_context(|| anyhow!("Failed to get metadata for {}", path.log_color_highlight()))
}

// TODO: we most probably do not need this anymore
pub enum OverwriteSafeAction {
    CopyFile {
        source: PathBuf,
        target: PathBuf,
    },
    CopyFileTransformed {
        source: PathBuf,
        source_content_transformed: String,
        target: PathBuf,
    },
    WriteFile {
        content: String,
        target: PathBuf,
    },
}

impl OverwriteSafeAction {
    pub fn copy_file_transformed<F>(
        source: PathBuf,
        target: PathBuf,
        transform: F,
    ) -> anyhow::Result<Self>
    where
        F: FnOnce(String) -> anyhow::Result<String>,
    {
        let content = std::fs::read_to_string(&source).with_context(|| {
            anyhow!(
                "Failed to read file as string, path: {}",
                source.log_color_highlight()
            )
        })?;

        let source_transformed = transform(content).with_context(|| {
            anyhow!(
                "Failed to transform file, path: {}",
                source.log_color_highlight()
            )
        })?;

        Ok(OverwriteSafeAction::CopyFileTransformed {
            source,
            source_content_transformed: source_transformed,
            target,
        })
    }

    pub fn target(&self) -> &Path {
        match self {
            OverwriteSafeAction::CopyFile { target, .. } => target,
            OverwriteSafeAction::CopyFileTransformed { target, .. } => target,
            OverwriteSafeAction::WriteFile { target, .. } => target,
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
pub enum OverwriteSafeActionPlan {
    Create,
    Overwrite,
    SkipSameContent,
}

pub struct OverwriteSafeActions(Vec<OverwriteSafeAction>);

impl Default for OverwriteSafeActions {
    fn default() -> Self {
        Self::new()
    }
}

impl OverwriteSafeActions {
    pub fn new() -> Self {
        OverwriteSafeActions(Vec::new())
    }

    pub fn add(&mut self, action: OverwriteSafeAction) -> &mut Self {
        self.0.push(action);
        self
    }

    pub fn targets(&self) -> Vec<&Path> {
        self.0.iter().map(|a| a.target()).collect()
    }

    pub fn run<F>(
        self,
        allow_overwrite: bool,
        allow_skip_by_content: bool,
        log_action: F,
    ) -> anyhow::Result<Vec<OverwriteSafeAction>>
    where
        F: Fn(&OverwriteSafeAction, OverwriteSafeActionPlan),
    {
        let actions_with_plan = {
            let mut actions_with_plan =
                Vec::<(OverwriteSafeAction, OverwriteSafeActionPlan)>::new();
            let mut forbidden_overwrites = Vec::<OverwriteSafeAction>::new();

            for action in self.0 {
                let plan = match &action {
                    OverwriteSafeAction::CopyFile { source, target } => Self::plan_for_action(
                        allow_overwrite,
                        allow_skip_by_content,
                        target,
                        || has_same_string_content(source, target),
                    )?,
                    OverwriteSafeAction::CopyFileTransformed {
                        source_content_transformed: source_transformed,
                        target,
                        ..
                    } => Self::plan_for_action(
                        allow_overwrite,
                        allow_skip_by_content,
                        target,
                        || has_str_content(target, source_transformed),
                    )?,
                    OverwriteSafeAction::WriteFile { content, target } => Self::plan_for_action(
                        allow_overwrite,
                        allow_skip_by_content,
                        target,
                        || has_str_content(target, content),
                    )?,
                };
                match plan {
                    Some(plan) => actions_with_plan.push((action, plan)),
                    None => forbidden_overwrites.push(action),
                }
            }

            if !forbidden_overwrites.is_empty() {
                return Ok(forbidden_overwrites);
            }

            actions_with_plan
        };

        for (action, plan) in actions_with_plan {
            log_action(&action, plan);
            if plan == OverwriteSafeActionPlan::SkipSameContent {
                continue;
            }

            match action {
                OverwriteSafeAction::CopyFile { source, target } => {
                    copy(source, target)?;
                }
                OverwriteSafeAction::CopyFileTransformed {
                    source_content_transformed,
                    target,
                    ..
                } => {
                    write_str(target, &source_content_transformed)?;
                }
                OverwriteSafeAction::WriteFile { content, target } => {
                    write_str(target, &content)?;
                }
            }
        }

        Ok(Vec::new())
    }

    fn plan_for_action<P, F>(
        allow_overwrite: bool,
        allow_skip_by_content: bool,
        target: P,
        skip_by_content: F,
    ) -> anyhow::Result<Option<OverwriteSafeActionPlan>>
    where
        P: AsRef<Path>,
        F: FnOnce() -> anyhow::Result<bool>,
    {
        if !target.as_ref().exists() {
            Ok(Some(OverwriteSafeActionPlan::Create))
        } else if allow_skip_by_content && skip_by_content()? {
            Ok(Some(OverwriteSafeActionPlan::SkipSameContent))
        } else if allow_overwrite {
            Ok(Some(OverwriteSafeActionPlan::Overwrite))
        } else {
            Ok(None)
        }
    }
}

pub fn resolve_relative_glob<P: AsRef<Path>, S: AsRef<str>>(
    base_dir: P,
    glob: S,
) -> anyhow::Result<(PathBuf, String)> {
    let glob = glob.as_ref();
    let path = Path::new(glob);

    let mut prefix_path = PathBuf::new();
    let mut resolved_path = PathBuf::new();
    let mut prefix_ended = false;

    for component in path.components() {
        match &component {
            Component::Prefix(_) => {
                bail!(
                    "Unexpected path prefix in glob: {}",
                    glob.log_color_error_highlight()
                );
            }
            Component::RootDir => {
                bail!(
                    "Unexpected root prefix in glob: {}",
                    glob.log_color_error_highlight()
                );
            }
            Component::CurDir => {
                // NOP
            }
            Component::ParentDir => {
                if prefix_ended {
                    if !resolved_path.pop() {
                        bail!(
                            "Too many parent directories in glob: {}",
                            glob.log_color_error_highlight()
                        );
                    }
                } else {
                    prefix_path.push(component);
                }
            }
            Component::Normal(component) => {
                resolved_path.push(component);
                prefix_ended = true;
            }
        }
    }

    Ok((
        base_dir.as_ref().join(prefix_path),
        path_to_str(&resolved_path)?.to_string(),
    ))
}

fn split_absolute_glob(base_dir: &Path, glob: &str) -> anyhow::Result<(PathBuf, String)> {
    let path = Path::new(glob);
    if !path.is_absolute() {
        bail!(
            "Expected absolute glob, got: {}",
            glob.log_color_error_highlight()
        );
    }

    let relative = path.strip_prefix(base_dir).map_err(|_| {
        anyhow!(
            "Absolute glob {} is outside base directory {}",
            glob.log_color_error_highlight(),
            base_dir.display().to_string().log_color_highlight()
        )
    })?;

    Ok((base_dir.to_path_buf(), path_to_str(relative)?.to_string()))
}

pub fn compile_and_collect_globs(
    root_dir_for_absolute_globs: &Path,
    root_dir_for_relative_globs: &Path,
    globs: &[String],
) -> Result<Vec<PathBuf>, Error> {
    let compiled_globs = compile_globs(
        root_dir_for_absolute_globs,
        root_dir_for_relative_globs,
        globs,
    )?;
    Ok(collect_glob_matches(&compiled_globs).into_iter().collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobExpander {
    TsConfigInclude,
}

pub fn compile_and_collect_globs_with_expanders(
    root_dir_for_absolute_globs: &Path,
    root_dir_for_relative_globs: &Path,
    globs: &[String],
    expanders: &[GlobExpander],
) -> Result<Vec<PathBuf>, Error> {
    let specs = compile_globs(
        root_dir_for_absolute_globs,
        root_dir_for_relative_globs,
        globs,
    )?;
    let mut matches = collect_glob_matches(&specs);

    if expanders.contains(&GlobExpander::TsConfigInclude) {
        for spec in specs
            .iter()
            .filter(|spec| is_explicit_tsconfig_pattern(&spec.original_pattern))
        {
            for tsconfig in collect_single_glob_matches(spec) {
                for include_pattern in read_tsconfig_include_patterns(&tsconfig) {
                    let include_specs = vec![compile_glob_spec(
                        root_dir_for_absolute_globs,
                        tsconfig.parent().unwrap_or(root_dir_for_relative_globs),
                        &include_pattern,
                    )?];
                    matches.extend(collect_glob_matches(&include_specs));
                }
            }
        }
    }

    Ok(matches.into_iter().collect())
}

#[derive(Debug)]
struct CompiledGlob {
    root_dir: PathBuf,
    glob: Glob<'static>,
    original_pattern: String,
}

fn compile_globs(
    root_dir_for_absolute_globs: &Path,
    root_dir_for_relative_globs: &Path,
    globs: &[String],
) -> Result<Vec<CompiledGlob>, Error> {
    globs
        .iter()
        .map(|pattern| {
            compile_glob_spec(
                root_dir_for_absolute_globs,
                root_dir_for_relative_globs,
                pattern,
            )
        })
        .collect()
}

fn compile_glob_spec(
    root_dir_for_absolute_globs: &Path,
    root_dir_for_relative_globs: &Path,
    pattern: &str,
) -> Result<CompiledGlob, Error> {
    let (root_dir, resolved_pattern) = if Path::new(pattern).is_absolute() {
        split_absolute_glob(root_dir_for_absolute_globs, pattern)?
    } else {
        resolve_relative_glob(root_dir_for_relative_globs, pattern)?
    };

    let normalized_pattern = normalize_str_path_as_unix(&resolved_pattern);
    let glob = Glob::new(&normalized_pattern)
        .with_context(|| anyhow!("Failed to compile glob expression: {}", resolved_pattern))?
        .into_owned();

    Ok(CompiledGlob {
        root_dir,
        glob,
        original_pattern: pattern.to_string(),
    })
}

fn collect_glob_matches(specs: &[CompiledGlob]) -> BTreeSet<PathBuf> {
    let mut matches = BTreeSet::new();
    for spec in specs {
        matches.extend(collect_single_glob_matches(spec));
    }
    matches
}

fn collect_single_glob_matches(spec: &CompiledGlob) -> BTreeSet<PathBuf> {
    spec.glob
        .walk_with_behavior(
            &spec.root_dir,
            WalkBehavior {
                link: LinkBehavior::ReadFile,
                ..WalkBehavior::default()
            },
        )
        .filter_map(|entry| entry.ok())
        .map(|walk_item| walk_item.path().to_path_buf())
        .collect::<BTreeSet<_>>()
}

fn is_explicit_tsconfig_pattern(pattern: &str) -> bool {
    Path::new(pattern)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == "tsconfig.json")
        .unwrap_or(false)
}

fn read_tsconfig_include_patterns(path: &Path) -> Vec<String> {
    let Ok(contents) = read_to_string(path) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<JsonValue>(&contents) else {
        return Vec::new();
    };

    let Some(include) = json.get("include") else {
        return Vec::new();
    };

    match include {
        JsonValue::String(value) => vec![value.clone()],
        JsonValue::Array(values) => values
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(not(windows))]
fn normalize_str_path_as_unix(pattern: &str) -> String {
    pattern.to_string()
}

#[cfg(windows)]
fn normalize_str_path(pattern: &str) -> String {
    // Replacing \ with / to make it work as a glob pattern
    pattern.replace('\\', "/")
}

pub fn delete_logged(context: &str, path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        log_warn_action(
            "Deleting",
            format!("{} {}", context, path.log_color_highlight()),
        );
        remove(path).with_context(|| {
            anyhow!(
                "Failed to delete {}, path: {}",
                context.log_color_highlight(),
                path.log_color_highlight()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use crate::fs;
    use pretty_assertions::assert_eq;
    use std::collections::BTreeSet;
    use std::path::Path;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use test_r::test;

    #[test]
    fn resolve_relative_globs() {
        let base_dir = PathBuf::from("somedir/somewhere");

        assert_eq!(
            fs::resolve_relative_glob(&base_dir, "").unwrap(),
            (base_dir.clone(), "".to_string())
        );
        assert_eq!(
            fs::resolve_relative_glob(&base_dir, "somepath/a/b/c").unwrap(),
            (base_dir.clone(), "somepath/a/b/c".to_string())
        );
        assert_eq!(
            fs::resolve_relative_glob(&base_dir, "../../target").unwrap(),
            (base_dir.join("../.."), "target".to_string())
        );
        assert_eq!(
            fs::resolve_relative_glob(&base_dir, "./.././../../target/a/b/../././c/d/.././..")
                .unwrap(),
            (base_dir.join("../../../"), "target/a".to_string())
        );
    }

    #[test]
    fn normalize_path_lexically_collapses_current_and_parent_segments() {
        assert_eq!(
            fs::normalize_path_lexically(Path::new("./a/./b/../c")),
            PathBuf::from("a/c")
        );
    }

    #[test]
    fn normalize_path_lexically_keeps_leading_parent_segments_for_relative_paths() {
        assert_eq!(
            fs::normalize_path_lexically(Path::new("../../a/b")),
            PathBuf::from("../../a/b")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn normalize_path_lexically_handles_absolute_paths() {
        assert_eq!(
            fs::normalize_path_lexically(Path::new("/tmp/golem/./a/../b")),
            PathBuf::from("/tmp/golem/b")
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn resolve_absolute_globs_under_base_dir() {
        let base_dir = PathBuf::from("/tmp/golem");

        assert_eq!(
            fs::split_absolute_glob(&base_dir, "/tmp/golem/dir/**/*.ts").unwrap(),
            (base_dir.clone(), "dir/**/*.ts".to_string())
        );
    }

    #[test]
    #[cfg(not(windows))]
    fn resolve_absolute_globs_outside_base_dir() {
        let base_dir = PathBuf::from("/tmp/golem");

        assert!(fs::split_absolute_glob(&base_dir, "/tmp/other/**/*.ts").is_err());
    }

    #[test]
    fn compile_globs_with_tsconfig_include_expander() {
        let temp_dir = TempDir::new().unwrap();
        let app_root = temp_dir.path();
        let build_dir = app_root.join("component");

        fs::create_dir_all(&build_dir).unwrap();
        fs::write_str(
            build_dir.join("tsconfig.json"),
            r#"{
  "include": ["src/**/*.ts", "shared.ts"]
}"#,
        )
        .unwrap();
        fs::write_str(build_dir.join("src/main.ts"), "export {};").unwrap();
        fs::write_str(build_dir.join("shared.ts"), "export {};").unwrap();

        let patterns = vec!["tsconfig.json".to_string()];

        let without_expander =
            fs::compile_and_collect_globs(app_root, &build_dir, &patterns).unwrap();
        let with_expander = fs::compile_and_collect_globs_with_expanders(
            app_root,
            &build_dir,
            &patterns,
            &[fs::GlobExpander::TsConfigInclude],
        )
        .unwrap();

        assert_eq!(
            path_set(without_expander),
            path_set(vec![build_dir.join("tsconfig.json")])
        );

        assert_eq!(
            path_set(with_expander),
            path_set(vec![
                build_dir.join("tsconfig.json"),
                build_dir.join("src/main.ts"),
                build_dir.join("shared.ts"),
            ])
        );
    }

    #[test]
    fn compile_globs_with_tsconfig_include_expander_is_lenient() {
        let temp_dir = TempDir::new().unwrap();
        let app_root = temp_dir.path();
        let build_dir = app_root.join("component");

        fs::create_dir_all(&build_dir).unwrap();
        fs::write_str(build_dir.join("tsconfig.json"), "{ invalid json }").unwrap();
        fs::write_str(build_dir.join("src/main.ts"), "export {};").unwrap();

        let patterns = vec!["tsconfig.json".to_string()];

        let with_expander = fs::compile_and_collect_globs_with_expanders(
            app_root,
            &build_dir,
            &patterns,
            &[fs::GlobExpander::TsConfigInclude],
        )
        .unwrap();

        assert_eq!(
            path_set(with_expander),
            path_set(vec![build_dir.join("tsconfig.json")])
        );
    }

    fn path_set(paths: Vec<PathBuf>) -> BTreeSet<PathBuf> {
        paths
            .into_iter()
            .map(|path| normalize_for_assert(path.as_path()))
            .collect()
    }

    fn normalize_for_assert(path: &Path) -> PathBuf {
        fs::canonicalize_path(path).unwrap_or_else(|_| path.to_path_buf())
    }
}
