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

use super::parse_agent_key_parts;
use super::structural_format::normalize_structural;

/// Normalizes agent key text without requiring component metadata.
/// Strips unnecessary whitespace outside string literals.
pub fn normalize_agent_key_text(s: &str) -> Result<String, String> {
    let (agent_type_name, param_list) = parse_agent_key_parts(s)?;
    Ok(format!(
        "{agent_type_name}({})",
        normalize_structural(param_list)
    ))
}
