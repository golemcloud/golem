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

use crate::app::edit;
use crate::app::template::generator::InMemoryFs;
use crate::fs;
use crate::log::LogColorize;
use anyhow::{Context, bail};
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePlanStep {
    Create { new: String },
    Overwrite { current: String, new: String },
    Merge { current: String, new: String },
    SkipSame { current: String },
}

pub struct TemplatePlan {
    file_steps: BTreeMap<PathBuf, anyhow::Result<TemplatePlanStep>>,
}

pub struct SafeTemplatePlan {
    file_steps: BTreeMap<PathBuf, SafeTemplatePlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafeTemplatePlanStep {
    Create { new: String },
    Merge { current: String, new: String },
    SkipSame { current: String },
}

pub struct UnsafeTemplatePlan {
    file_steps: BTreeMap<PathBuf, UnsafeTemplatePlanStep>,
}

pub enum UnsafeTemplatePlanStep {
    Overwrite { current: String, new: String },
    FailedPlan { error: anyhow::Error },
}

impl TemplatePlan {
    pub fn file_steps(&self) -> &BTreeMap<PathBuf, anyhow::Result<TemplatePlanStep>> {
        &self.file_steps
    }

    pub fn partition(self) -> (SafeTemplatePlan, UnsafeTemplatePlan) {
        let mut safe_steps = BTreeMap::new();
        let mut unsafe_steps = BTreeMap::new();

        for (path, step) in self.file_steps {
            match step {
                Ok(TemplatePlanStep::Create { new }) => {
                    safe_steps.insert(path, SafeTemplatePlanStep::Create { new });
                }
                Ok(TemplatePlanStep::Overwrite { current, new }) => {
                    unsafe_steps.insert(path, UnsafeTemplatePlanStep::Overwrite { current, new });
                }
                Ok(TemplatePlanStep::Merge { current, new }) => {
                    safe_steps.insert(path, SafeTemplatePlanStep::Merge { current, new });
                }
                Ok(TemplatePlanStep::SkipSame { current }) => {
                    safe_steps.insert(path, SafeTemplatePlanStep::SkipSame { current });
                }
                Err(error) => {
                    unsafe_steps.insert(path, UnsafeTemplatePlanStep::FailedPlan { error });
                }
            }
        }

        (
            SafeTemplatePlan {
                file_steps: safe_steps,
            },
            UnsafeTemplatePlan {
                file_steps: unsafe_steps,
            },
        )
    }
}

impl SafeTemplatePlan {
    pub fn is_empty(&self) -> bool {
        self.file_steps.is_empty()
    }

    pub fn file_steps(&self) -> &BTreeMap<PathBuf, SafeTemplatePlanStep> {
        &self.file_steps
    }
}

impl UnsafeTemplatePlan {
    pub fn is_empty(&self) -> bool {
        self.file_steps.is_empty()
    }

    pub fn file_steps(&self) -> &BTreeMap<PathBuf, UnsafeTemplatePlanStep> {
        &self.file_steps
    }

    pub fn overwrites(&self) -> impl Iterator<Item = &Path> {
        self.file_steps
            .iter()
            .filter_map(|(path, step)| match step {
                UnsafeTemplatePlanStep::Overwrite { .. } => Some(path.as_ref()),
                _ => None,
            })
    }

    pub fn failed_plans(&self) -> impl Iterator<Item = (&PathBuf, &anyhow::Error)> {
        self.file_steps
            .iter()
            .filter_map(|(path, step)| match step {
                UnsafeTemplatePlanStep::FailedPlan { error } => Some((path, error)),
                _ => None,
            })
    }
}

#[derive(Debug, Default)]
pub struct TemplatePlanBuilder {
    file_steps: BTreeMap<PathBuf, Vec<FallibleNamedTemplatePlanStep>>,
}

impl TemplatePlanBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &BTreeMap<PathBuf, Vec<FallibleNamedTemplatePlanStep>> {
        &self.file_steps
    }

    pub fn add(&mut self, name: impl Into<String>, in_memory_fs: &InMemoryFs) {
        let name = name.into();
        for (path, new) in in_memory_fs.files() {
            let last_step = match self.last_step(path) {
                Some(step) => Some(step.step.as_ref().ok()),
                None => Some(None),
            };

            let Some(last_step) = last_step else {
                continue;
            };

            let next_step = self.plan_next_step_for_file(path, new, last_step);

            self.file_steps
                .entry(path.clone())
                .or_default()
                .push(FallibleNamedTemplatePlanStep {
                    name: name.clone(),
                    step: next_step,
                });
        }
    }

