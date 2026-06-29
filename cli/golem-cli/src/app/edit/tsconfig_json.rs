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

use crate::app::edit::json::{build_object_source, collect_value_text_by_path, merge_object};

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
    merge_object(base_source, newer_source)
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

/// Builds the JSON object (to be deep-merged via [`merge_with_newer`]) that fixes the
/// given missing settings, by placing each one's expected literal at its full path.
/// The object shape (e.g. the `compilerOptions` wrapper) follows from the paths
/// themselves. Settings without an expected literal (presence-only) are skipped — there
/// is no value to write.
pub fn build_settings_patch(missing: &[MissingSetting]) -> anyhow::Result<String> {
    let entries = missing
        .iter()
        .filter_map(|setting| {
            setting
                .expected_literal
                .clone()
                .map(|literal| (setting.path.clone(), literal))
        })
        .collect::<Vec<_>>();
    build_object_source(&entries)
}
