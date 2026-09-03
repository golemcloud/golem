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
use golem_common::model::tool::ToolName;
use golem_common::schema::graph::SchemaGraph;
use golem_service_base::custom_api::PathSegment;

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
        "Http api deployment {domain} contains and invalid route {rendered_path} (protocol is a placeholder): {error}",
        rendered_path = itertools::join(path.iter().map(|p| p.to_string()), "/")
    )]
    HttpApiDeploymentInvalidRoute {
        domain: Domain,
        path: Vec<PathSegment>,
        error: String,
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
        "Agent secret config at path {path} must be declared as secret<T> or option<secret<T>> with plaintext T"
    )]
    AgentSecretInvalidConfigType { path: CanonicalAgentSecretPath },
    #[error(
        "Agent secret at path {path} is not compatible with existing secret in the environment. agent: {agent_secret_type:?}; environment: {environment_secret_type:?}"
    )]
    AgentSecretNotCompatibleWithEnvironmentSecret {
        path: CanonicalAgentSecretPath,
        agent_secret_type: Box<SchemaGraph>,
        environment_secret_type: Box<SchemaGraph>,
    },
    #[error("Agent secret at path {path} has different type across deployed agents")]
    AgentSecretTypeConflict { path: CanonicalAgentSecretPath },
    #[error("Multiple resource definitions for the name: {name}")]
    ConflictingResourceDefinitions { name: ResourceName },
    #[error("Multiple retry policy defaults with the same name: {name}")]
    ConflictingRetryPolicyDefaults { name: String },
    #[error(
        "Reset override flags are only allowed when environment compatibility_check is disabled"
    )]
    ResetOverrideRequiresCompatibilityCheckDisabled,
    #[error(
        "Component {component_name} contains tools but does not export golem:tool/guest@0.1.0 (found {found:?})"
    )]
    ToolUnsupportedGuestExport {
        component_name: ComponentName,
        found: Option<String>,
    },
    #[error(
        "Tool {tool_name} in component {component_name} has definition name {definition_name:?}"
    )]
    ToolDefinitionNameMismatch {
        component_name: ComponentName,
        tool_name: ToolName,
        definition_name: Option<String>,
    },
    #[error(
        "Tool {tool_name} in component {component_name} is invalid: {errors}",
        errors = errors.join(", ")
    )]
    InvalidTool {
        component_name: ComponentName,
        tool_name: ToolName,
        errors: Vec<String>,
    },
    #[error("Tool {tool_name} in component {component_name} could not be serialized: {error}")]
    ToolMetadataSerialization {
        component_name: ComponentName,
        tool_name: ToolName,
        error: String,
    },
    #[error(
        "Tool {tool_name} is implemented by multiple components: {components}",
        components = components.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )]
    DuplicateToolImplementation {
        tool_name: ToolName,
        components: Vec<ComponentName>,
    },
    #[error(
        "Tool {tool_name} has multiple local or registry sources: {sources}",
        sources = sources.join(", ")
    )]
    ToolSourceCollision {
        tool_name: ToolName,
        sources: Vec<String>,
    },
    #[error("Registry tool {tool_name} is unavailable in this environment")]
    RemoteToolUnavailable { tool_name: ToolName },
    #[error("Registry tool declaration {tool_name} selected a release for tool {release_name}")]
    RemoteToolNameMismatch {
        tool_name: ToolName,
        release_name: ToolName,
    },
    #[error("Registry tool {tool_name} release definition has root name {definition_name:?}")]
    RemoteToolDefinitionNameMismatch {
        tool_name: ToolName,
        definition_name: Option<String>,
    },
    #[error(
        "Registry tool {tool_name} release version {release_version} does not match its definition version {definition_version}"
    )]
    RemoteToolVersionMismatch {
        tool_name: ToolName,
        release_version: String,
        definition_version: String,
    },
    #[error(
        "Registry tool {tool_name} uses unsupported metadata schema version {metadata_version}"
    )]
    RemoteToolUnsupportedMetadataVersion {
        tool_name: ToolName,
        metadata_version: String,
    },
    #[error("Registry tool {tool_name} has an invalid metadata digest")]
    RemoteToolMetadataDigestMismatch { tool_name: ToolName },
    #[error(
        "Registry tool {tool_name} is invalid: {errors}",
        errors = errors.join(", ")
    )]
    InvalidRemoteTool {
        tool_name: ToolName,
        errors: Vec<String>,
    },
    #[error("Registry tool {tool_name} has a binding for unknown agent type {agent_type}")]
    RemoteToolBindingUnknownAgent {
        tool_name: ToolName,
        agent_type: AgentTypeName,
    },
    #[error(
        "Tool {tool_name} in component {component_name} has a binding for unknown agent type {agent_type}"
    )]
    ToolBindingUnknownAgent {
        component_name: ComponentName,
        tool_name: ToolName,
        agent_type: AgentTypeName,
    },
    #[error(
        "Tool {tool_name} binding{agent} requests version {requested_version}, but the deployed tool version is {tool_version}",
        agent = agent_type.as_ref().map(|name| format!(" for agent {name}")).unwrap_or_default()
    )]
    ToolBindingVersionMismatch {
        tool_name: ToolName,
        agent_type: Option<AgentTypeName>,
        requested_version: String,
        tool_version: String,
    },
    #[error(
        "Tool {tool_name} binding{agent} requests account {requested_account}, but the implementing component is owned by {owner_account}",
        agent = agent_type.as_ref().map(|name| format!(" for agent {name}")).unwrap_or_default()
    )]
    ToolBindingAccountMismatch {
        tool_name: ToolName,
        agent_type: Option<AgentTypeName>,
        requested_account: String,
        owner_account: String,
    },
    #[error(
        "Tool {tool_name} binding{agent} parameters must be a JSON object",
        agent = agent_type.as_ref().map(|name| format!(" for agent {name}")).unwrap_or_default()
    )]
    ToolBindingParametersMustBeObject {
        tool_name: ToolName,
        agent_type: Option<AgentTypeName>,
    },
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