    fn plan_next_step_for_file(
        &self,
        path: &Path,
        new: &str,
        last_step: Option<&TemplatePlanStep>,
    ) -> anyhow::Result<TemplatePlanStep> {
        let current = if let Some(existing) = last_step {
            Some(existing.planned_contents().to_string())
        } else {
            path.exists()
                .then(|| fs::read_to_string(path))
                .transpose()?
        };

        Ok(match current {
            None => TemplatePlanStep::Create {
                new: new.to_string(),
            },
            Some(current) => {
                if current == *new {
                    TemplatePlanStep::SkipSame { current }
                } else if let Some(merged) = try_merge(path, &current, new)? {
                    if merged == current {
                        TemplatePlanStep::SkipSame { current }
                    } else {
                        TemplatePlanStep::Merge {
                            new: merged,
                            current,
                        }
                    }
                } else {
                    TemplatePlanStep::Overwrite {
                        current,
                        new: new.to_string(),
                    }
                }
            }
        })
    }

    fn last_step(&self, path: &Path) -> Option<&FallibleNamedTemplatePlanStep> {
        self.file_steps.get(path).and_then(|steps| steps.last())
    }

    pub fn build(self) -> TemplatePlan {
        let mut file_steps = BTreeMap::new();

        for (path, steps) in self.file_steps {
            let flattened_step =
                steps
                    .into_iter()
                    .map(|named_step| named_step.step)
                    .reduce(|flattened, next| {
                        let flattened = flattened?;
                        let next = next?;

                        Ok(match flattened {
                            TemplatePlanStep::Create { .. } => match next {
                                TemplatePlanStep::Create { .. } => {
                                    bail!("Illegal template step sequence: Create, Create");
                                }
                                TemplatePlanStep::Overwrite { .. } => {
                                    bail!("Illegal template step sequence: Create, Overwrite");
                                }
                                TemplatePlanStep::Merge { current: _, new } => {
                                    TemplatePlanStep::Create { new }
                                }
                                TemplatePlanStep::SkipSame { current } => {
                                    TemplatePlanStep::Create { new: current }
                                }
                            },
                            TemplatePlanStep::Overwrite {
                                current: prev_current,
                                new: prev_new,
                            } => match next {
                                TemplatePlanStep::Create { .. } => {
                                    bail!("Illegal template step sequence: Overwrite, Create");
                                }
                                TemplatePlanStep::Overwrite { current, new } => {
                                    TemplatePlanStep::Overwrite { current, new }
                                }
                                TemplatePlanStep::Merge { .. } => {
                                    bail!("Illegal template step sequence: Overwrite, Merge")
                                }
                                TemplatePlanStep::SkipSame { .. } => TemplatePlanStep::Overwrite {
                                    current: prev_current,
                                    new: prev_new,
                                },
                            },
                            TemplatePlanStep::Merge {
                                current: prev_current,
                                new: prev_new,
                            } => match next {
                                TemplatePlanStep::Create { .. } => {
                                    bail!("Illegal template step sequence: Merge, Create");
                                }
                                TemplatePlanStep::Overwrite { .. } => {
                                    bail!("Illegal template step sequence: Merge, Overwrite");
                                }
                                TemplatePlanStep::Merge { current: _, new } => {
                                    if prev_current == new {
                                        TemplatePlanStep::SkipSame {
                                            current: prev_current,
                                        }
                                    } else {
                                        TemplatePlanStep::Merge {
                                            current: prev_current,
                                            new,
                                        }
                                    }
                                }
                                TemplatePlanStep::SkipSame { .. } => TemplatePlanStep::Merge {
                                    current: prev_current,
                                    new: prev_new,
                                },
                            },
                            TemplatePlanStep::SkipSame { .. } => match next {
                                TemplatePlanStep::Create { .. } => {
                                    bail!("Illegal template step sequence: SkipSame, Create");
                                }
                                TemplatePlanStep::Overwrite { .. } => {
                                    bail!("Illegal template step sequence: SkipSame, Overwrite");
                                }
                                TemplatePlanStep::Merge { current, new } => {
                                    TemplatePlanStep::Merge { current, new }
                                }
                                TemplatePlanStep::SkipSame { current } => {
                                    TemplatePlanStep::SkipSame { current }
                                }
                            },
                        })
                    });

            if let Some(flattened_step) = flattened_step {
                file_steps.insert(path.to_path_buf(), flattened_step);
            }
        }

        TemplatePlan { file_steps }
    }
}

#[derive(Debug)]
pub struct FallibleNamedTemplatePlanStep {
    pub name: String,
    pub step: anyhow::Result<TemplatePlanStep>,
}

