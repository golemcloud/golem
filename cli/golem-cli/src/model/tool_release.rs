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

use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::{Column, TextOutput, log_table, new_table_full_condensed};
use golem_common::model::tool_release::ToolRelease;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReleaseView {
    pub release: ToolRelease,
}

impl StructuredOutput for ToolReleaseView {
    const KIND: &'static str = "tool.release";
}

impl TextOutput for ToolReleaseView {
    fn log(&self) {
        log_releases(std::slice::from_ref(&self.release));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReleaseListView {
    pub releases: Vec<ToolRelease>,
}

impl StructuredOutput for ToolReleaseListView {
    const KIND: &'static str = "tool.release.list";
}

impl TextOutput for ToolReleaseListView {
    fn log(&self) {
        log_releases(&self.releases);
    }
}

fn log_releases(releases: &[ToolRelease]) {
    let mut table = new_table_full_condensed(vec![
        Column::new("Release ID"),
        Column::new("Tool"),
        Column::new("Version"),
        Column::new("Lifecycle"),
        Column::new("Immutable"),
    ]);
    for release in releases {
        table.add_row(vec![
            release.id.to_string(),
            release.name.to_string(),
            release.version.clone(),
            release.lifecycle.to_string(),
            release.immutable.to_string(),
        ]);
    }
    log_table(table);
}
