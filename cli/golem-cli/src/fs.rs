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

use crate::log::LogColorize;
use anyhow::{anyhow, bail, Context, Error};
use std::cmp::PartialEq;
use std::fs::{Metadata, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;
use wax::{Glob, LinkBehavior, WalkBehavior};

pub fn create_dir_all<P: AsRef<Path>>(path: P) -> anyhow::Result<()> {
    let path = path.as_ref();
    if path.exists() {
        Ok(())
    } else {
        std::fs::create_dir_all(path)
            .with_context(|| anyhow!("Failed to create directory {}", path.log_color_highlight()))
    }
}

// Differences compared to std::fs::copy
//  - ensures that the target dir exists
//  - updated the modtime after copy, which is not guaranteed to happen, making it not usable for
//    modtime based up-to-date checks (see https://github.com/rust-lang/rust/issues/115982 for more info)
//  - uses anyhow error with added context
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<u64> {
    let from = PathExtra(from);
    let to = PathExtra(to);

    let context = || format!("Failed to copy from {} to {}", from.display(), to.display());

    create_dir_all(to.parent()?)
        .context("Failed to create target dir")
        .with_context(context)?;

    let bytes = std::fs::copy(&from, &to).with_context(context)?;

    std::fs::File::open(&to)
        .and_then(|to| to.set_modified(SystemTime::now()))
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
    let from = PathExtra(from);
    let to = PathExtra(to);

    let context = || {
        format!(
            "Failed to copy (and transform) from {} to {}",
            from.display(),
            to.display()
        )
    };

    create_dir_all(from.parent()?)
        .context("Failed to create target dir")
        .with_context(context)?;

    let content = read_to_string(&from).with_context(context)?;

    let transformed_content = transform(content)
        .context("Failed to transform source content")
        .with_context(context)?;

    let bytes_count = transformed_content.len();

    write(&to, transformed_content.as_bytes())
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
    let path = PathExtra(path);
    let str = str.as_ref();

    let context = || anyhow!("Failed to write string to {}", path.log_color_highlight());

    let target_parent = path.parent().with_context(context)?;
    create_dir_all(target_parent).with_context(context)?;
    std::fs::write(&path, str.as_bytes()).with_context(context)
}

pub fn append_str<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<()> {
    let path = PathExtra(path);
    let str = str.as_ref();

    let context = || anyhow!("Failed to write string to {}", path.log_color_highlight());
    let mut file = OpenOptions::new()
        .append(true)
        .open(&path)
        .with_context(context)?;
    file.write(str.as_bytes()).with_context(context).map(|_| ())
}

pub fn write<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> anyhow::Result<()> {
    let path = PathExtra(path);

    let context = || anyhow!("Failed to write to {}", path.log_color_highlight());

    let target_parent = path.parent().with_context(context)?;
    create_dir_all(target_parent).with_context(context)?;
    std::fs::write(&path, contents).with_context(context)
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

// TODO: change it to "extension methods" on path and path buf?
pub struct PathExtra<P: AsRef<Path>>(P);

impl<P: AsRef<Path>> PathExtra<P> {
    pub fn new(path: P) -> Self {
        Self(path)
    }

    pub fn parent(&self) -> anyhow::Result<&Path> {
        let path = self.0.as_ref();
        path.parent().ok_or_else(|| {
            anyhow!(
                "Failed to get parent dir for path: {}",
                path.log_color_highlight()
            )
        })
    }

    pub fn file_name_to_string(&self) -> anyhow::Result<String> {
        let path = self.0.as_ref();
        path.file_name()
            .ok_or_else(|| {
                anyhow!(
                    "Failed to get file name for path: {}",
                    path.log_color_highlight(),
                )
            })?
            .to_os_string()
            .into_string()
            .map_err(|_| {
                anyhow!(
                    "Failed to convert filename for path: {}",
                    path.log_color_highlight()
                )
            })
    }

    pub fn to_str(&self) -> anyhow::Result<&str> {
        let path = self.0.as_ref();
        path.to_str().ok_or_else(|| {
            anyhow!(
                "Failed to convert path to string: {}",
                path.log_color_highlight()
            )
        })
    }

    pub fn to_string(&self) -> anyhow::Result<String> {
        Ok(self.to_str()?.to_string())
    }

    pub fn strip_prefix<Q: AsRef<Path>>(&self, prefix: Q) -> anyhow::Result<PathBuf> {
        let path = self.0.as_ref();
        let prefix = prefix.as_ref();

        Ok(path
            .strip_prefix(prefix)
            .with_context(|| {
                anyhow!(
                    "Failed to strip prefix from path, prefix: {}, path: {}",
                    prefix.log_color_highlight(),
                    path.log_color_highlight()
                )
            })?
            .to_path_buf())
    }

    pub fn as_path(&self) -> &Path {
        self.0.as_ref()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.as_ref().into()
    }

    pub fn display(&self) -> std::path::Display<'_> {
        self.as_path().display()
    }
}

impl<P: AsRef<Path>> AsRef<Path> for PathExtra<P> {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
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
        PathExtra::new(resolved_path).to_string()?,
    ))
}

pub fn compile_and_collect_globs(root_dir: &Path, globs: &[String]) -> Result<Vec<PathBuf>, Error> {
    Ok(globs
        .iter()
        .map(|pattern| resolve_relative_glob(root_dir, pattern))
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(|(root_dir, pattern)| {
            Glob::new(pattern)
                .with_context(|| anyhow!("Failed to compile glob expression: {}", pattern))
                .map(|pattern| (root_dir, pattern))
        })
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .flat_map(|(root_dir, glob)| {
            glob.walk_with_behavior(
                root_dir,
                WalkBehavior {
                    link: LinkBehavior::ReadFile,
                    ..WalkBehavior::default()
                },
            )
            .filter_map(|entry| entry.ok())
            .map(|walk_item| walk_item.path().to_path_buf())
        })
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod test {
    use crate::fs::resolve_relative_glob;
    use assert2::check;
    use std::path::PathBuf;
    use test_r::test;

    #[test]
    fn resolve_relative_globs() {
        let base_dir = PathBuf::from("somedir/somewhere");

        check!(resolve_relative_glob(&base_dir, "").unwrap() == (base_dir.clone(), "".to_string()));
        check!(
            resolve_relative_glob(&base_dir, "somepath/a/b/c").unwrap()
                == (base_dir.clone(), "somepath/a/b/c".to_string())
        );
        check!(
            resolve_relative_glob(&base_dir, "../../target").unwrap()
                == (base_dir.join("../.."), "target".to_string())
        );
        check!(
            resolve_relative_glob(&base_dir, "./.././../../target/a/b/../././c/d/.././..").unwrap()
                == (base_dir.join("../../../"), "target/a".to_string())
        );
    }
}