impl TemplatePlanStep {
    fn planned_contents(&self) -> &str {
        match self {
            TemplatePlanStep::Create { new } => new,
            TemplatePlanStep::Overwrite { new, .. } => new,
            TemplatePlanStep::Merge { new, .. } => new,
            TemplatePlanStep::SkipSame { current } => current,
        }
    }
}

fn try_merge(path: &Path, current: &str, new: &str) -> anyhow::Result<Option<String>> {
    let file_name = fs::file_name_to_str(path)?;

    fn merge(file_name: &str, current: &str, new: &str) -> anyhow::Result<Option<String>> {
        Ok(match file_name {
            ".gitignore" => Some(edit::gitignore::merge(current, new)),
            "AGENTS.md" => Some(merge_with_validation(
                current,
                new,
                edit::agents_md::validate,
                edit::agents_md::merge_guides,
            )?),
            "golem.yaml" => Some(merge_with_validation(
                current,
                new,
                validate_yaml,
                edit::golem_yaml::merge_documents,
            )?),
            "main.ts" => Some(merge_with_validation(
                current,
                new,
                edit::main_ts::validate,
                edit::main_ts::merge_imports,
            )?),
            "lib.rs" => Some(merge_with_validation(
                current,
                new,
                edit::main_rs::validate,
                edit::main_rs::merge_reexports,
            )?),
            "package.json" => Some(merge_with_validation(
                current,
                new,
                validate_json,
                edit::json::merge_object,
            )?),
            "Cargo.toml" => Some(merge_with_validation(
                current,
                new,
                validate_toml,
                edit::cargo_toml::merge_documents,
            )?),
            "tsconfig.json" => Some(merge_with_validation(
                current,
                new,
                validate_json,
                edit::json::merge_object,
            )?),
            _ => None,
        })
    }

    merge(file_name, current, new)
        .inspect_err(|_err| {
            warn!("merge: file name: {}", file_name);
            warn!("merge: current:\n{}\n", current);
            warn!("merge: new:\n{}\n", new);
        })
        .with_context(|| format!("Failed to merge {}", file_name.log_color_error_highlight()))
}

