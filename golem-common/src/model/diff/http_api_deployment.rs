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

use crate::model::diff::{hash_from_serialized_value, BTreeSetDiff, Diffable, Hash, Hashable};
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt::Display;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HttpApiDeploymentTarget {
    host: String,
    subdomain: Option<String>,
}

pub static NO_SUBDOMAIN: Option<&str> = None;

impl<Subdomain: Into<String>, Host: Into<String>> From<(Option<Subdomain>, Host)>
    for HttpApiDeploymentTarget
{
    fn from(value: (Option<Subdomain>, Host)) -> Self {
        HttpApiDeploymentTarget {
            host: value.1.into(),
            subdomain: value.0.map(|v| v.into()),
        }
    }
}

impl Serialize for HttpApiDeploymentTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl From<&HttpApiDeploymentTarget> for String {
    fn from(value: &HttpApiDeploymentTarget) -> Self {
        match &value.subdomain {
            Some(subdomain) => {
                format!("{}.{}", subdomain, value.host)
            }
            None => value.host.to_string(),
        }
    }
}

impl Display for HttpApiDeploymentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(self))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HttpApiDeployment {
    pub apis: BTreeSet<String>,
}

impl Hashable for HttpApiDeployment {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

impl Diffable for HttpApiDeployment {
    type DiffResult = BTreeSetDiff<String>;

    fn diff(local: &Self, server: &Self) -> Option<Self::DiffResult> {
        local.apis.diff_with_server(&server.apis)
    }
}
