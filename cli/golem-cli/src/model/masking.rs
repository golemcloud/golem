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

use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::worker::{AgentConfigEntryDto, TypedAgentConfigEntry};
use golem_wasm::analysis::analysed_type;
use golem_wasm::{Value, ValueAndType};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaskingConfig {
    pub show_secrets: bool,
}

impl MaskingConfig {
    pub fn new(show_secrets: bool) -> Self {
        Self { show_secrets }
    }

    pub fn show_secrets() -> Self {
        Self { show_secrets: true }
    }

    pub fn hide_secrets() -> Self {
        Self {
            show_secrets: false,
        }
    }
}

pub trait Masked: Sized {
    fn masked(self, _config: MaskingConfig) -> anyhow::Result<Self> {
        Ok(self)
    }
}

pub const SECRET_MASK: &str = "***";

const SENSITIVE_KEY_PATTERNS: &[&str] = &[
    "CREDENTIAL",
    "CREDENTIALS",
    "KEY",
    "PASS",
    "PASSWORD",
    "PWD",
    "SECRET",
    "TOKEN",
];

pub fn is_sensitive_key(name: &str) -> bool {
    let name = name.to_uppercase();
    SENSITIVE_KEY_PATTERNS
        .iter()
        .any(|pattern| name.contains(pattern))
}

pub fn mask_secret() -> String {
    SECRET_MASK.to_string()
}

pub fn mask_known_secret_value(config: MaskingConfig, value: &str) -> String {
    if config.show_secrets {
        value.to_string()
    } else {
        mask_secret()
    }
}

pub fn mask_sensitive_key_value(config: MaskingConfig, key: &str, value: &str) -> String {
    if !config.show_secrets && is_sensitive_key(key) {
        mask_secret()
    } else {
        value.to_string()
    }
}

pub fn mask_sensitive_map<'a, M>(
    config: MaskingConfig,
    values: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> M
where
    M: FromIterator<(String, String)>,
{
    values
        .into_iter()
        .map(|(key, value)| (key.clone(), mask_sensitive_key_value(config, key, value)))
        .collect()
}

pub fn mask_agent_config_entries<'a>(
    config: MaskingConfig,
    values: impl IntoIterator<Item = &'a AgentConfigEntryDto>,
    secret_paths: &BTreeSet<String>,
) -> Vec<AgentConfigEntryDto> {
    values
        .into_iter()
        .map(|entry| {
            let mut entry = entry.clone();
            if should_mask_config_path(config, &entry.path, secret_paths) {
                entry.value = NormalizedJsonValue(serde_json::Value::String(mask_secret()));
            }
            entry
        })
        .collect()
}

pub fn mask_typed_agent_config_entries<'a>(
    config: MaskingConfig,
    values: impl IntoIterator<Item = &'a TypedAgentConfigEntry>,
    secret_paths: &BTreeSet<String>,
) -> Vec<TypedAgentConfigEntry> {
    values
        .into_iter()
        .map(|entry| {
            let mut entry = entry.clone();
            if should_mask_config_path(config, &entry.path, secret_paths) {
                entry.value = masked_typed_secret_value();
            }
            entry
        })
        .collect()
}

fn should_mask_config_path(
    config: MaskingConfig,
    path: &[String],
    secret_paths: &BTreeSet<String>,
) -> bool {
    !config.show_secrets && secret_paths.contains(&path.join("."))
}

fn masked_typed_secret_value() -> ValueAndType {
    ValueAndType::new(Value::String(mask_secret()), analysed_type::str())
}

pub fn mask_json_secret_value(
    config: MaskingConfig,
    value: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    if config.show_secrets {
        value.clone()
    } else {
        value
            .as_ref()
            .map(|_| serde_json::Value::String(mask_secret()))
    }
}

pub fn mask_json_secret_value_or_null(
    config: MaskingConfig,
    value: &Option<serde_json::Value>,
) -> serde_json::Value {
    mask_json_secret_value(config, value).unwrap_or(serde_json::Value::Null)
}

