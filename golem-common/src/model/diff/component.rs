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

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        let content_changed = local.hash != remote.hash;
        let permissions_changed = local.permissions != remote.permissions;

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
pub struct ComponentWasmRpcTarget {
    pub interface_name: String,
    pub component_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dynamic_linking_wasm_rpc: BTreeMap<String, BTreeMap<String, ComponentWasmRpcTarget>>,
    // TODO: atomic: agents? or should consider that part of the wasm binary?
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
    pub plugins_by_priority: BTreeMap<String, PluginInstallation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentDiff {
    pub binary_changed: bool,
    pub metadata_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub file_changes: BTreeMapDiff<String, HashOf<ComponentFile>>,
    pub plugins_changed: bool,
}

impl Diffable for Component {
    type DiffResult = ComponentDiff;

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        let update_metadata = local.metadata != remote.metadata;
        let update_binary = local.wasm_hash != remote.wasm_hash;
        let file_changes = local.files_by_path.diff_with_server(&remote.files_by_path);
        let plugins_changed = local.plugins_by_priority == remote.plugins_by_priority;

        if update_metadata || update_binary || file_changes.is_some() || plugins_changed {
            Some(ComponentDiff {
                metadata_changed: update_metadata,
                binary_changed: update_binary,
                file_changes: file_changes.unwrap_or_default(),
                plugins_changed,
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
