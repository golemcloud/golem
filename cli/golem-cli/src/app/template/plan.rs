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

use crate::app::edit;
use crate::app::template::generator::InMemoryFs;
use crate::fs;
use crate::log::{log_action, log_skipping_up_to_date};
use anyhow::Context;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tracing::warn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplatePlanEntry {
    Create { new: String },
    Overwrite { current: String, new: String },
    Merge { current: String, new: String },
    SkipSame { current: String },
}

#[derive(Debug, Default, Clone)]
pub struct TemplatePlan {
    file_plans: BTreeMap<PathBuf, Vec<TemplatePlanLayerEntry>>,
    existing_files: BTreeMap<PathBuf, String>,
}

impl TemplatePlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &BTreeMap<PathBuf, Vec<TemplatePlanLayerEntry>> {
        &self.file_plans
    }

    pub fn existing_files(&self) -> &BTreeMap<PathBuf, String> {
        &self.existing_files
    }

    pub fn add(
        &mut self,
        name: impl Into<String>,
        in_memory_fs: &InMemoryFs,
    ) -> anyhow::Result<()> {
        let name = name.into();
        for (path, new) in in_memory_fs.files() {
            let current = if let Some(existing) = self.latest_entry(path) {
                Some(existing.planned_contents().to_string())
            } else {
                let current = path
                    .exists()
                    .then(|| fs::read_to_string(path))
                    .transpose()?;
                if let Some(loaded) = current.as_ref() {
                    self.existing_files
                        .entry(path.clone())
                        .or_insert_with(|| loaded.clone());
                }
                current
            };

            let entry = match current {
                None => TemplatePlanEntry::Create { new: new.clone() },
                Some(current) => {
                    if current == *new {
                        TemplatePlanEntry::SkipSame { current }
                    } else if let Some(merged) = try_merge(path, &current, new)? {
                        if merged == current {
                            TemplatePlanEntry::SkipSame { current }
                        } else {
                            TemplatePlanEntry::Merge {
                                new: merged,
                                current,
                            }
                        }
                    } else {
                        TemplatePlanEntry::Overwrite {
                            current,
                            new: new.clone(),
                        }
                    }
                }
            };

            self.file_plans
                .entry(path.clone())
                .or_default()
                .push(TemplatePlanLayerEntry {
                    layer_name: name.clone(),
                    entry,
                });
        }

        Ok(())
    }

    pub fn apply(&self) -> anyhow::Result<()> {
        for (path, layers) in &self.file_plans {
            let Some(entry) = self.effective_entry(layers) else {
                continue;
            };
            match entry {
                TemplatePlanEntry::Create { new } => {
                    log_action("Creating", format!("{}", path.display()));
                    fs::write_str(path, new)?;
                }
                TemplatePlanEntry::Overwrite { new, .. } => {
                    log_action("Overwriting", format!("{}", path.display()));
                    fs::write_str(path, new)?;
                }
                TemplatePlanEntry::Merge { new, .. } => {
                    log_action("Updating", format!("{}", path.display()));
                    fs::write_str(path, new)?;
                }
                TemplatePlanEntry::SkipSame { .. } => {
                    log_skipping_up_to_date(format!("updating {}", path.display()));
                }
            }
        }
        Ok(())
    }

    fn latest_entry(&self, path: &Path) -> Option<&TemplatePlanEntry> {
        self.file_plans
            .get(path)
            .and_then(|layers| layers.last())
            .map(|layer| &layer.entry)
    }

    fn effective_entry<'a>(
        &self,
        layers: &'a [TemplatePlanLayerEntry],
    ) -> Option<&'a TemplatePlanEntry> {
        layers
            .iter()
            .rev()
            .find(|layer| !matches!(layer.entry, TemplatePlanEntry::SkipSame { .. }))
            .map(|layer| &layer.entry)
            .or_else(|| layers.last().map(|layer| &layer.entry))
    }
}

#[derive(Debug, Clone)]
pub struct TemplatePlanLayerEntry {
    pub layer_name: String,
    pub entry: TemplatePlanEntry,
}

impl TemplatePlanEntry {
    fn planned_contents(&self) -> &str {
        match self {
            TemplatePlanEntry::Create { new } => new,
            TemplatePlanEntry::Overwrite { new, .. } => new,
            TemplatePlanEntry::Merge { new, .. } => new,
            TemplatePlanEntry::SkipSame { current } => current,
        }
    }
}

fn try_merge(path: &Path, current: &str, new: &str) -> anyhow::Result<Option<String>> {
    let file_name = fs::file_name_to_str(path)?;

    fn merge(file_name: &str, current: &str, new: &str) -> anyhow::Result<Option<String>> {
        Ok(match file_name {
            ".gitignore" => Some(edit::gitignore::merge(current, new)),
            "golem.yaml" => Some(edit::golem_yaml::merge_documents(current, new)?),
            "main.ts" => Some(edit::main_ts::merge_reexports(current, new)?),
            "package.json" => Some(edit::json::merge_object(current, new)?), // TODO: FCL: review if we still need the package.json specific editor
            "tsconfig.json" => Some(edit::json::merge_object(current, new)?), // TODO: FCL: review if we still need the tsconfig.json specific editor
            _ => None,
        })
    }

    merge(file_name, current, new)
        .map_err(|err| {
            warn!("merge: file name: {}", file_name);
            warn!("merge: current:\n{}\n", current);
            warn!("merge: new:\n{}\n", new);
            err
        })
        .with_context(|| format!("Failed to merge '{}'", file_name))
}
