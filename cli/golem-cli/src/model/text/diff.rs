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
        logln("");
        if !self.components.is_empty() {
            logln("Component changes:".log_color_help_group().to_string());
            for (component_name, component_diff) in &self.components {
                match component_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} component {}",
                            "create".green(),
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
                        DiffForHashOf::HashDiff { .. } => {
                            logln(format!(
                                "  - {} component {}",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                        }
                        DiffForHashOf::ValueDiff { diff } => {
                            logln(format!(
                                "  - {} component {}, changes:",
                                "update".yellow(),
                                component_name.log_color_highlight()
                            ));
                            if diff.metadata_changed {
                                logln("    - metadata");
                            }
                            if diff.wasm_changed {
                                logln("    - binary");
                            }
                            if !diff.file_changes.is_empty() {
                                logln("    - files");
                                for (path, file_diff) in &diff.file_changes {
                                    match file_diff {
                                        BTreeMapDiffValue::Create => {
                                            logln(format!(
                                                "      - {} file {}",
                                                "create".green(),
                                                path.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Delete => {
                                            logln(format!(
                                                "      - {} file {}",
                                                "delete".red(),
                                                path.log_color_highlight()
                                            ));
                                        }
                                        BTreeMapDiffValue::Update(diff) => match diff {
                                            DiffForHashOf::HashDiff { .. } => {
                                                logln(format!(
                                                    "      - {} file {}",
                                                    "update".yellow(),
                                                    path.log_color_highlight()
                                                ));
                                            }
                                            DiffForHashOf::ValueDiff { diff } => {
                                                logln(format!(
                                                    "      - {} file {}, changes:",
                                                    "update".yellow(),
                                                    path.log_color_highlight()
                                                ));
                                                if diff.content_changed {
                                                    logln("        - content");
                                                }
                                                if diff.permissions_changed {
                                                    logln("        - permissions");
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                            if diff.plugins_changed {
                                logln("    - plugins");
                                // TODO: atomic
                            }
                        }
                    },
                }
            }
            logln("");
        }
        if !self.http_api_definitions.is_empty() {
            logln(
                "HTTP API definition changes:"
                    .log_color_help_group()
                    .to_string(),
            );
            for (http_api_definition_name, http_api_definition_diff) in &self.http_api_definitions {
                match http_api_definition_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} HTTP API definition {}",
                            "create".green(),
                            http_api_definition_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} HTTP API definition {}",
                            "delete".red(),
                            http_api_definition_name.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(_) => {
                        logln(format!(
                            "  - {} HTTP API definition {}",
                            "update".yellow(),
                            http_api_definition_name.log_color_highlight()
                        ));
                    }
                }
            }
            logln("");
        }
        if !self.http_api_deployments.is_empty() {
            logln(
                "HTTP API deployment changes:"
                    .log_color_help_group()
                    .to_string(),
            );
            for (domain, http_api_deployment_diff) in &self.http_api_deployments {
                match http_api_deployment_diff {
                    BTreeMapDiffValue::Create => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "create".green(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Delete => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "delete".red(),
                            domain.log_color_highlight()
                        ));
                    }
                    BTreeMapDiffValue::Update(_) => {
                        logln(format!(
                            "  - {} HTTP API deployment {}",
                            "update".yellow(),
                            domain.log_color_highlight()
                        ));
                    }
                }
            }
            logln("");
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
