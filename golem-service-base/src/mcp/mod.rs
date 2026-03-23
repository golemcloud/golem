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
// .

mod protobuf;

use golem_common::base_model::account::AccountId;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::Domain;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::base_model::security_scheme::SecuritySchemeName;
use golem_common::model::component::{ComponentId, ComponentRevision};
use std::collections::HashMap;

use crate::custom_api::SecuritySchemeDetails;

pub type AgentTypeImplementers = HashMap<AgentTypeName, (ComponentId, ComponentRevision)>;

#[derive(Clone)]
pub struct CompiledMcp {
    pub account_id: AccountId,
    pub environment_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub domain: Domain,
    pub agent_type_implementers: AgentTypeImplementers,
    pub security_scheme: Option<SecuritySchemeDetails>,
    pub security_scheme_name: Option<SecuritySchemeName>,
    pub registered_agent_types: Vec<RegisteredAgentType>,
}

impl CompiledMcp {
    pub fn agent_types(&self) -> Vec<AgentTypeName> {
        self.agent_type_implementers.keys().cloned().collect()
    }
}
