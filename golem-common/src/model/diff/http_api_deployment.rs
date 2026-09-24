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

use super::BTreeMapDiff;
use crate::model::diff::{DiffError, Diffable, Hash, Hashable, hash_from_serialized_value};
use crate::model::http_api_deployment::HttpApiDeploymentScheme;
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

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let security_scheme_changed = new.security_scheme != current.security_scheme;
        let test_session_header_changed = new.test_session_header != current.test_session_header;

        Ok(if security_scheme_changed || test_session_header_changed {
            Some(HttpApiDeploymentAgentOptionsDiff {
                security_scheme_changed,
                test_session_header_changed,
            })
        } else {
            None
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeployment {
    pub scheme: HttpApiDeploymentScheme,
    pub webhooks_prefix: String,
    pub openapi_endpoint_prefix: String,
    pub agents: BTreeMap<String, HttpApiDeploymentAgentOptions>,
}

impl Hashable for HttpApiDeployment {
    fn hash(&self) -> Result<Hash, DiffError> {
        hash_from_serialized_value(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpApiDeploymentDiff {
    pub scheme_changed: bool,
    pub webhooks_url_changed: bool,
    pub openapi_endpoint_changed: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents_changes: BTreeMapDiff<String, HttpApiDeploymentAgentOptions>,
}

impl Diffable for HttpApiDeployment {
    type DiffResult = HttpApiDeploymentDiff;

    fn diff(new: &Self, current: &Self) -> Result<Option<Self::DiffResult>, DiffError> {
        let scheme_changed = new.scheme != current.scheme;
        let webhooks_url_changed = new.webhooks_prefix != current.webhooks_prefix;
        let openapi_endpoint_changed =
            new.openapi_endpoint_prefix != current.openapi_endpoint_prefix;
        let agents_changes = new
            .agents
            .diff_with_current(&current.agents)?
            .unwrap_or_default();
        Ok(
            if scheme_changed
                || webhooks_url_changed
                || openapi_endpoint_changed
                || !agents_changes.is_empty()
            {
                Some(Self::DiffResult {
                    scheme_changed,
                    webhooks_url_changed,
                    openapi_endpoint_changed,
                    agents_changes,
                })
            } else {
                None
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn scheme_only_change_affects_deployment_hash_and_diff() {
        let https = HttpApiDeployment {
            scheme: HttpApiDeploymentScheme::Https,
            webhooks_prefix: "/webhooks/".into(),
            openapi_endpoint_prefix: "/docs/".into(),
            agents: BTreeMap::new(),
        };
        let http = HttpApiDeployment {
            scheme: HttpApiDeploymentScheme::Http,
            ..https.clone()
        };
        assert_ne!(https.hash().unwrap(), http.hash().unwrap());
        let diff = HttpApiDeployment::diff(&http, &https).unwrap().unwrap();
        assert!(diff.scheme_changed);
        assert!(!diff.webhooks_url_changed);
        assert!(!diff.openapi_endpoint_changed);
        assert!(diff.agents_changes.is_empty());
        assert!(HttpApiDeployment::diff(&http, &http).unwrap().is_none());
    }
}