pub fn mask_json_secret_with_fingerprint(
    value: &impl Serialize,
) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::String(mask_secret_with_fingerprint(
        &serde_json::to_string(value)?,
    )))
}

pub fn mask_secret_with_fingerprint(value: &str) -> String {
    format!(
        "<masked-secret:{}>",
        blake3::hash(value.as_bytes()).to_hex()
    )
}

pub fn mask_json_secret_for_deploy_diff(
    config: MaskingConfig,
    value: &impl Serialize,
) -> anyhow::Result<serde_json::Value> {
    if config.show_secrets {
        Ok(serde_json::to_value(value)?)
    } else {
        mask_json_secret_with_fingerprint(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::worker::{AgentConfigEntryDto, TypedAgentConfigEntry};
    use golem_wasm::json::ValueAndTypeJsonExtensions;
    use serde_json::json;
    use std::collections::BTreeSet;
    use test_r::test;

    #[test]
    fn sensitive_key_detection_is_case_insensitive() {
        assert!(is_sensitive_key("db_password"));
        assert!(is_sensitive_key("ApiToken"));
        assert!(!is_sensitive_key("regular_name"));
    }

    #[test]
    fn sensitive_key_values_are_masked_by_default() {
        let masked = mask_sensitive_key_value(MaskingConfig::hide_secrets(), "API_TOKEN", "abc");
        assert_eq!(masked, "***");

        let visible = mask_sensitive_key_value(MaskingConfig::show_secrets(), "API_TOKEN", "abc");
        assert_eq!(visible, "abc");
    }

    #[test]
    fn deploy_diff_secret_mask_is_stable_and_not_plaintext() {
        let first =
            mask_json_secret_for_deploy_diff(MaskingConfig::hide_secrets(), &json!("abc")).unwrap();
        let second =
            mask_json_secret_for_deploy_diff(MaskingConfig::hide_secrets(), &json!("abc")).unwrap();

        assert_eq!(first, second);
        assert!(first.as_str().unwrap().starts_with("<masked-secret:"));
        assert!(!first.to_string().contains("abc"));
    }

    #[test]
    fn agent_config_entries_are_masked_by_secret_path() {
        let entries = vec![AgentConfigEntryDto {
            path: vec!["regular".to_string()],
            value: NormalizedJsonValue(json!("secret")),
        }];
        let secret_paths = BTreeSet::from_iter(["regular".to_string()]);

        let masked =
            mask_agent_config_entries(MaskingConfig::hide_secrets(), &entries, &secret_paths);
        assert_eq!(masked[0].value.0, json!("***"));

        let visible =
            mask_agent_config_entries(MaskingConfig::show_secrets(), &entries, &secret_paths);
        assert_eq!(visible[0].value.0, json!("secret"));
    }

    #[test]
    fn agent_config_entries_do_not_use_sensitive_key_heuristics() {
        let entries = vec![AgentConfigEntryDto {
            path: vec!["apiToken".to_string()],
            value: NormalizedJsonValue(json!("not-a-declared-secret")),
        }];

        let masked =
            mask_agent_config_entries(MaskingConfig::hide_secrets(), &entries, &BTreeSet::new());

        assert_eq!(masked[0].value.0, json!("not-a-declared-secret"));
    }

    #[test]
    fn typed_agent_config_mask_is_valid_typed_string_value() {
        let entries = vec![TypedAgentConfigEntry {
            path: vec!["token".to_string()],
            value: ValueAndType::new(Value::Bool(true), analysed_type::bool()),
        }];
        let secret_paths = BTreeSet::from_iter(["token".to_string()]);

        let masked =
            mask_typed_agent_config_entries(MaskingConfig::hide_secrets(), &entries, &secret_paths);

        assert_eq!(masked[0].value.to_json_value().unwrap(), json!("***"));
        assert_eq!(masked[0].value.typ, analysed_type::str());
    }
}
