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

use std::fmt::Write;
use uuid::Uuid;

use super::compact_value_formatter::compact_wave_element;
use super::{parse_agent_id_parts, split_top_level_commas};

/// Normalizes an agent ID string without requiring component metadata.
/// Strips unnecessary whitespace from the agent ID by parsing WAVE values and re-emitting them compactly.
pub fn normalize_agent_id_text(s: &str) -> Result<String, String> {
    let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;

    let phantom_id = phantom_id_str
        .map(|id| Uuid::parse_str(id).map_err(|e| format!("Invalid UUID in phantom ID: {e}")))
        .transpose()?;

    let elements = split_top_level_commas(param_list);
    if !param_list.trim().is_empty() && elements.iter().any(|e| e.trim().is_empty()) {
        return Err(format!(
            "Invalid agent-id parameter list (empty element): {param_list}"
        ));
    }

    let compacted_elements: Vec<String> = elements
        .iter()
        .map(|elem| compact_wave_element(elem.trim()))
        .collect();
    let compacted_params = compacted_elements.join(",");

    let mut result = format!("{agent_type_name}({compacted_params})");
    if let Some(phantom_id) = phantom_id {
        let _ = write!(result, "[{phantom_id}]");
    }
    Ok(result)
}
