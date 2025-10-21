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
use crate::model::diff::http_api_definition::HttpApiDefinition;
use crate::model::diff::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentTarget};
use crate::model::diff::ser::serialize_with_mode;
use crate::model::diff::{BTreeMapDiff, Diffable};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub components: BTreeMap<String, HashOf<Component>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub http_api_definitions: BTreeMap<String, HashOf<HttpApiDefinition>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    #[serde(serialize_with = "serialize_with_mode")]
    pub http_api_deployments: BTreeMap<HttpApiDeploymentTarget, HashOf<HttpApiDeployment>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    components: BTreeMapDiff<String, HashOf<Component>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    http_api_definitions: BTreeMapDiff<String, HashOf<HttpApiDefinition>>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    http_api_deployments: BTreeMapDiff<HttpApiDeploymentTarget, HashOf<HttpApiDeployment>>,
}

impl Diffable for Deployment {
    type DiffResult = DeploymentDiff;

    fn diff(local: &Self, remote: &Self) -> Option<Self::DiffResult> {
        let components = local.components.diff_with_server(&remote.components);
        let http_api_definitions = local
            .http_api_definitions
            .diff_with_server(&remote.http_api_definitions);
        let http_api_deployments = local
            .http_api_deployments
            .diff_with_server(&remote.http_api_deployments);

        if components.is_some() || http_api_definitions.is_some() || http_api_deployments.is_some()
        {
            Some(DeploymentDiff {
                components: components.unwrap_or_default(),
                http_api_definitions: http_api_definitions.unwrap_or_default(),
                http_api_deployments: http_api_deployments.unwrap_or_default(),
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
