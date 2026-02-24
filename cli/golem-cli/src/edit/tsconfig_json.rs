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

use crate::edit::json::{collect_value_text_by_path, merge_object_from_source};

#[derive(Debug, Clone)]
pub struct RequiredSetting {
    pub path: Vec<String>,
    pub expected_literal: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MissingSetting {
    pub path: Vec<String>,
    pub found: Option<String>,
    pub expected_literal: Option<String>,
}

pub fn merge_with_newer(base_source: &str, newer_source: &str) -> anyhow::Result<String> {
    merge_object_from_source(base_source, newer_source)
}

pub fn check_required_settings(
    source: &str,
    required: &[RequiredSetting],
) -> anyhow::Result<Vec<MissingSetting>> {
    let mut missing = Vec::new();
    for setting in required {
        let path: Vec<&str> = setting.path.iter().map(String::as_str).collect();
        let found = collect_value_text_by_path(source, &path)?;
        if let Some(expected) = &setting.expected_literal {
            if found.as_deref() != Some(expected.as_str()) {
                missing.push(MissingSetting {
                    path: setting.path.clone(),
                    found,
                    expected_literal: Some(expected.clone()),
                });
            }
        } else if found.is_none() {
            missing.push(MissingSetting {
                path: setting.path.clone(),
                found,
                expected_literal: None,
            });
        }
    }
    Ok(missing)
}
