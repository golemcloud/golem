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

use crate::log::{log_action, LogColorize};
use golem_common::model::agent::AgentType;
use std::path::Path;

/// Extracts the implemented agent types from the given WASM component, assuming it implements the `golem:agent/guest` interface.
/// If it does not, it fails.
pub async fn extract_agent_types(wasm_path: &Path) -> anyhow::Result<Vec<AgentType>> {
    log_action(
        "Extracting",
        format!(
            "agent types from {}",
            wasm_path
                .to_string_lossy()
                .to_string()
                .log_color_highlight()
        ),
    );

    golem_common::model::agent::extraction::extract_agent_types(wasm_path).await
}
