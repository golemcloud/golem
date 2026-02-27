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

use super::BTreeMapDiff;
use crate::model::diff::{hash_from_serialized_value, Diffable, Hash, Hashable};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeploymentAgentOptions {
    pub security_scheme: Option<String>,
    pub test_session_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeploymentAgentOptionsDiff {
    pub security_scheme_changed: bool,
    pub test_session_header_changed: bool,
}

impl Diffable for HttpApiDeploymentAgentOptions {
    type DiffResult = HttpApiDeploymentAgentOptionsDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let security_scheme_changed = new.security_scheme != current.security_scheme;
        let test_session_header_changed = new.test_session_header != current.test_session_header;

        if security_scheme_changed || test_session_header_changed {
            Some(HttpApiDeploymentAgentOptionsDiff {
                security_scheme_changed,
                test_session_header_changed,
            })
        } else {
            None
        }
    }
}

// TODO; peal off non-user things from mcp-deployment
//
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeployment {
    pub webhooks_url: String,
    pub agents: BTreeMap<String, HttpApiDeploymentAgentOptions>,
}

impl Hashable for HttpApiDeployment {
    fn hash(&self) -> Hash {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeploymentDiff {
    pub webhooks_url_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents_changes: BTreeMapDiff<String, HttpApiDeploymentAgentOptions>,
}

impl Diffable for HttpApiDeployment {
    type DiffResult = HttpApiDeploymentDiff;

    fn diff(new: &Self, current: &Self) -> Option<Self::DiffResult> {
        let webhooks_url_changed = new.webhooks_url != current.webhooks_url;
        let agents_changes = new
            .agents
            .diff_with_current(&current.agents)
            .unwrap_or_default();
        if webhooks_url_changed || !agents_changes.is_empty() {
            Some(Self::DiffResult {
                webhooks_url_changed,
                agents_changes,
            })
        } else {
            None
        }
    }
}
