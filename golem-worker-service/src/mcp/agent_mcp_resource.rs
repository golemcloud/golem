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

use golem_common::base_model::account::AccountId;
use golem_common::base_model::agent::{
    AgentMethod, AgentTypeName, DataSchema, NamedElementSchemas,
};
use golem_common::base_model::component::ComponentId;
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::AgentConstructor;
use rmcp::model::{Resource, ResourceTemplate};

pub type ResourceUri = String;

#[derive(Clone)]
pub struct AgentMcpResource {
    pub kind: AgentMcpResourceKind,
    pub environment_id: EnvironmentId,
    pub account_id: AccountId,
    pub constructor: AgentConstructor,
    pub raw_method: AgentMethod,
    pub component_id: ComponentId,
    pub agent_type_name: AgentTypeName,
}

#[derive(Clone)]
pub enum AgentMcpResourceKind {
    Static(Resource),
    Template {
        template: ResourceTemplate,
        constructor_param_names: Vec<String>,
    },
}

impl AgentMcpResource {
    pub fn resource_name(agent_type_name: &AgentTypeName, method: &AgentMethod) -> String {
        format!("{}-{}", agent_type_name.0, method.name)
    }

    pub fn static_uri(agent_type_name: &AgentTypeName, method: &AgentMethod) -> String {
        format!("golem://{}/{}", agent_type_name.0, method.name)
    }

    pub fn template_uri(
        agent_type_name: &AgentTypeName,
        method: &AgentMethod,
        param_names: &[String],
    ) -> String {
        let base = format!("golem://{}/{}", agent_type_name.0, method.name);
        let placeholders: Vec<String> = param_names.iter().map(|n| format!("{{{}}}", n)).collect();
        format!("{}/{}", base, placeholders.join("/"))
    }

    pub fn extract_params_from_uri(
        template_uri: &str,
        concrete_uri: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let template_parts: Vec<&str> = template_uri.split('/').collect();
        let concrete_parts: Vec<&str> = concrete_uri.split('/').collect();

        if template_parts.len() != concrete_parts.len() {
            return Err(format!(
                "URI segment count mismatch: template has {}, concrete has {}",
                template_parts.len(),
                concrete_parts.len()
            ));
        }

        let mut params = Vec::new();
        for (tmpl, conc) in template_parts.iter().zip(concrete_parts.iter()) {
            if tmpl.starts_with('{') && tmpl.ends_with('}') {
                let name = tmpl[1..tmpl.len() - 1].to_string();
                params.push((name, conc.to_string()));
            } else if tmpl != conc {
                return Err(format!(
                    "URI segment mismatch: expected '{}', got '{}'",
                    tmpl, conc
                ));
            }
        }

        Ok(params)
    }

    pub fn constructor_param_names(constructor: &AgentConstructor) -> Vec<String> {
        match &constructor.input_schema {
            DataSchema::Tuple(NamedElementSchemas { elements }) => {
                elements.iter().map(|e| e.name.clone()).collect()
            }
            DataSchema::Multimodal(_) => vec![],
        }
    }
}
