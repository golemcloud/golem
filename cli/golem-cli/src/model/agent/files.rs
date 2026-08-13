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

use crate::log::logln;
use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFilesView {
    pub nodes: Vec<FileNodeView>,
}

impl StructuredOutput for AgentFilesView {
    const KIND: &'static str = "agent.files";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNodeView {
    pub name: String,
    pub last_modified: String, // Human-readable timestamp
    pub kind: String,
    pub permissions: String,
    pub size: u64,
}

impl TextOutput for AgentFilesView {
    fn log(&self) {
        if self.nodes.is_empty() {
            logln("No files found.");
        } else {
            let mut table = new_table_full_condensed(vec![
                Column::new("Name"),
                Column::new("Kind").fixed(),
                Column::new("Permissions").fixed(),
                Column::new("Size").fixed_right(),
                Column::new("Last Modified").fixed_right(),
            ]);
            for node in &self.nodes {
                table.add_row(vec![
                    node.name.clone(),
                    node.kind.clone(),
                    node.permissions.clone(),
                    format_binary_size(&node.size),
                    node.last_modified.clone(),
                ]);
            }
            log_table(table);
        }
    }
}
