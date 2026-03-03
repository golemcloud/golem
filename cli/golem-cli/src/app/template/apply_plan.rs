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

use crate::app::template::generator::InMemoryFs;
use crate::edit::{golem_yaml, json};
use crate::fs;
use golem_common::model::diff;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateApplyPlanEntry {
    Create { new: String },
    Update { current: String, new: String },
    Merge { current: String, new: String },
    SkipSame,
}

#[derive(Debug, Default, Clone)]
pub struct TemplateApplyPlan {
    pub entries: BTreeMap<PathBuf, TemplateApplyPlanEntry>,
}

impl TemplateApplyPlan {
    pub fn new(in_memory_fs: &InMemoryFs) -> anyhow::Result<TemplateApplyPlan> {
        let mut entries = BTreeMap::new();
        for (path, new) in in_memory_fs.files() {
            let current = path
                .exists()
                .then(|| fs::read_to_string(path))
                .transpose()?;

            let entry = match current {
                None => TemplateApplyPlanEntry::Create { new: new.clone() },
                Some(current) => {
                    if current == *new {
                        TemplateApplyPlanEntry::SkipSame
                    } else if let Some(merged) = try_merge(path, &current, new)? {
                        if merged == current {
                            TemplateApplyPlanEntry::SkipSame
                        } else {
                            TemplateApplyPlanEntry::Merge {
                                new: merged,
                                current: current.clone(),
                            }
                        }
                    } else {
                        TemplateApplyPlanEntry::Update {
                            current: current.clone(),
                            new: new.clone(),
                        }
                    }
                }
            };

            entries.insert(path.clone(), entry);
        }

        Ok(TemplateApplyPlan { entries })
    }

    pub fn apply(&self) -> anyhow::Result<()> {
        todo!()
    }
}

fn try_merge(path: &Path, current: &str, new: &str) -> anyhow::Result<Option<String>> {
    let file_name = fs::file_name_to_str(path)?;
    Ok(match file_name {
        "golem.yaml" => Some(golem_yaml::merge_documents(current, new)?),
        "tsconfig.json" => Some(json::merge_object(current, new)?),
        "package.json" => Some(json::merge_object(current, new)?),
        _ => None,
    })
}
