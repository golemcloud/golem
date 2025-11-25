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

use crate::log::{logln, LogColorize};
use crate::model::text::fmt::TextView;
use colored::Colorize;
use golem_common::model::diff::{BTreeMapDiffValue, DeploymentDiff, DiffForHashOf};

impl TextView for DeploymentDiff {
    fn log(&self) {
        if !self.components.is_empty() {
            logln("Component changes:");
            for (component_name, component_diff) in &self.components {
                match component_diff {
                    BTreeMapDiffValue::Add => {
                        logln(format!(
                            "  - {} component {}",
                            "add".green(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} component {}",
                            "delete".red(),
                            component_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(diff) => match diff {
                        Some(DiffForHashOf::HashDiff { .. }) | None => {
                            logln(format!(
                                "  - {} component {}",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                        }
                        Some(DiffForHashOf::ValueDiff { diff }) => {
                            logln(format!(
                                "  - {} component {}, changes:",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                            if diff.metadata_changed {
                                logln("    - metadata");
                            }
                            if diff.binary_changed {
                                logln("    - binary");
                            }
                            if !diff.file_changes.is_empty() {
                                logln("    - files");
                                // TODO: atomic: add detailed diff for files
                            }
                            if diff.plugins_changed {
                                logln("    - plugins");
                            }
                        }
                    },
                }
            }
            if !self.http_api_definitions.is_empty() {
                logln("HTTP API definition changes:");
                // TODO: atomic
            }
            if !self.http_api_deployments.is_empty() {
                logln("HTTP API deployment changes:");
                // TODO: atomic
            }
        }
    }
}

pub fn log_unified_diff(diff: &str) {
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            logln(line.green().bold().to_string());
        } else if line.starts_with('-') && !line.starts_with("---") {
            logln(line.red().bold().to_string());
        } else if line.starts_with("@@") {
            logln(line.bold().to_string());
        } else {
            logln(line);
        }
    }
}