fn ensure_valid(
    label: &str,
    source: &str,
    parse: fn(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    parse(source).with_context(|| format!("{} content is not valid", label))?;
    Ok(())
}

fn merge_with_validation(
    current: &str,
    new: &str,
    parse: fn(&str) -> anyhow::Result<()>,
    merge: fn(&str, &str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    ensure_valid("current", current, parse)?;
    ensure_valid("new", new, parse)?;
    let merged = merge(current, new)?;
    ensure_valid("merged", &merged, parse)?;
    Ok(merged)
}

fn validate_json(source: &str) -> anyhow::Result<()> {
    serde_json::from_str::<JsonValue>(source)?;
    Ok(())
}

fn validate_toml(source: &str) -> anyhow::Result<()> {
    source.parse::<toml_edit::DocumentMut>()?;
    Ok(())
}

fn validate_yaml(source: &str) -> anyhow::Result<()> {
    serde_yaml::from_str::<YamlValue>(source)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultiComponentLayoutUpgradePlanStep {
    Move { source: PathBuf, target: PathBuf },
    Rewrite { path: PathBuf, edit: TextEdit },
}

/// A targeted in-place edit applied to an existing source file as part of a
/// multi-component layout upgrade. Edits are designed to preserve any unrelated
/// user customizations in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEdit {
    /// Replace the argument of the first `.in(file("..."))` call inside the
    /// `lazy val <lazy_val> = project` block of an sbt file.
    ScalaSbtComponentDir { lazy_val: String, new_dir: String },
}

impl TextEdit {
    /// Apply the edit to `current` and return the new file contents.
    /// Returns an error if the file structure does not match what the edit expects.
    pub fn apply(&self, current: &str) -> anyhow::Result<String> {
        match self {
            TextEdit::ScalaSbtComponentDir { lazy_val, new_dir } => {
                rewrite_scala_sbt_component_dir(current, lazy_val, new_dir)
            }
        }
    }

    /// Short, human-readable description of the edit, suitable for log lines.
    pub fn describe(&self) -> String {
        match self {
            TextEdit::ScalaSbtComponentDir { lazy_val, new_dir } => format!(
                "set componentDir of `lazy val {}` to {:?}",
                lazy_val, new_dir
            ),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MultiComponentLayoutUpgradePlan {
    steps: Vec<MultiComponentLayoutUpgradePlanStep>,
}

impl MultiComponentLayoutUpgradePlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, step: MultiComponentLayoutUpgradePlanStep) {
        self.steps.push(step);
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn steps(&self) -> &[MultiComponentLayoutUpgradePlanStep] {
        &self.steps
    }
}

fn rewrite_scala_sbt_component_dir(
    current: &str,
    lazy_val: &str,
    new_dir: &str,
) -> anyhow::Result<String> {
    use anyhow::anyhow;

    // Locate the `lazy val <lazy_val>` declaration.
    let needle = format!("lazy val {}", lazy_val);
    let lazy_val_offsets: Vec<usize> = current.match_indices(&needle).map(|(i, _)| i).collect();
    if lazy_val_offsets.is_empty() {
        return Err(anyhow!(
            "expected to find `lazy val {}` in the sbt file but did not",
            lazy_val
        ));
    }
    if lazy_val_offsets.len() > 1 {
        return Err(anyhow!(
            "expected exactly one `lazy val {}` declaration in the sbt file, found {}",
            lazy_val,
            lazy_val_offsets.len()
        ));
    }

    let lazy_val_start = lazy_val_offsets[0];

    // From the lazy val onwards, find the first `.in(file("..."))` call and replace
    // its quoted argument.
    let after = &current[lazy_val_start..];
    let prefix = ".in(file(\"";
    let Some(in_call_offset_in_after) = after.find(prefix) else {
        return Err(anyhow!(
            "expected `.in(file(\"...\"))` inside the `lazy val {}` block but did not find one",
            lazy_val
        ));
    };
    let arg_start_in_current = lazy_val_start + in_call_offset_in_after + prefix.len();

    let after_arg = &current[arg_start_in_current..];
    let Some(arg_end_in_after) = after_arg.find('"') else {
        return Err(anyhow!(
            "unterminated string literal in `.in(file(\"...\"))` for `lazy val {}`",
            lazy_val
        ));
    };
    let arg_end_in_current = arg_start_in_current + arg_end_in_after;

    let mut result = String::with_capacity(current.len() + new_dir.len());
    result.push_str(&current[..arg_start_in_current]);
    result.push_str(new_dir);
    result.push_str(&current[arg_end_in_current..]);
    Ok(result)
}

#[cfg(test)]
mod sbt_tests {
    use super::*;
    use test_r::test;

    fn flat_sbt() -> String {
        r#"import org.scalajs.linker.interface.ModuleKind

lazy val component_name = project
  .in(file("."))
  .enablePlugins(org.scalajs.sbtplugin.ScalaJSPlugin, golem.sbt.GolemPlugin)
  .settings(
    name := "component-name",
    scalaJSUseMainModuleInitializer := false,
    scalacOptions += "-experimental",
    libraryDependencies ++= Seq(
      "cloud.golem" %%% "golem-scala-core"  % "0.1.0",
    ),
  )
"#
        .to_string()
    }

    #[test]
    fn scala_sbt_component_dir_edits_in_place() {
        let edited =
            rewrite_scala_sbt_component_dir(&flat_sbt(), "component_name", "scala-main").unwrap();
        assert!(edited.contains("project\n  .in(file(\"scala-main\"))\n"));
        // user-added settings are preserved
        assert!(edited.contains("scalacOptions += \"-experimental\""));
        assert!(edited.contains("golem-scala-core"));
    }

    #[test]
    fn scala_sbt_component_dir_is_idempotent() {
        let once =
            rewrite_scala_sbt_component_dir(&flat_sbt(), "component_name", "scala-main").unwrap();
        let twice = rewrite_scala_sbt_component_dir(&once, "component_name", "scala-main").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn scala_sbt_component_dir_preserves_user_added_dependencies() {
        let with_user_edit = flat_sbt().replace(
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n",
            "      \"cloud.golem\" %%% \"golem-scala-core\"  % \"0.1.0\",\n      \"dev.zio\" %%% \"zio\" % \"2.1.0\",\n",
        );
        let edited =
            rewrite_scala_sbt_component_dir(&with_user_edit, "component_name", "scala-main")
                .unwrap();
        assert!(edited.contains("\"dev.zio\" %%% \"zio\""));
        assert!(edited.contains(".in(file(\"scala-main\"))"));
    }

    #[test]
    fn scala_sbt_component_dir_errors_when_lazy_val_missing() {
        let err = rewrite_scala_sbt_component_dir("// nothing here\n", "component_name", "x")
            .unwrap_err();
        assert!(err.to_string().contains("lazy val component_name"));
    }

    #[test]
    fn scala_sbt_component_dir_errors_when_in_file_missing() {
        let src = "lazy val component_name = project\n  .settings()\n";
        let err = rewrite_scala_sbt_component_dir(src, "component_name", "x").unwrap_err();
        assert!(err.to_string().contains(".in(file"));
    }
}
