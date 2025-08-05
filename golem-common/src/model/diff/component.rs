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

use crate::model::diff::hash::{Hash, HashOf, Hashable, hash_from_serialized_value};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use crate::model::{ComponentFilePermissions, ComponentType};
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
pub struct ComponentMetadata {
    pub version: Option<String>,
    pub component_type: ComponentType,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dynamic_linking_wasm_rpc: BTreeMap<String, BTreeMap<String, String>>,
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
    pub binary_hash: Hash,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub files: BTreeMap<String, HashOf<ComponentFile>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentDiff {
    binary_changed: bool,
    metadata_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    file_changes: BTreeMapDiff<String, HashOf<ComponentFile>>,
}

impl Diffable for Component {
    type DiffResult = ComponentDiff;

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        let update_metadata = local.metadata != remote.metadata;
        let update_binary = local.binary_hash != remote.binary_hash;
        let files_diff = local.files.diff_with_server(&remote.files);

        if update_metadata || update_binary || files_diff.is_some() {
            Some(ComponentDiff {
                metadata_changed: update_metadata,
                binary_changed: update_binary,
                file_changes: files_diff.unwrap_or_default(),
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
