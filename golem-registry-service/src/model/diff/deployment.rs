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

use crate::model::diff::component::Component;
use crate::model::diff::hash::{hash_from_serialized_value, Hash, HashOf, Hashable};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeDiff, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub components: BTreeMap<String, HashOf<Component>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    components: BTreeDiff<String, HashOf<Component>>,
}

impl Diffable for Deployment {
    type DiffResult = DeploymentDiff;

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        let components_diff = local.components.diff_with_remote(&remote.components);

        if components_diff.is_some() {
            Some(DeploymentDiff {
                components: components_diff.unwrap_or_default(),
            })
        } else {
            None
        }
    }
}

impl Hashable for Deployment {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}
