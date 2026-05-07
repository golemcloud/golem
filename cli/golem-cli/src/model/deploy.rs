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

use crate::agent_id_display::{SourceLanguage, render_type_for_language};
use crate::command::shared_args::{ForceBuildArg, PostDeployArgs};
use crate::model::GuestLanguage;
use crate::model::component::{render_agent_constructor, render_data_schema};
use crate::model::text::component::is_sensitive_env_var_name;
use crate::model::worker::RawAgentId;
use golem_common::model::agent::{
    AgentConfigSource, AgentMethod, AgentType, HttpEndpointDetails, HttpMethod, HttpMountDetails,
    PathSegment, Snapshotting,
};
use golem_common::model::component::{AgentFilePermissions, ComponentName, ComponentRevision};
use golem_common::model::diff::{self, Hashable};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplay {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, DeploymentDisplayComponent>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub http_api_deployments: BTreeMap<String, DeploymentDisplayHttpApiDeployment>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_deployments: BTreeMap<String, DeploymentDisplayMcpDeployment>,
}

pub struct DeploymentDisplayContext<'a> {
    pub show_sensitive: bool,
    pub mode: DeploymentDisplayMode,
    pub deployment: &'a diff::Deployment,
    pub diff: &'a diff::DeploymentDiff,
    pub agent_types_by_component: &'a HashMap<String, Vec<AgentType>>,
}

