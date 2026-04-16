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

use crate::base_model::json::NormalizedJsonValue;
use crate::model::component::AgentFilePermissions;
use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::plugin::PluginInstallation;
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFile {
    pub hash: Hash,
    pub permissions: AgentFilePermissions,
}

impl Hashable for AgentFile {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileDiff {
    pub content_changed: bool,
    pub permissions_changed: bool,
}

impl Diffable for AgentFile {
    type DiffResult = AgentFileDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let content_changed = new.hash != current.hash;
        let permissions_changed = new.permissions != current.permissions;

        if content_changed || permissions_changed {
            Some(AgentFileDiff {
                content_changed,
                permissions_changed,
            })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeProvisionConfig {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub wasi_config: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, NormalizedJsonValue>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub files_by_path: BTreeMap<String, HashOf<AgentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins_by_grant_id: BTreeMap<Uuid, PluginInstallation>,
}

impl Hashable for AgentTypeProvisionConfig {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTypeProvisionConfigDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env_changes: BTreeMapDiff<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub wasi_config_changes: BTreeMapDiff<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_changes: BTreeMapDiff<String, HashOf<AgentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_changes: BTreeMapDiff<Uuid, PluginInstallation>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_changes: BTreeMapDiff<String, NormalizedJsonValue>,
}

impl Diffable for AgentTypeProvisionConfig {
    type DiffResult = AgentTypeProvisionConfigDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let env_changes = new.env.diff_with_current(&current.env).unwrap_or_default();
        let wasi_config_changes = new
            .wasi_config
            .diff_with_current(&current.wasi_config)
            .unwrap_or_default();
        let file_changes = new
            .files_by_path
            .diff_with_current(&current.files_by_path)
            .unwrap_or_default();
        let plugin_changes = new
            .plugins_by_grant_id
            .diff_with_current(&current.plugins_by_grant_id)
            .unwrap_or_default();
        let config_changes = new
            .config
            .diff_with_current(&current.config)
            .unwrap_or_default();

        if !env_changes.is_empty()
            || !wasi_config_changes.is_empty()
            || !file_changes.is_empty()
            || !plugin_changes.is_empty()
            || !config_changes.is_empty()
        {
            Some(AgentTypeProvisionConfigDiff {
                env_changes,
                wasi_config_changes,
                file_changes,
                plugin_changes,
                config_changes,
            })
        } else {
            None
        }
    }
}
