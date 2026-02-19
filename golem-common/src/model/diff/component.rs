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

use crate::model::component::ComponentFilePermissions;
use crate::model::diff::hash::{hash_from_serialized_value, Hash, HashOf, Hashable};
use crate::model::diff::plugin::PluginInstallation;
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentFile {
    pub hash: Hash,
    pub permissions: ComponentFilePermissions,
}

impl Hashable for ComponentFile {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentFileDiff {
    pub content_changed: bool,
    pub permissions_changed: bool,
}

impl Diffable for ComponentFile {
    type DiffResult = ComponentFileDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let content_changed = new.hash != current.hash;
        let permissions_changed = new.permissions != current.permissions;

        if content_changed || permissions_changed {
            Some(ComponentFileDiff {
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
pub struct ComponentMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl Hashable for ComponentMetadata {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    #[serde(serialize_with = "serialize_with_mode")]
    pub metadata: HashOf<ComponentMetadata>,
    pub wasm_hash: Hash,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub files_by_path: BTreeMap<String, HashOf<ComponentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins_by_grant_id: BTreeMap<Uuid, PluginInstallation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentDiff {
    pub wasm_changed: bool,
    pub metadata_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_changes: BTreeMapDiff<String, HashOf<ComponentFile>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugin_changes: BTreeMapDiff<Uuid, PluginInstallation>,
}

impl Diffable for Component {
    type DiffResult = ComponentDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let metadata_changed = new.metadata != current.metadata;
        let wasm_changed = new.wasm_hash != current.wasm_hash;
        let file_changes = new
            .files_by_path
            .diff_with_current(&current.files_by_path)
            .unwrap_or_default();
        let plugin_changes = new
            .plugins_by_grant_id
            .diff_with_current(&current.plugins_by_grant_id)
            .unwrap_or_default();

        if metadata_changed
            || wasm_changed
            || !file_changes.is_empty()
            || !plugin_changes.is_empty()
        {
            Some(ComponentDiff {
                metadata_changed,
                wasm_changed,
                file_changes,
                plugin_changes,
            })
        } else {
            None
        }
    }
}

impl Hashable for Component {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}
