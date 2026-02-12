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

pub use crate::base_model::http_api_deployment::*;
use crate::declare_structs;
use crate::model::agent::AgentTypeName;
use crate::model::diff;
use std::collections::BTreeMap;

impl HttpApiDeploymentAgentOptions {
    pub fn to_diffable(&self) -> diff::HttpApiDeploymentAgentOptions {
        let mut security_scheme = None;
        let mut test_session_header = None;

        match &self.security {
            None => {}
            Some(HttpApiDeploymentAgentSecurity::TestSessionHeader(inner)) => {
                test_session_header = Some(inner.header_name.clone());
            }
            Some(HttpApiDeploymentAgentSecurity::SecurityScheme(inner)) => {
                security_scheme = Some(inner.security_scheme.0.clone());
            }
        }

        diff::HttpApiDeploymentAgentOptions {
            security_scheme,
            test_session_header,
        }
    }
}

impl HttpApiDeployment {
    pub fn to_diffable(&self) -> diff::HttpApiDeployment {
        diff::HttpApiDeployment {
            webhooks_url: self.webhooks_url.clone(),
            agents: self
                .agents
                .iter()
                .map(|(k, v)| (k.0.clone(), v.to_diffable()))
                .collect(),
        }
    }
}

declare_structs! {
    pub struct HttpApiDeploymentUpdate {
        pub current_revision: HttpApiDeploymentRevision,
        pub webhook_url: Option<String>,
        pub agents: Option<BTreeMap<AgentTypeName, HttpApiDeploymentAgentOptions>>
    }
}