#[derive(Clone, Copy, Debug)]
pub enum DeploymentDisplayMode {
    ChangedOnly,
    Full,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayComponent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_hash: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayAgentType>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayAgentType {
    pub constructor: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source_language: String,
    pub mode: String,
    pub snapshotting: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_declarations: BTreeMap<String, DeploymentDisplayConfigDeclaration>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub config_defaults: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, DeploymentDisplayAgentFile>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: BTreeMap<String, DeploymentDisplayPlugin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_mount: Option<DeploymentDisplayHttpMount>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub methods: BTreeMap<String, DeploymentDisplayMethod>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayConfigDeclaration {
    pub source: String,
    pub value_type: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayAgentFile {
    pub permissions: String,
    pub hash: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayPlugin {
    pub priority: i32,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpMount {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<String>,
    pub phantom_agent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMethod {
    pub signature: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_hint: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub http: Vec<DeploymentDisplayHttpEndpoint>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpEndpoint {
    pub method: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cors: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpApiDeployment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub webhooks_prefix: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub openapi_endpoint_prefix: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayHttpApiAgentOptions>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayHttpApiAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_session_header: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMcpDeployment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, DeploymentDisplayMcpAgentOptions>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentDisplayMcpAgentOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_scheme: Option<String>,
}

impl DeploymentDisplay {
    pub fn from_context(ctx: DeploymentDisplayContext<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            components: display_components(&ctx)?,
            http_api_deployments: display_http_api_deployments(&ctx),
            mcp_deployments: display_mcp_deployments(&ctx),
        })
    }

    pub fn unified_yaml_diff_with_current(&self, current: &Self) -> anyhow::Result<String> {
        Ok(diff::unified_diff(
            current.to_yaml_for_diff()?,
            self.to_yaml_for_diff()?,
        ))
    }

    pub fn unified_yaml_diff_with_current_full_context(
        &self,
        current: &Self,
    ) -> anyhow::Result<String> {
        Ok(diff::unified_diff_with_context(
            current.to_yaml_for_diff()?,
            self.to_yaml_for_diff()?,
            usize::MAX,
        ))
    }

    fn to_yaml_for_diff(&self) -> anyhow::Result<String> {
        if self.is_empty() {
            Ok(String::new())
        } else {
            Ok(serde_yaml::to_string(self)?)
        }
    }

    fn is_empty(&self) -> bool {
        self.components.is_empty()
            && self.http_api_deployments.is_empty()
            && self.mcp_deployments.is_empty()
    }
}

fn display_components(
    ctx: &DeploymentDisplayContext<'_>,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayComponent>> {
    display_keys(ctx.mode, &ctx.deployment.components, &ctx.diff.components)
        .filter(|component_name| ctx.deployment.components.contains_key(*component_name))
        .map(|component_name| {
            let agent_types = ctx
                .agent_types_by_component
                .get(component_name)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let component = ctx
                .deployment
                .components
                .get(component_name)
                .and_then(|component| component.as_value());

            let binary_hash = component.map(|component| component.wasm_hash.to_string());

            let agents = agent_types
                .iter()
                .sorted_by_key(|agent| &agent.type_name.0)
                .map(|agent| {
                    Ok((
                        agent.type_name.0.clone(),
                        display_agent_type(ctx.show_sensitive, agent, component)?,
                    ))
                })
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

            Ok((
                component_name.clone(),
                DeploymentDisplayComponent {
                    binary_hash,
                    agents,
                },
            ))
        })
        .collect()
}

fn display_agent_type(
    show_sensitive: bool,
    agent: &AgentType,
    component: Option<&diff::Component>,
) -> anyhow::Result<DeploymentDisplayAgentType> {
    let lang = SourceLanguage::from(agent.source_language.as_str());
    let provision_config = component
        .and_then(|component| {
            component
                .agent_type_provision_configs
                .get(&agent.type_name.0)
        })
        .and_then(|config| config.as_value());

    Ok(DeploymentDisplayAgentType {
        constructor: render_agent_constructor(agent, false, false),
        description: agent.description.clone(),
        source_language: agent.source_language.clone(),
        mode: format!("{:?}", agent.mode),
        snapshotting: render_snapshotting(&agent.snapshotting),
        config_declarations: display_config_declarations(agent)?,
        config_defaults: display_config_defaults(show_sensitive, agent, provision_config)?,
        env: provision_config
            .map(|config| display_env(show_sensitive, &config.env))
            .unwrap_or_default(),
        files: provision_config
            .map(display_files)
            .transpose()?
            .unwrap_or_default(),
        plugins: provision_config
            .map(|config| display_plugins(show_sensitive, config))
            .unwrap_or_default(),
        http_mount: agent.http_mount.as_ref().map(display_http_mount),
        methods: agent
            .methods
            .iter()
            .map(|method| (method.name.clone(), display_method(&lang, method)))
            .collect(),
        dependencies: agent
            .dependencies
            .iter()
            .map(|dependency| dependency.type_name.clone())
            .collect(),
    })
}

fn display_config_declarations(
    agent: &AgentType,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayConfigDeclaration>> {
    let lang = SourceLanguage::from(agent.source_language.as_str());

    agent
        .config
        .iter()
        .map(|config| {
            Ok((
                config.path.join("."),
                DeploymentDisplayConfigDeclaration {
                    source: render_agent_config_source(config.source).to_string(),
                    value_type: render_type_for_language(&lang, &config.value_type, true),
                },
            ))
        })
        .collect()
}

fn display_config_defaults(
    show_sensitive: bool,
    agent: &AgentType,
    provision_config: Option<&diff::AgentTypeProvisionConfig>,
) -> anyhow::Result<BTreeMap<String, serde_json::Value>> {
    let provision_values = provision_config
        .map(|config| &config.config)
        .into_iter()
        .flatten()
        .collect::<BTreeMap<_, _>>();

    let mut result = BTreeMap::new();

    for (path, value) in &provision_values {
        let declaration = agent
            .config
            .iter()
            .find(|config| config.path.join(".") == path.as_str());
        let is_secret =
            declaration.is_some_and(|config| config.source == AgentConfigSource::Secret);

        let rendered_value = if is_secret && !show_sensitive {
            masked_json_value(value)?
        } else {
            serde_json::to_value(value)?
        };

        result.insert((*path).clone(), rendered_value);
    }

    Ok(result)
}

fn display_env(show_sensitive: bool, env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    env.iter()
        .map(|(key, value)| {
            (
                key.clone(),
                mask_sensitive_value(show_sensitive, key, value),
            )
        })
        .collect()
}

fn display_files(
    provision_config: &diff::AgentTypeProvisionConfig,
) -> anyhow::Result<BTreeMap<String, DeploymentDisplayAgentFile>> {
    provision_config
        .files_by_path
        .iter()
        .filter_map(|(path, file)| file.as_value().map(|file| (path, file)))
        .map(|(path, file)| {
            Ok((
                path.clone(),
                DeploymentDisplayAgentFile {
                    permissions: display_agent_file_permissions(file.permissions).to_string(),
                    hash: file.hash.to_string(),
                },
            ))
        })
        .collect()
}

fn display_plugins(
    show_sensitive: bool,
    provision_config: &diff::AgentTypeProvisionConfig,
) -> BTreeMap<String, DeploymentDisplayPlugin> {
    provision_config
        .plugins_by_grant_id
        .values()
        .map(|plugin| {
            let key = format!("{}@{}", plugin.name, plugin.version);
            let parameters = plugin
                .parameters
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        mask_sensitive_value(show_sensitive, key, value),
                    )
                })
                .collect();

            (
                key,
                DeploymentDisplayPlugin {
                    priority: plugin.priority,
                    parameters,
                },
            )
        })
        .collect()
}

fn display_method(lang: &SourceLanguage, method: &AgentMethod) -> DeploymentDisplayMethod {
    let output = render_data_schema(&method.output_schema, lang, false);
    let signature = if output.is_empty() {
        format!(
            "{}({})",
            method.name,
            render_data_schema(&method.input_schema, lang, true)
        )
    } else {
        format!(
            "{}({}) -> {}",
            method.name,
            render_data_schema(&method.input_schema, lang, true),
            output
        )
    };

    DeploymentDisplayMethod {
        signature,
        description: method.description.clone(),
        prompt_hint: method.prompt_hint.clone(),
        http: method
            .http_endpoint
            .iter()
            .map(display_http_endpoint)
            .collect(),
    }
}

fn display_http_mount(http_mount: &HttpMountDetails) -> DeploymentDisplayHttpMount {
    DeploymentDisplayHttpMount {
        path: render_path(&http_mount.path_prefix),
        webhook: (!http_mount.webhook_suffix.is_empty())
            .then(|| render_path(&http_mount.webhook_suffix)),
        phantom_agent: http_mount.phantom_agent,
        auth_required: http_mount.auth_details.as_ref().map(|auth| auth.required),
        cors: http_mount.cors_options.allowed_patterns.clone(),
    }
}

fn display_http_endpoint(endpoint: &HttpEndpointDetails) -> DeploymentDisplayHttpEndpoint {
    DeploymentDisplayHttpEndpoint {
        method: render_http_method(&endpoint.http_method).to_string(),
        path: render_path(&endpoint.path_suffix),
        auth_required: endpoint.auth_details.as_ref().map(|auth| auth.required),
        headers: endpoint
            .header_vars
            .iter()
            .map(|header| (header.header_name.clone(), header.variable_name.clone()))
            .collect(),
        query: endpoint
            .query_vars
            .iter()
            .map(|query| (query.query_param_name.clone(), query.variable_name.clone()))
            .collect(),
        cors: endpoint.cors_options.allowed_patterns.clone(),
    }
}

fn display_http_api_deployments(
    ctx: &DeploymentDisplayContext<'_>,
) -> BTreeMap<String, DeploymentDisplayHttpApiDeployment> {
    display_keys(
        ctx.mode,
        &ctx.deployment.http_api_deployments,
        &ctx.diff.http_api_deployments,
    )
    .filter_map(|domain| {
        let hash = ctx
            .deployment
            .http_api_deployments
            .get(domain)
            .and_then(|deployment| deployment.hash().ok())
            .map(|hash| hash.to_string());
        ctx.deployment
            .http_api_deployments
            .get(domain)
            .and_then(|deployment| deployment.as_value())
            .map(|deployment| {
                (
                    domain.clone(),
                    DeploymentDisplayHttpApiDeployment {
                        hash: hash.clone(),
                        webhooks_prefix: deployment.webhooks_prefix.clone(),
                        openapi_endpoint_prefix: deployment.openapi_endpoint_prefix.clone(),
                        agents: deployment
                            .agents
                            .iter()
                            .map(|(agent, options)| {
                                (
                                    agent.clone(),
                                    DeploymentDisplayHttpApiAgentOptions {
                                        security_scheme: options.security_scheme.clone(),
                                        test_session_header: options.test_session_header.clone(),
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .or_else(|| {
                hash.map(|hash| {
                    (
                        domain.clone(),
                        DeploymentDisplayHttpApiDeployment {
                            hash: Some(hash),
                            webhooks_prefix: String::new(),
                            openapi_endpoint_prefix: String::new(),
                            agents: BTreeMap::new(),
                        },
                    )
                })
            })
    })
    .collect()
}

fn display_mcp_deployments(
    ctx: &DeploymentDisplayContext<'_>,
) -> BTreeMap<String, DeploymentDisplayMcpDeployment> {
    display_keys(
        ctx.mode,
        &ctx.deployment.mcp_deployments,
        &ctx.diff.mcp_deployments,
    )
    .filter_map(|domain| {
        let hash = ctx
            .deployment
            .mcp_deployments
            .get(domain)
            .and_then(|deployment| deployment.hash().ok())
            .map(|hash| hash.to_string());
        ctx.deployment
            .mcp_deployments
            .get(domain)
            .and_then(|deployment| deployment.as_value())
            .map(|deployment| {
                (
                    domain.clone(),
                    DeploymentDisplayMcpDeployment {
                        hash: hash.clone(),
                        agents: deployment
                            .agents
                            .iter()
                            .map(|(agent, options)| {
                                (
                                    agent.clone(),
                                    DeploymentDisplayMcpAgentOptions {
                                        security_scheme: options.security_scheme.clone(),
                                    },
                                )
                            })
                            .collect(),
                    },
                )
            })
            .or_else(|| {
                hash.map(|hash| {
                    (
                        domain.clone(),
                        DeploymentDisplayMcpDeployment {
                            hash: Some(hash),
                            agents: BTreeMap::new(),
                        },
                    )
                })
            })
    })
    .collect()
}

fn display_keys<'a, V>(
    mode: DeploymentDisplayMode,
    deployment: &'a BTreeMap<String, V>,
    diff: &'a BTreeMap<String, diff::BTreeMapDiffValue<<V as diff::Diffable>::DiffResult>>,
) -> Box<dyn Iterator<Item = &'a String> + 'a>
where
    V: diff::Diffable,
{
    match mode {
        DeploymentDisplayMode::ChangedOnly => Box::new(diff.keys()),
        DeploymentDisplayMode::Full => Box::new(deployment.keys()),
    }
}

fn display_agent_file_permissions(permissions: AgentFilePermissions) -> &'static str {
    match permissions {
        AgentFilePermissions::ReadOnly => "readonly",
        AgentFilePermissions::ReadWrite => "read-write",
    }
}

fn render_path(segments: &[PathSegment]) -> String {
    if segments.is_empty() {
        return "/".to_string();
    }

    format!("/{}", segments.iter().map(render_path_segment).join("/"))
}

fn render_path_segment(segment: &PathSegment) -> String {
    match segment {
        PathSegment::Literal(segment) => segment.value.clone(),
        PathSegment::SystemVariable(segment) => format!("{{{}}}", segment.value),
        PathSegment::PathVariable(segment) => format!("{{{}}}", segment.variable_name),
        PathSegment::RemainingPathVariable(segment) => format!("{{*{}}}", segment.variable_name),
    }
}

fn render_http_method(method: &HttpMethod) -> &str {
    match method {
        HttpMethod::Get(_) => "GET",
        HttpMethod::Head(_) => "HEAD",
        HttpMethod::Post(_) => "POST",
        HttpMethod::Put(_) => "PUT",
        HttpMethod::Delete(_) => "DELETE",
        HttpMethod::Connect(_) => "CONNECT",
        HttpMethod::Options(_) => "OPTIONS",
        HttpMethod::Trace(_) => "TRACE",
        HttpMethod::Patch(_) => "PATCH",
        HttpMethod::Custom(method) => &method.value,
    }
}

fn render_snapshotting(snapshotting: &Snapshotting) -> String {
    match snapshotting {
        Snapshotting::Disabled(_) => "disabled".to_string(),
        Snapshotting::Enabled(config) => format!("enabled {:?}", config),
    }
}

fn render_agent_config_source(source: AgentConfigSource) -> &'static str {
    match source {
        AgentConfigSource::Local => "local",
        AgentConfigSource::Secret => "secret",
    }
}

fn mask_sensitive_value(show_sensitive: bool, key: &str, value: &str) -> String {
    if !show_sensitive && is_sensitive_env_var_name(show_sensitive, key) {
        masked_secret(value)
    } else {
        value.to_string()
    }
}

fn masked_json_value(value: &impl Serialize) -> anyhow::Result<serde_json::Value> {
    Ok(serde_json::Value::String(masked_secret(
        &serde_json::to_string(value)?,
    )))
}

fn masked_secret(value: &str) -> String {
    format!(
        "<masked-secret:{}>",
        blake3::hash(value.as_bytes()).to_hex()
    )
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TryUpdateAllWorkersResult {
    pub triggered: Vec<WorkerUpdateAttempt>,
    pub failed: Vec<WorkerUpdateAttempt>,
}

impl TryUpdateAllWorkersResult {
    pub fn extend(&mut self, other: TryUpdateAllWorkersResult) {
        self.triggered.extend(other.triggered);
        self.failed.extend(other.failed);
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerUpdateAttempt {
    pub component_name: ComponentName,
    pub target_revision: ComponentRevision,
    pub agent_name: RawAgentId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeployConfig {
    pub plan: bool,
    pub stage: bool,
    pub approve_staging_steps: bool,
    pub show_full_deployment: bool,
    pub force_build: Option<ForceBuildArg>,
    pub post_deploy_args: PostDeployArgs,
    pub repl_bridge_sdk_target: Option<GuestLanguage>,
    pub skip_build: bool,
}

pub enum DeploySummary {
    PlanOk,
    PlanUpToDate,
    StagingOk, // Only for internal testing purposes
    DeployOk(PostDeployResult),
    DeployUpToDate(PostDeployResult),
    RollbackOk(PostDeployResult),
    RollbackUpToDate(PostDeployResult),
}

#[derive(Error, Debug)]
pub enum DeployError {
    #[error("Cancelled")]
    Cancelled,
    #[error("Build error: {0}")]
    BuildError(anyhow::Error),
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Plan error: {0}")]
    PlanError(anyhow::Error),
    #[error("Environment check error: {0}")]
    EnvironmentCheckError(anyhow::Error),
    #[error("Staging error: {0}")]
    StagingError(anyhow::Error),
    #[error("Deploy error: {0}")]
    DeployError(anyhow::Error),
    #[error("Rollback error: {0}")]
    RollbackError(anyhow::Error),
}

pub type DeployResult = Result<DeploySummary, DeployError>;

pub enum PostDeploySummary {
    NoRequestedChanges,
    NoDeployment,
    AgentUpdateOk,
    AgentRedeployOk,
    AgentDeleteOk,
}

#[derive(Error, Debug)]
pub enum PostDeployError {
    #[error("Prepare error: {0}")]
    PrepareError(anyhow::Error),
    #[error("Agent update error: {0}")]
    AgentUpdateError(anyhow::Error),
    #[error("Agent redeploy error: {0}")]
    AgentRedeployError(anyhow::Error),
    #[error("Agent delete error: {0}")]
    AgentDeleteError(anyhow::Error),
}

pub type PostDeployResult = Result<PostDeploySummary, PostDeployError>;
