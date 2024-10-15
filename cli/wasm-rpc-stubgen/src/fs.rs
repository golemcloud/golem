use anyhow::{anyhow, Context};
use std::cmp::PartialEq;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// Differences compared to std::fs::copy
//  - ensures that the target dir exists
//  - updated the modtime after copy, which is not guaranteed to happen, making it not usable for
//    modtime based up-to-date checks (see https://github.com/rust-lang/rust/issues/115982 for more info)
//  - uses anyhow error with added context
pub fn copy<P: AsRef<Path>, Q: AsRef<Path>>(from: P, to: Q) -> anyhow::Result<u64> {
    let from = from.as_ref();
    let to = to.as_ref();

    let context = || {
        format!(
            "Failed to copy from {} to {}",
            from.to_string_lossy(),
            to.to_string_lossy()
        )
    };

    let target_parent = to
        .parent()
        .ok_or_else(|| anyhow!("Failed to get target parent dir"))
        .with_context(context)?;

    std::fs::create_dir_all(target_parent)
        .with_context(|| anyhow!("Failed to create target dir"))
        .with_context(context)?;

    let bytes = std::fs::copy(from, to).with_context(context)?;

    std::fs::File::open(to)
        .and_then(|to| to.set_modified(SystemTime::now()))
        .with_context(|| anyhow!("Failed to update target modification time"))
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
            from.to_string_lossy(),
            to.to_string_lossy()
        )
    };

    let target_parent = to
        .parent()
        .ok_or_else(|| anyhow!("Failed to get target parent dir"))
        .with_context(context)?;

    std::fs::create_dir_all(target_parent)
        .with_context(|| anyhow!("Failed to create target dir"))
        .with_context(context)?;

    let content = std::fs::read_to_string(from)
        .with_context(|| anyhow!("Failed to read source content"))
        .with_context(context)?;

    let transformed_content = transform(content)
        .with_context(|| anyhow!("Failed to transform source content"))
        .with_context(context)?;

    let bytes_count = transformed_content.as_bytes().len();

    std::fs::write(to, transformed_content.as_bytes())
        .with_context(|| anyhow!("Failed to write transformed content"))
        .with_context(context)?;

    Ok(bytes_count as u64)
}

// Creates all missing parent directories if necessary and writes str to path.
pub fn write_str<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<()> {
    let path = path.as_ref();
    let str = str.as_ref();

    let context = || format!("Failed to write string to {}", path.to_string_lossy());

    let target_parent = path
        .parent()
        .ok_or_else(|| anyhow!("Failed to get parent dir"))
        .with_context(context)?;

    std::fs::create_dir_all(target_parent)
        .with_context(|| anyhow!("Failed to create parent dir"))
        .with_context(context)?;

    std::fs::write(path, str.as_bytes()).with_context(context)?;

    Ok(())
}

pub fn has_str_content<P: AsRef<Path>, S: AsRef<str>>(path: P, str: S) -> anyhow::Result<bool> {
    let path = path.as_ref();
    let str = str.as_ref();

    let context = || {
        format!(
            "Failed to compare content to string for {}",
            path.to_string_lossy()
        )
    };

    let content = std::fs::read_to_string(path)
        .with_context(|| anyhow!("Failed to read as string: {}", path.to_string_lossy()))
        .with_context(context)?;

    Ok(content == str)
}

pub fn has_same_string_content<P: AsRef<Path>, Q: AsRef<Path>>(a: P, b: Q) -> anyhow::Result<bool> {
    let a = a.as_ref();
    let b = b.as_ref();

    let context = || {
        format!(
            "Failed to compare string contents of {} and {}",
            a.to_string_lossy(),
            b.to_string_lossy()
        )
    };

    let content_a = std::fs::read_to_string(a)
        .with_context(|| anyhow!("Failed to read as string: {}", a.to_string_lossy()))
        .with_context(context)?;

    let content_b = std::fs::read_to_string(b)
        .with_context(|| anyhow!("Failed to read as string: {}", b.to_string_lossy()))
        .with_context(context)?;

    Ok(content_a == content_b)
}

pub fn must_get_file_name<P: AsRef<Path>>(path: P) -> anyhow::Result<OsString> {
    let path = path.as_ref();
    Ok(path
        .file_name()
        .ok_or_else(|| {
            anyhow!(
                "Failed to get file name for package source: {}",
                path.to_string_lossy(),
            )
        })?
        .to_os_string())
}

pub fn strip_path_prefix<P: AsRef<Path>, Q: AsRef<Path>>(
    path: P,
    prefix: Q,
) -> anyhow::Result<PathBuf> {
    let path = path.as_ref();
    let prefix = prefix.as_ref();

    Ok(path
        .strip_prefix(prefix)
        .with_context(|| {
            anyhow!(
                "Failed to strip prefix from path, path: {}, prefix: {}",
                path.to_string_lossy(),
                prefix.to_string_lossy()
            )
        })?
        .to_path_buf())
}

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
        F: Fn(String) -> anyhow::Result<String>,
    {
        let content = std::fs::read_to_string(&source).with_context(|| {
            anyhow!(
                "Failed to read file as string, path: {}",
                source.to_string_lossy()
            )
        })?;

        let source_transformed = transform(content).with_context(|| {
            anyhow!(
                "Failed to transform file, path: {}",
                source.to_string_lossy()
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
                    OverwriteSafeAction::CopyFile { source, target } => {
                        Self::plan_for_action(allow_overwrite, target, || {
                            has_same_string_content(source, target)
                        })?
                    }
                    OverwriteSafeAction::CopyFileTransformed {
                        source_content_transformed: source_transformed,
                        target,
                        ..
                    } => Self::plan_for_action(allow_overwrite, target, || {
                        has_str_content(target, source_transformed)
                    })?,
                    OverwriteSafeAction::WriteFile { content, target } => {
                        Self::plan_for_action(allow_overwrite, target, || {
                            has_str_content(target, content)
                        })?
                    }
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
        target: P,
        skip_by_content: F,
    ) -> anyhow::Result<Option<OverwriteSafeActionPlan>>
    where
        P: AsRef<Path>,
        F: Fn() -> anyhow::Result<bool>,
    {
        if !target.as_ref().exists() {
            Ok(Some(OverwriteSafeActionPlan::Create))
        } else if skip_by_content()? {
            Ok(Some(OverwriteSafeActionPlan::SkipSameContent))
        } else if allow_overwrite {
            Ok(Some(OverwriteSafeActionPlan::Overwrite))
        } else {
            Ok(None)
        }
    }
}
