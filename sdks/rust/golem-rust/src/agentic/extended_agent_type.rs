// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::AutoInjectedParamType;
use crate::golem_agentic::golem::agent::common::{
    AgentConstructor, AgentDependency, AgentMethod, AgentMode, AgentType, DataSchema,
    ElementSchema, HttpEndpointDetails, HttpMountDetails, Snapshotting,
};
use std::collections::HashSet;

// An enriched representation is a different view of the agent type that will include
// extra details such as auto-injected schemas such as Principal
// The agent registry will hold this information for the generated `initiate` , `invoke` and rpc calls
// to look up so that these parameters can be injected automatically by the platform
#[derive(Clone)]
pub struct ExtendedAgentType {
    pub type_name: String,
    pub description: String,
    pub constructor: ExtendedAgentConstructor,
    pub methods: Vec<EnrichedAgentMethod>,
    pub dependencies: Vec<AgentDependency>,
    pub mode: AgentMode,
    pub http_mount: Option<HttpMountDetails>,
    pub snapshotting: Snapshotting,
}

impl ExtendedAgentType {
    pub fn principal_params_in_constructor(&self) -> HashSet<String> {
        let mut principal_params = HashSet::new();

        if let ExtendedDataSchema::Tuple(fields) = &self.constructor.input_schema {
            for (name, schema) in fields {
                if let EnrichedElementSchema::AutoInject(AutoInjectedParamType::Principal) = schema
                {
                    principal_params.insert(name.clone());
                }
            }
        }

        principal_params
    }

    pub fn to_agent_type(&self) -> AgentType {
        AgentType {
            type_name: self.type_name.clone(),
            description: self.description.clone(),
            constructor: self.constructor.to_agent_constructor(),
            methods: self.methods.iter().map(|m| m.to_agent_method()).collect(),
            dependencies: self.dependencies.clone(),
            mode: self.mode,
            http_mount: self.http_mount.clone(),
            snapshotting: self.snapshotting.clone(),
            config: Vec::new()
        }
    }
}

#[derive(Clone)]
pub struct EnrichedAgentMethod {
    pub name: String,
    pub description: String,
    pub http_endpoint: Vec<HttpEndpointDetails>,
    pub prompt_hint: Option<String>,
    pub input_schema: ExtendedDataSchema,
    pub output_schema: ExtendedDataSchema,
}

impl EnrichedAgentMethod {
    pub fn to_agent_method(&self) -> AgentMethod {
        AgentMethod {
            name: self.name.clone(),
            description: self.description.clone(),
            http_endpoint: self.http_endpoint.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema: self.input_schema.to_data_schema(),
            output_schema: self.output_schema.to_data_schema(),
        }
    }
}

#[derive(Clone)]
pub struct ExtendedAgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: ExtendedDataSchema,
}

impl ExtendedAgentConstructor {
    pub fn to_agent_constructor(&self) -> AgentConstructor {
        AgentConstructor {
            name: self.name.clone(),
            description: self.description.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema: self.input_schema.to_data_schema(),
        }
    }
}

#[derive(Clone)]
pub enum ExtendedDataSchema {
    Tuple(Vec<(String, EnrichedElementSchema)>),
    // Disallow any auto-injected schemas within multimodal
    // This simplifies the implementation as multimodal is not expected to have anything outside
    // such as Principal
    Multimodal(Vec<(String, ElementSchema)>),
}

impl ExtendedDataSchema {
    pub fn to_data_schema(&self) -> DataSchema {
        match self {
            ExtendedDataSchema::Tuple(fields) => {
                let fields_without_auto_injected = fields
                    .iter()
                    .filter_map(|(name, schema)| match schema {
                        EnrichedElementSchema::ElementSchema(element_schema) => {
                            Some((name.clone(), element_schema.clone()))
                        }
                        EnrichedElementSchema::AutoInject(_) => None,
                    })
                    .collect::<Vec<(String, ElementSchema)>>();

                DataSchema::Tuple(fields_without_auto_injected)
            }
            ExtendedDataSchema::Multimodal(variants) => DataSchema::Multimodal(variants.clone()),
        }
    }
}

#[derive(Clone)]
pub enum EnrichedElementSchema {
    ElementSchema(ElementSchema),
    AutoInject(AutoInjectedParamType),
}
