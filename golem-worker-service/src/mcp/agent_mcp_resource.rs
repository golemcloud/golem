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
use std::collections::HashMap;

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

pub struct ConstructorParam {
    pub name: String,
    pub value: String,
}

pub struct McpResourceUri {
    pub agent: String,
    pub method: String,
    pub tail_segments: Vec<String>,
}

impl McpResourceUri {
    pub fn parse(uri: &str) -> Result<Self, String> {
        let rest = uri
            .strip_prefix("golem://")
            .ok_or_else(|| format!("URI must start with golem://, got: {uri}"))?;

        let (agent, path) = rest
            .split_once('/')
            .ok_or_else(|| format!("URI must contain agent and method: {uri}"))?;

        if agent.is_empty() {
            return Err(format!("URI agent name cannot be empty: {uri}"));
        }

        let mut segments: Vec<String> = path
            .split('/')
            .filter(|s| !s.is_empty())
            .map(percent_decode)
            .collect();

        if segments.is_empty() {
            return Err(format!("URI must contain a method name: {uri}"));
        }

        let method = segments.remove(0);

        Ok(McpResourceUri {
            agent: agent.to_string(),
            method,
            tail_segments: segments,
        })
    }
}

fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let hi = chars.next();
            let lo = chars.next();
            if let (Some(hi), Some(lo)) = (hi, lo)
                && let Ok(byte) = u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16)
            {
                result.push(byte as char);
                continue;
            }
            result.push('%');
        } else {
            result.push(b as char);
        }
    }
    result
}

type StaticResourceUri = String;

#[derive(Hash, Eq, PartialEq)]
struct AgentMethodKey {
    agent_type_name: String,
    method_name: String,
}

#[derive(Default)]
pub struct ResourceRegistry {
    static_resources: HashMap<StaticResourceUri, AgentMcpResource>,
    template_resources: HashMap<AgentMethodKey, AgentMcpResource>,
}

impl ResourceRegistry {
    pub fn insert(&mut self, resource: AgentMcpResource) {
        match &resource.kind {
            AgentMcpResourceKind::Static(res) => {
                self.static_resources.insert(res.uri.clone(), resource);
            }
            AgentMcpResourceKind::Template { .. } => {
                let key = AgentMethodKey {
                    agent_type_name: resource.agent_type_name.0.clone(),
                    method_name: resource.raw_method.name.clone(),
                };
                self.template_resources.insert(key, resource);
            }
        }
    }

    pub fn get_static(&self, uri: &str) -> Option<&AgentMcpResource> {
        self.static_resources.get(uri)
    }

    pub fn list_static_resources(&self) -> Vec<Resource> {
        self.static_resources
            .values()
            .filter_map(|r| match &r.kind {
                AgentMcpResourceKind::Static(resource) => Some(resource.clone()),
                AgentMcpResourceKind::Template { .. } => None,
            })
            .collect()
    }

    pub fn list_resource_templates(&self) -> Vec<ResourceTemplate> {
        self.template_resources
            .values()
            .filter_map(|r| match &r.kind {
                AgentMcpResourceKind::Template { template, .. } => Some(template.clone()),
                AgentMcpResourceKind::Static(_) => None,
            })
            .collect()
    }

    pub fn extract_mcp_resource_with_input(
        &self,
        uri: &McpResourceUri,
    ) -> Option<(&AgentMcpResource, Vec<ConstructorParam>)> {
        let key = AgentMethodKey {
            agent_type_name: uri.agent.clone(),
            method_name: uri.method.clone(),
        };
        let resource = self.template_resources.get(&key)?;

        if let AgentMcpResourceKind::Template {
            constructor_param_names,
            ..
        } = &resource.kind
            && uri.tail_segments.len() == constructor_param_names.len()
        {
            let params = constructor_param_names
                .iter()
                .zip(uri.tail_segments.iter())
                .map(|(name, value)| ConstructorParam {
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect();
            return Some((resource, params));
        }
        None
    }
}

impl AgentMcpResource {
    pub fn resource_name(agent_type_name: &AgentTypeName, method: &AgentMethod) -> String {
        format!("{}-{}", agent_type_name.0, method.name)
    }

    pub fn static_uri(agent_type_name: &AgentTypeName, method: &AgentMethod) -> String {
        // https://modelcontextprotocol.info/docs/concepts/resources
        // The protocol and path structure is defined by the MCP server implementation.
        // Servers can define their own custom URI schemes.
        format!("golem://{}/{}", agent_type_name.0, method.name)
    }

    pub fn template_uri(
        agent_type_name: &AgentTypeName,
        method: &AgentMethod,
        param_names: &[String],
    ) -> String {
        // https://modelcontextprotocol.info/docs/concepts/resources
        // The protocol and path structure is defined by the MCP server implementation.
        // Servers can define their own custom URI schemes.
        let base = format!("golem://{}/{}", agent_type_name.0, method.name);
        let placeholders: Vec<String> = param_names.iter().map(|n| format!("{{{}}}", n)).collect();
        format!("{}/{}", base, placeholders.join("/"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_parse_static_uri() {
        let uri = McpResourceUri::parse("golem://counter/get-value").unwrap();
        assert_eq!(uri.agent, "counter");
        assert_eq!(uri.method, "get-value");
        assert!(uri.tail_segments.is_empty());
    }

    #[test]
    fn test_parse_template_uri_with_params() {
        let uri = McpResourceUri::parse("golem://counter/get-value/my-counter").unwrap();
        assert_eq!(uri.agent, "counter");
        assert_eq!(uri.method, "get-value");
        assert_eq!(uri.tail_segments, vec!["my-counter"]);
    }

    #[test]
    fn test_parse_template_uri_with_multiple_params() {
        let uri = McpResourceUri::parse("golem://counter/get-value/ns/my-counter").unwrap();
        assert_eq!(uri.agent, "counter");
        assert_eq!(uri.method, "get-value");
        assert_eq!(uri.tail_segments, vec!["ns", "my-counter"]);
    }

    #[test]
    fn test_parse_percent_encoded_uri() {
        let uri = McpResourceUri::parse("golem://counter/get-value/my%20counter").unwrap();
        assert_eq!(uri.tail_segments, vec!["my counter"]);
    }

    #[test]
    fn test_parse_uri_trailing_slash() {
        let uri = McpResourceUri::parse("golem://counter/get-value/").unwrap();
        assert_eq!(uri.agent, "counter");
        assert_eq!(uri.method, "get-value");
        assert!(uri.tail_segments.is_empty());
    }

    #[test]
    fn test_parse_invalid_scheme() {
        assert!(McpResourceUri::parse("http://counter/get-value").is_err());
    }

    #[test]
    fn test_parse_missing_method() {
        assert!(McpResourceUri::parse("golem://counter/").is_err());
    }

    #[test]
    fn test_parse_empty_agent() {
        assert!(McpResourceUri::parse("golem:///get-value").is_err());
    }
}
