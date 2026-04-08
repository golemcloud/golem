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

use crate::services::deployment::route_compilation::render_http_method;
use golem_common::SafeDisplay;
use golem_common::model::agent::{AgentTypeName, HttpMethod};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::component::ComponentName;
use golem_common::model::domain_registration::Domain;
use golem_common::model::quota::ResourceName;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_service_base::custom_api::PathSegment;
use golem_wasm::analysis::AnalysedType;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DeployValidationError {
    #[error(
        "Agent type {missing_agent_type} requested by http api deployment {http_api_deployment_domain} is not part of the deployment"
    )]
    HttpApiDeploymentMissingAgentType {
        http_api_deployment_domain: Domain,
        missing_agent_type: AgentTypeName,
    },
    #[error(
        "Agent type {missing_agent_type} requested by mcp deployment {mcp_deployment_domain} is not part of the deployment"
    )]
    McpDeploymentMissingAgentType {
        mcp_deployment_domain: Domain,
        missing_agent_type: AgentTypeName,
    },
    #[error("Invalid path pattern: {0}")]
    HttpApiDefinitionInvalidPathPattern(String),
    #[error("Invalid http cors binding expression: {0}")]
    InvalidHttpCorsBindingExpr(String),
    #[error("Component {0} not found in deployment")]
    ComponentNotFound(ComponentName),
    #[error("No security scheme configured for agent {0} but agent has methods that require auth")]
    NoSecuritySchemeConfigured(AgentTypeName),
    #[error(
        "MCP deployment {mcp_deployment_domain} has conflicting security schemes across agents"
    )]
    McpDeploymentConflictingSecuritySchemes { mcp_deployment_domain: Domain },
    #[error(
        "MCP deployment {mcp_deployment_domain} references unknown security scheme {security_scheme}"
    )]
    McpDeploymentUnknownSecurityScheme {
        mcp_deployment_domain: Domain,
        security_scheme: SecuritySchemeName,
    },
    #[error(
        "Method {agent_method} of agent {agent_type} used by http api at {method} {domain}/{path} is invalid: {error}"
    )]
    HttpApiDeploymentAgentMethodInvalid {
        domain: Domain,
        method: String,
        path: String,
        agent_type: AgentTypeName,
        agent_method: String,
        error: String,
    },
    #[error(
        "Method constructor of agent {agent_type} mounted by by http api at {domain}/{path} is invalid: {error}"
    )]
    HttpApiDeploymentAgentConstructorInvalid {
        domain: Domain,
        path: String,
        agent_type: AgentTypeName,
        error: String,
    },
    #[error(
        "Agent type {agent_type} is deployed to multiple domains. An agent type can only be deployed to one domain at a time"
    )]
    HttpApiDeploymentMultipleDeploymentsForAgentType { agent_type: AgentTypeName },
    #[error("Agent type {agent_type} is deployed to a domain but does not have http mount details")]
    HttpApiDeploymentAgentTypeMissingHttpMount { agent_type: AgentTypeName },
    #[error(
        "Agent type {agent_type} uses forbidden patterns in its webhook. Variable and catchall segments are not allowed in webhook urls"
    )]
    HttpApiDeploymentInvalidAgentWebhookSegmentType { agent_type: AgentTypeName },
    #[error(
        "Agent type {agent_type} has an invalid final webhook url {url}. (Protocol is a placeholder)"
    )]
    HttpApiDeploymentInvalidWebhookUrl {
        agent_type: AgentTypeName,
        url: String,
    },
    #[error("Overriding security scheme is only allowed if the environment level option is set")]
    SecurityOverrideDisabled,
    #[error("Http api for domain {domain} has multiple routes for pattern {rendered_method} {rendered_path}", rendered_method = render_http_method(method), rendered_path = itertools::join(path.iter().map(|p| p.to_string()), "/"))]
    RouteIsAmbiguous {
        domain: Domain,
        method: HttpMethod,
        path: Vec<PathSegment>,
    },
    #[error("Invalid http method: {method:?}")]
    InvalidHttpMethod { method: HttpMethod },
    #[error("Agent type name {0} is provided by multiple components")]
    AmbiguousAgentTypeName(AgentTypeName),
    #[error(
        "Agent type names '{name1}' and '{name2}' conflict: both normalize to '{normalized}' in kebab-case"
    )]
    ConflictingAgentTypeNames {
        name1: AgentTypeName,
        name2: AgentTypeName,
        normalized: String,
    },
    #[error(
        "Secret default at key {path} has the wrong type: [{rendered_errors}]",
        rendered_errors = errors.join(", ")
    )]
    AgentSecretDefaultTypeMismatch {
        path: CanonicalAgentSecretPath,
        errors: Vec<String>,
    },
    #[error(
        "Agent secret at path {path} is not compatible with existing secret in the environment. agent: {agent_secret_type:?}; environment: {environment_secret_type:?}"
    )]
    AgentSecretNotCompatibleWithEnvironmentSecret {
        path: CanonicalAgentSecretPath,
        agent_secret_type: AnalysedType,
        environment_secret_type: AnalysedType,
    },
    #[error("Agent secret at path {path} has different type across deployed agents")]
    AgentSecretTypeConflict { path: CanonicalAgentSecretPath },
    #[error("Multiple resource definitions for the name: {name}")]
    ConflictingResourceDefinitions { name: ResourceName },
}

impl SafeDisplay for DeployValidationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

pub fn format_validation_errors(errors: &[DeployValidationError]) -> String {
    errors
        .iter()
        .map(|err| format!("{err}"))
        .collect::<Vec<_>>()
        .join(",\n")
}
