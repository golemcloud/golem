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

use crate::model::diff::{hash_from_serialized_value, BTreeMapDiff, Diffable, Hash, Hashable};
use crate::model::GatewayBindingType;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpApiMethodAndPath {
    pub method: String,
    pub path: String,
}

impl PartialOrd<Self> for HttpApiMethodAndPath {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HttpApiMethodAndPath {
    fn cmp(&self, other: &Self) -> Ordering {
        // NOTE: we order first by path, then by method, so in diffs the same paths are "grouped"
        match self.path.cmp(&other.path) {
            Ordering::Less => Ordering::Less,
            Ordering::Equal => self.method.cmp(&other.method),
            Ordering::Greater => Ordering::Greater,
        }
    }
}

impl<Method: Into<String>, Path: Into<String>> From<(Method, Path)> for HttpApiMethodAndPath {
    fn from(value: (Method, Path)) -> Self {
        HttpApiMethodAndPath {
            method: value.0.into(),
            path: value.1.into(),
        }
    }
}

impl Serialize for HttpApiMethodAndPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<&HttpApiMethodAndPath> for String {
    fn from(value: &HttpApiMethodAndPath) -> Self {
        format!("{} {}", value.method, value.path)
    }
}

impl Display for HttpApiMethodAndPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(self))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDefinitionBinding {
    pub binding_type: Option<GatewayBindingType>,
    pub component_name: Option<String>,
    pub worker_name: Option<String>,
    pub idempotency_key: Option<String>,
    pub response: Option<String>,
}

impl Hashable for HttpApiDefinitionBinding {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiRoute {
    pub binding: HttpApiDefinitionBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
}

impl Hashable for HttpApiRoute {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiRouteDiff {
    pub binding_changed: bool,
    pub security_changed: bool,
}

impl Diffable for HttpApiRoute {
    type DiffResult = HttpApiRouteDiff;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        if local.hash() == server.hash() {
            return None;
        }

        Some(HttpApiRouteDiff {
            binding_changed: local.binding != server.binding,
            security_changed: local.security != server.security,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDefinition {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub routes: BTreeMap<HttpApiMethodAndPath, HttpApiRoute>,
    pub version: String,
}

impl Hashable for HttpApiDefinition {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDefinitionDiff {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    routes: BTreeMapDiff<HttpApiMethodAndPath, HttpApiRoute>,
    version_changed: bool,
}

impl Diffable for HttpApiDefinition {
    type DiffResult = HttpApiDefinitionDiff;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        if local.hash() == server.hash() {
            return None;
        }

        Some(HttpApiDefinitionDiff {
            routes: local
                .routes
                .diff_with_server(&server.routes)
                .unwrap_or_default(),
            version_changed: local.version != server.version,
        })
    }
}
