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

use super::DeployValidationError;
use super::DeploymentWriteError;
use super::http_parameter_conversion::build_http_agent_constructor_parameters;
use super::ok_or_continue;
use super::route_compilation::{
    add_agent_method_http_routes, add_cors_preflight_http_routes, add_openapi_spec_routes,
    add_webhook_callback_routes, build_agent_http_api_deployment_details, compile_fallback_mount,
    make_invalid_agent_mount_error_maker,
};
use crate::model::agent_secret::{
    DeploymentAgentSecretCreation, DeploymentAgentSecretReplacement, DeploymentAgentSecretUpdate,
};
use crate::model::api_definition::UnboundCompiledRoute;
use crate::repo::model::deployment::CompiledTools;
use crate::repo::model::retry_policy::RetryPolicyCreationRecord;
use crate::services::agent_secret::schema_contains_host_managed_capability;
use crate::services::deployment::route_compilation::validate_path_segments;
use crate::services::environment_tool_grant::ResolvedGrantedToolRelease;
use crate::services::environment_tool_middleware_grant::ResolvedGrantedToolMiddlewareRelease;
use golem_common::base_model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{
    AgentConfigSource, AgentTypeName, DeployedRegisteredAgentType, RegisteredAgentTypeImplementer,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{DeploymentAgentSecretDefault, DeploymentRetryPolicyDefault};
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::mcp_import::McpImport;
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionCreation, ResourceName};
use golem_common::model::retry_policy::RetryPolicyId;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, RemoteToolDeployment, TOOL_METADATA_WIT_VERSION,
    ToolBindingInput, ToolBindingOwner, ToolDeploymentMetadata, ToolName, ToolSource,
};
use golem_common::model::tool_middleware::{
    RegisteredToolMiddleware, RemoteToolMiddlewareDeployment, TOOL_MIDDLEWARE_METADATA_WIT_VERSION,
    ToolMiddlewareName, ToolMiddlewareSource,
};
use golem_common::model::tool_middleware_release::{
    ToolMiddlewareReleaseSource, tool_middleware_metadata_digest,
};
use golem_common::model::tool_release::ToolReleaseId;
use golem_common::schema::agent::reachable_defs;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::tool::validation::validate_tool;
use golem_common::schema::validation::is_equivalent_cross_graph;
use golem_common::schema::{AgentTypeSchema, RegisteredAgentTypeSchema};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::Component;
use golem_service_base::model::retry_policy::StoredRetryPolicy;
use heck::ToKebabCase;
use std::collections::{BTreeMap, HashMap, HashSet, hash_map};

#[derive(Debug)]
pub struct InProgressDeployedRegisteredAgentType {
    pub agent_type: AgentTypeSchema,
    pub implemented_by: RegisteredAgentTypeImplementer,
    pub webhook_domain_and_segments: Option<(Domain, Vec<String>)>,
}

impl From<InProgressDeployedRegisteredAgentType> for DeployedRegisteredAgentType {
    fn from(value: InProgressDeployedRegisteredAgentType) -> Self {
        Self {
            agent_type: value.agent_type,
            implemented_by: value.implemented_by,
            webhook_prefix_authority_and_path: value
                .webhook_domain_and_segments
                .map(|(domain, segments)| format!("{}/{}", domain.0, segments.join("/"))),
        }
    }
}

#[derive(Debug)]
pub struct DeploymentContext {
    pub environment: Environment,
    pub components: BTreeMap<ComponentName, Component>,
    pub http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
    pub mcp_deployments: BTreeMap<Domain, golem_common::model::mcp_deployment::McpDeployment>,
    pub registered_agent_types: HashMap<AgentTypeName, InProgressDeployedRegisteredAgentType>,
}

impl DeploymentContext {
    pub fn collect_tool_middleware_registrations(
        &self,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        remote: &[(
            RemoteToolMiddlewareDeployment,
            Option<ResolvedGrantedToolMiddlewareRelease>,
        )],
        errors: &mut Vec<DeployValidationError>,
    ) -> Vec<RegisteredToolMiddleware> {
        let mut registrations = BTreeMap::<ToolMiddlewareName, RegisteredToolMiddleware>::new();
        for component in self.components.values() {
            let export_valid = component
                .metadata
                .known_exports()
                .tool_middleware_guest_interface
                .as_deref()
                == Some("golem:tool/tool-middleware-guest@0.1.0");
            for (name, metadata) in component.metadata.tool_middlewares() {
                let valid = export_valid
                    && metadata.definition.name == name.as_str()
                    && golem_common::schema::tool::validation::validate_tool_middleware(
                        &metadata.definition,
                    )
                    .is_ok();
                if !valid {
                    errors.push(DeployValidationError::ToolMiddleware {
                        middleware_name: Some(name.clone()),
                        agent_type_name: None,
                        tool_name: None,
                        message:
                            "invalid descriptor, name, or missing tool-middleware guest export"
                                .to_string(),
                    });
                    continue;
                }
                let registration = RegisteredToolMiddleware {
                    deployment_revision,
                    release_id: None,
                    definition: metadata.definition.clone(),
                    provision: metadata.provision.clone(),
                    source: ToolMiddlewareSource::Component {
                        component_id: component.id,
                        component_revision: component.revision,
                        component_name: component.component_name.clone(),
                    },
                    owner_account_id: component.account_id,
                    owner_account_email: component.account_email.clone(),
                    metadata_version: TOOL_MIDDLEWARE_METADATA_WIT_VERSION.to_string(),
                    metadata_digest: tool_middleware_metadata_digest(
                        TOOL_MIDDLEWARE_METADATA_WIT_VERSION,
                        &metadata.definition,
                    )
                    .unwrap_or_default(),
                };
                if registrations.insert(name.clone(), registration).is_some() {
                    errors.push(DeployValidationError::ToolMiddleware {
                        middleware_name: Some(name.clone()),
                        agent_type_name: None,
                        tool_name: None,
                        message: "multiple local or remote implementations".to_string(),
                    });
                }
            }
        }
        for (deployment, resolved) in remote {
            let Some(resolved) = resolved else {
                errors.push(DeployValidationError::ToolMiddleware {
                    middleware_name: Some(deployment.name.clone()),
                    agent_type_name: None,
                    tool_name: None,
                    message: "remote release is unavailable in this environment".to_string(),
                });
                continue;
            };
            let release = &resolved.release;
            let valid = release.name == deployment.name
                && release.definition.name == deployment.name.as_str()
                && release.version == release.definition.version
                && tool_middleware_metadata_digest(&release.metadata_version, &release.definition)
                    .is_ok_and(|digest| digest == release.metadata_digest);
            if !valid {
                errors.push(DeployValidationError::ToolMiddleware {
                    middleware_name: Some(deployment.name.clone()),
                    agent_type_name: None,
                    tool_name: None,
                    message: "remote release identity, version, or digest is invalid".to_string(),
                });
                continue;
            }
            let ToolMiddlewareReleaseSource::Component {
                component_id,
                component_revision,
                component_name,
            } = &release.source;
            let registration = RegisteredToolMiddleware {
                deployment_revision,
                release_id: Some(release.id),
                definition: release.definition.clone(),
                provision: deployment.provision.clone(),
                source: ToolMiddlewareSource::Component {
                    component_id: *component_id,
                    component_revision: *component_revision,
                    component_name: component_name.clone(),
                },
                owner_account_id: release.owner_account_id,
                owner_account_email: resolved.owner.email.clone(),
                metadata_version: release.metadata_version.clone(),
                metadata_digest: release.metadata_digest,
            };
            if registrations
                .insert(deployment.name.clone(), registration)
                .is_some()
            {
                errors.push(DeployValidationError::ToolMiddleware {
                    middleware_name: Some(deployment.name.clone()),
                    agent_type_name: None,
                    tool_name: None,
                    message: "multiple local or remote implementations".to_string(),
                });
            }
        }
        registrations.into_values().collect()
    }

    pub fn tool_middleware_binding_inputs(
        &self,
        remote_tools: &[RemoteToolDeployment],
    ) -> (
        BTreeMap<ToolName, ToolBindingInput>,
        BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
    ) {
        let mut environment = BTreeMap::new();
        let mut agents = BTreeMap::new();
        for component in self.components.values() {
            for (name, metadata) in component.metadata.tools() {
                if let Some(binding) = &metadata.environment_binding {
                    environment.insert(name.clone(), binding.clone());
                }
                for (agent, binding) in &metadata.agent_bindings {
                    agents
                        .entry(agent.clone())
                        .or_insert_with(BTreeMap::new)
                        .insert(name.clone(), binding.clone());
                }
            }
        }
        for remote in remote_tools {
            if let Some(binding) = &remote.environment_binding {
                environment.insert(remote.name.clone(), binding.clone());
            }
            for (agent, binding) in &remote.agent_bindings {
                agents
                    .entry(agent.clone())
                    .or_insert_with(BTreeMap::new)
                    .insert(remote.name.clone(), binding.clone());
            }
        }
        (environment, agents)
    }

    pub fn new(
        environment: Environment,
        components: Vec<Component>,
        http_api_deployments: Vec<HttpApiDeployment>,
        mcp_deployments: Vec<golem_common::model::mcp_deployment::McpDeployment>,
    ) -> Result<Self, DeploymentWriteError> {
        let components = components
            .into_iter()
            .map(|c| (c.component_name.clone(), c))
            .collect();

        let http_api_deployments = http_api_deployments
            .into_iter()
            .map(|had| (had.domain.clone(), had))
            .collect();

        let mcp_deployments = mcp_deployments
            .into_iter()
            .map(|mcd| (mcd.domain.clone(), mcd))
            .collect();

        let registered_agent_types =
            extract_registered_agent_types(&components, &http_api_deployments)?;

        Ok(Self {
            environment,
            components,
            http_api_deployments,
            mcp_deployments,
            registered_agent_types,
        })
    }

    pub fn hash_with_tools(
        &self,
        compiled_tools: &CompiledTools,
        published_tools: &[ToolName],
        mcp_imports: &[McpImport],
        registered_tool_middlewares: &[RegisteredToolMiddleware],
        published_tool_middlewares: &[ToolMiddlewareName],
        universal_tool_middlewares: &[golem_common::model::tool_middleware::ToolMiddlewareInstallation],
        tool_compatibility_mode: golem_common::schema::tool::compatibility::ToolCompatibilityMode,
        environment_tool_bindings: &BTreeMap<ToolName, ToolBindingInput>,
        agent_tool_bindings: &BTreeMap<AgentTypeName, BTreeMap<ToolName, ToolBindingInput>>,
    ) -> Result<diff::Hash, diff::DiffError> {
        let published_tools = published_tools.iter().map(ToString::to_string).collect();
        let published_tool_middlewares = published_tool_middlewares
            .iter()
            .map(ToString::to_string)
            .collect();
        let (environment_tool_middleware_bindings, agent_tool_middleware_bindings) =
            diff::tool_middleware_binding_inputs(environment_tool_bindings, agent_tool_bindings);
        let diffable = diff::Deployment {
            components: self
                .components
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            mcp_deployments: self
                .mcp_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            remote_tools: diff::remote_tool_deployments(
                compiled_tools.registered_tools.clone(),
                compiled_tools.agent_tool_bindings.clone(),
                &self
                    .components
                    .values()
                    .map(|component| (component.id, component.component_name.clone()))
                    .collect(),
                &published_tools,
            )?,
            mcp_imports: mcp_imports
                .iter()
                .enumerate()
                .map(|(index, import)| (index.to_string(), HashOf::form_value(import.clone())))
                .collect(),
            published_tools,
            remote_tool_middleware_deployments: diff::remote_tool_middleware_deployments(
                registered_tool_middlewares.to_vec(),
                &published_tool_middlewares,
            )?,
            published_tool_middlewares,
            universal_tool_middlewares: universal_tool_middlewares.to_vec(),
            tool_compatibility_mode,
            environment_tool_middleware_bindings,
            agent_tool_middleware_bindings,
        };
        diffable.hash()
    }

    #[cfg(test)]
    pub fn compile_tools(
        &self,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        errors: &mut Vec<DeployValidationError>,
        warnings: &mut Vec<super::DeployValidationWarning>,
    ) -> CompiledTools {
        self.compile_tools_with_remote(deployment_revision, &[], errors, warnings)
    }

    pub fn compile_tools_with_remote(
        &self,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        remote_tools: &[(RemoteToolDeployment, Option<ResolvedGrantedToolRelease>)],
        errors: &mut Vec<DeployValidationError>,
        warnings: &mut Vec<super::DeployValidationWarning>,
    ) -> CompiledTools {
        let mut implementations =
            BTreeMap::<ToolName, Vec<(&Component, &ToolDeploymentMetadata, bool)>>::new();

        for component in self.components.values() {
            let supported_guest = component.metadata.tools().is_empty()
                || component
                    .metadata
                    .known_exports()
                    .tool_guest_interface
                    .as_deref()
                    == Some("golem:tool/guest@0.1.0");
            if !supported_guest {
                errors.push(DeployValidationError::ToolUnsupportedGuestExport {
                    component_name: component.component_name.clone(),
                    found: component
                        .metadata
                        .known_exports()
                        .tool_guest_interface
                        .clone(),
                });
            }

            for (tool_name, metadata) in component.metadata.tools() {
                let definition_name = metadata.definition.name().map(ToOwned::to_owned);
                let name_matches = definition_name.as_deref() == Some(tool_name.as_str());
                if !name_matches {
                    errors.push(DeployValidationError::ToolDefinitionNameMismatch {
                        component_name: component.component_name.clone(),
                        tool_name: tool_name.clone(),
                        definition_name,
                    });
                }

                let definition_valid = match validate_tool(&metadata.definition) {
                    Ok(()) => true,
                    Err(validation_errors) => {
                        errors.push(DeployValidationError::InvalidTool {
                            component_name: component.component_name.clone(),
                            tool_name: tool_name.clone(),
                            errors: validation_errors
                                .into_iter()
                                .map(|error| error.to_string())
                                .collect(),
                        });
                        false
                    }
                };

                implementations.entry(tool_name.clone()).or_default().push((
                    component,
                    metadata,
                    supported_guest && name_matches && definition_valid,
                ));
            }
        }

        let mut all_sources = BTreeMap::<ToolName, Vec<String>>::new();
        for (tool_name, local_implementations) in &implementations {
            all_sources.insert(
                tool_name.clone(),
                local_implementations
                    .iter()
                    .map(|(component, _, _)| format!("component {}", component.component_name))
                    .collect(),
            );
        }
        let remote_tool_names = remote_tools
            .iter()
            .map(|(deployment, _)| deployment.name.clone())
            .collect::<HashSet<_>>();
        for (index, (deployment, resolved)) in remote_tools.iter().enumerate() {
            let source = resolved
                .as_ref()
                .map(|resolved| format!("published release {}", resolved.release.id))
                .unwrap_or_else(|| format!("remote reference {}", index + 1));
            all_sources
                .entry(deployment.name.clone())
                .or_default()
                .push(source);
        }
        let mut colliding_tools = HashSet::new();
        for (tool_name, sources) in all_sources {
            if sources.len() > 1 && remote_tool_names.contains(&tool_name) {
                colliding_tools.insert(tool_name.clone());
                errors.push(DeployValidationError::ToolSourceCollision { tool_name, sources });
            }
        }

        let mut registered_tools = Vec::new();
        let mut agent_tool_bindings = Vec::new();

        for (tool_name, implementations) in implementations {
            let mut implementations = implementations
                .into_iter()
                .map(|(component, metadata, valid)| {
                    let bindings =
                        self.validate_tool_bindings(&tool_name, component, metadata, errors);
                    (component, metadata, valid, bindings)
                })
                .collect::<Vec<_>>();

            if implementations.len() > 1 {
                errors.push(DeployValidationError::DuplicateToolImplementation {
                    tool_name,
                    components: implementations
                        .iter()
                        .map(|(component, _, _, _)| component.component_name.clone())
                        .collect(),
                });
                continue;
            }
            if colliding_tools.contains(&tool_name) {
                continue;
            }

            let (
                component,
                metadata,
                valid,
                (environment_binding, valid_component_bindings, valid_agent_bindings),
            ) = implementations
                .pop()
                .expect("tool implementation list is never empty");
            if !valid {
                continue;
            }

            let source = ToolSource::Component {
                component_id: component.id,
                component_revision: component.revision,
                component_name: component.component_name.clone(),
            };
            let metadata_digest = match golem_common::model::tool_release::tool_metadata_digest(
                TOOL_METADATA_WIT_VERSION,
                &metadata.definition,
            ) {
                Ok(metadata_digest) => metadata_digest,
                Err(error) => {
                    errors.push(DeployValidationError::ToolMetadataSerialization {
                        component_name: component.component_name.clone(),
                        tool_name: tool_name.clone(),
                        error: error.to_string(),
                    });
                    continue;
                }
            };
            registered_tools.push(RegisteredTool {
                deployment_revision,
                release_id: None,
                definition: metadata.definition.clone(),
                provision: metadata.provision.clone(),
                component_bindings: valid_component_bindings.clone(),
                source: source.clone(),
                owner_account_id: component.account_id,
                owner_account_email: component.account_email.clone(),
                metadata_version: TOOL_METADATA_WIT_VERSION.to_string(),
                metadata_digest,
            });

            let mut agent_types = self.registered_agent_types.keys().collect::<Vec<_>>();
            agent_types.sort();
            for agent_type in agent_types {
                let agent_binding = valid_agent_bindings.get(agent_type).copied();

                let Some(binding) = compile_tool_binding(
                    deployment_revision,
                    ToolBindingOwner::AgentType {
                        agent_type_name: agent_type.clone(),
                    },
                    &tool_name,
                    environment_binding,
                    agent_binding,
                    None,
                    component.account_id,
                    &component.account_email,
                    source.clone(),
                    &metadata.definition.version,
                    TOOL_METADATA_WIT_VERSION,
                    metadata_digest,
                    warnings,
                ) else {
                    continue;
                };
                agent_tool_bindings.push(binding);
            }
            for (component_name, component_binding) in &valid_component_bindings {
                let Some(owner_component) = self.components.get(component_name) else {
                    continue;
                };
                if let Some(binding) = compile_tool_binding(
                    deployment_revision,
                    ToolBindingOwner::ComponentBaseline {
                        component_id: owner_component.id,
                    },
                    &tool_name,
                    environment_binding,
                    Some(component_binding),
                    None,
                    component.account_id,
                    &component.account_email,
                    source.clone(),
                    &metadata.definition.version,
                    TOOL_METADATA_WIT_VERSION,
                    metadata_digest,
                    warnings,
                ) {
                    agent_tool_bindings.push(binding);
                }
            }
        }

        for (deployment, resolved) in remote_tools {
            let Some(resolved) = resolved else {
                errors.push(DeployValidationError::RemoteToolUnavailable {
                    tool_name: deployment.name.clone(),
                });
                continue;
            };
            let release = &resolved.release;
            let mut valid = true;
            if deployment.name != release.name {
                valid = false;
                errors.push(DeployValidationError::RemoteToolNameMismatch {
                    tool_name: deployment.name.clone(),
                    release_name: release.name.clone(),
                });
            }
            if release.definition.name() != Some(release.name.as_str()) {
                valid = false;
                errors.push(DeployValidationError::RemoteToolDefinitionNameMismatch {
                    tool_name: deployment.name.clone(),
                    definition_name: release.definition.name().map(ToOwned::to_owned),
                });
            }
            if release.version != release.definition.version {
                valid = false;
                errors.push(DeployValidationError::RemoteToolVersionMismatch {
                    tool_name: deployment.name.clone(),
                    release_version: release.version.clone(),
                    definition_version: release.definition.version.clone(),
                });
            }
            if release.metadata_version != TOOL_METADATA_WIT_VERSION {
                valid = false;
                errors.push(
                    DeployValidationError::RemoteToolUnsupportedMetadataVersion {
                        tool_name: deployment.name.clone(),
                        metadata_version: release.metadata_version.clone(),
                    },
                );
            }
            if !matches!(
                golem_common::model::tool_release::tool_metadata_digest(
                    &release.metadata_version,
                    &release.definition,
                ),
                Ok(digest) if digest == release.metadata_digest
            ) {
                valid = false;
                errors.push(DeployValidationError::RemoteToolMetadataDigestMismatch {
                    tool_name: deployment.name.clone(),
                });
            }
            if let Err(validation_errors) = validate_tool(&release.definition) {
                valid = false;
                errors.push(DeployValidationError::InvalidRemoteTool {
                    tool_name: deployment.name.clone(),
                    errors: validation_errors
                        .into_iter()
                        .map(|error| error.to_string())
                        .collect(),
                });
            }

            let environment_binding = deployment.environment_binding.as_ref().and_then(|binding| {
                validate_tool_binding(
                    &deployment.name,
                    None,
                    binding,
                    &resolved.owner.email,
                    &release.version,
                    errors,
                )
            });
            let valid_component_bindings: BTreeMap<ComponentName, ToolBindingInput> = deployment
                .component_bindings
                .iter()
                .filter_map(|(component_name, binding)| {
                    validate_tool_binding(
                        &deployment.name,
                        None,
                        binding,
                        &resolved.owner.email,
                        &release.version,
                        errors,
                    )
                    .map(|binding| (component_name.clone(), binding.clone()))
                })
                .collect();
            let mut valid_agent_bindings = BTreeMap::new();
            for (agent_type, binding) in &deployment.agent_bindings {
                if !self.registered_agent_types.contains_key(agent_type) {
                    errors.push(DeployValidationError::RemoteToolBindingUnknownAgent {
                        tool_name: deployment.name.clone(),
                        agent_type: agent_type.clone(),
                    });
                }
                if let Some(binding) = validate_tool_binding(
                    &deployment.name,
                    Some(agent_type),
                    binding,
                    &resolved.owner.email,
                    &release.version,
                    errors,
                ) {
                    valid_agent_bindings.insert(agent_type.clone(), binding);
                }
            }

            if !valid || colliding_tools.contains(&deployment.name) {
                continue;
            }
            registered_tools.push(RegisteredTool {
                deployment_revision,
                release_id: Some(release.id),
                definition: release.definition.clone(),
                provision: deployment.provision.clone(),
                component_bindings: valid_component_bindings.clone(),
                source: release.source.clone(),
                owner_account_id: release.owner_account_id,
                owner_account_email: resolved.owner.email.clone(),
                metadata_version: release.metadata_version.clone(),
                metadata_digest: release.metadata_digest,
            });

            let mut agent_types = self.registered_agent_types.keys().collect::<Vec<_>>();
            agent_types.sort();
            for agent_type in agent_types {
                let Some(binding) = compile_tool_binding(
                    deployment_revision,
                    ToolBindingOwner::AgentType {
                        agent_type_name: agent_type.clone(),
                    },
                    &deployment.name,
                    environment_binding,
                    valid_agent_bindings.get(agent_type).copied(),
                    Some(release.id),
                    release.owner_account_id,
                    &resolved.owner.email,
                    release.source.clone(),
                    &release.version,
                    &release.metadata_version,
                    release.metadata_digest,
                    warnings,
                ) else {
                    continue;
                };
                agent_tool_bindings.push(binding);
            }
            for (component_name, component_binding) in &valid_component_bindings {
                let Some(owner_component) = self.components.get(component_name) else {
                    continue;
                };
                if let Some(binding) = compile_tool_binding(
                    deployment_revision,
                    ToolBindingOwner::ComponentBaseline {
                        component_id: owner_component.id,
                    },
                    &deployment.name,
                    environment_binding,
                    Some(component_binding),
                    Some(release.id),
                    release.owner_account_id,
                    &resolved.owner.email,
                    release.source.clone(),
                    &release.version,
                    &release.metadata_version,
                    release.metadata_digest,
                    warnings,
                ) {
                    agent_tool_bindings.push(binding);
                }
            }
        }

        CompiledTools {
            registered_tools,
            agent_tool_bindings,
        }
    }

    fn validate_tool_bindings<'a>(
        &self,
        tool_name: &ToolName,
        component: &Component,
        metadata: &'a ToolDeploymentMetadata,
        errors: &mut Vec<DeployValidationError>,
    ) -> (
        Option<&'a ToolBindingInput>,
        BTreeMap<ComponentName, ToolBindingInput>,
        BTreeMap<AgentTypeName, &'a ToolBindingInput>,
    ) {
        let environment_binding = metadata.environment_binding.as_ref().and_then(|binding| {
            validate_tool_binding(
                tool_name,
                None,
                binding,
                &component.account_email,
                &metadata.definition.version,
                errors,
            )
        });
        let component_bindings = metadata
            .component_bindings
            .iter()
            .filter_map(|(component_name, binding)| {
                validate_tool_binding(
                    tool_name,
                    None,
                    binding,
                    &component.account_email,
                    &metadata.definition.version,
                    errors,
                )
                .map(|binding| (component_name.clone(), binding.clone()))
            })
            .collect();
        let mut agent_bindings = BTreeMap::new();
        for (agent_type, binding) in &metadata.agent_bindings {
            if !self.registered_agent_types.contains_key(agent_type) {
                errors.push(DeployValidationError::ToolBindingUnknownAgent {
                    component_name: component.component_name.clone(),
                    tool_name: tool_name.clone(),
                    agent_type: agent_type.clone(),
                });
            }
            if let Some(binding) = validate_tool_binding(
                tool_name,
                Some(agent_type),
                binding,
                &component.account_email,
                &metadata.definition.version,
                errors,
            ) {
                agent_bindings.insert(agent_type.clone(), binding);
            }
        }
        (environment_binding, component_bindings, agent_bindings)
    }

    pub fn compile_http_api_routes(
        &self,
        security_schemes: &HashMap<SecuritySchemeName, SecuritySchemeDetails>,
        errors: &mut Vec<DeployValidationError>,
        warnings: &mut Vec<super::DeployValidationWarning>,
    ) -> Vec<UnboundCompiledRoute> {
        let mut current_route_id: i32 = 0;
        let mut all_routes = Vec::new();
        let mut seen_agent_types = HashSet::new();

        for deployment in self.http_api_deployments.values() {
            let first_route_id = current_route_id;
            let mut deployment_routes = Vec::new();

            for (agent_type, agent_options) in &deployment.agents {
                let registered_agent_type = ok_or_continue!(
                    self.registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::HttpApiDeploymentMissingAgentType {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                // check we haven't seen the agent type yet.
                // agent types may only show up once across all domains
                ok_or_continue!(
                    if !seen_agent_types.insert(agent_type.clone()) {
                        Err(DeployValidationError::HttpApiDeploymentMultipleDeploymentsForAgentType {
                            agent_type: agent_type.clone(),
                        })
                    } else {
                        Ok(())
                    },
                    errors
                );

                let http_mount = ok_or_continue!(
                    if let Some(v) = &registered_agent_type.agent_type.http_mount {
                        Ok(v)
                    } else {
                        Err(
                            DeployValidationError::HttpApiDeploymentAgentTypeMissingHttpMount {
                                agent_type: agent_type.clone(),
                            },
                        )
                    },
                    errors
                );

                let make_mount_validation_error = make_invalid_agent_mount_error_maker(
                    deployment,
                    http_mount,
                    &registered_agent_type.agent_type,
                );

                ok_or_continue!(
                    registered_agent_type
                        .agent_type
                        .validate()
                        .map_err(&make_mount_validation_error),
                    errors
                );

                let constructor_parameters = ok_or_continue!(
                    build_http_agent_constructor_parameters(
                        http_mount,
                        &registered_agent_type.agent_type.schema,
                        &registered_agent_type.agent_type.constructor.input_schema,
                        &make_mount_validation_error
                    ),
                    errors
                );

                if let Some(mount) = ok_or_continue!(
                    compile_fallback_mount(
                        &self.environment,
                        deployment,
                        &registered_agent_type.agent_type,
                        &registered_agent_type.implemented_by,
                        http_mount,
                        constructor_parameters.clone(),
                        agent_options,
                        current_route_id,
                    ),
                    errors
                ) {
                    deployment_routes.push(mount);
                    current_route_id = current_route_id.checked_add(1).unwrap();
                }

                if registered_agent_type.agent_type.kind
                    != golem_common::schema::AgentTypeKind::HttpRouter
                {
                    add_agent_method_http_routes(
                        &self.environment,
                        deployment,
                        &registered_agent_type.agent_type,
                        &registered_agent_type.implemented_by,
                        http_mount,
                        &registered_agent_type.agent_type.methods,
                        constructor_parameters,
                        agent_options,
                        &mut current_route_id,
                        &mut deployment_routes,
                        errors,
                        warnings,
                    );
                }

                add_webhook_callback_routes(
                    deployment,
                    registered_agent_type,
                    &mut current_route_id,
                    &mut deployment_routes,
                );
            }

            if let Err(error) =
                add_openapi_spec_routes(deployment, &mut current_route_id, &mut deployment_routes)
            {
                errors.push(error);
            }

            add_cors_preflight_http_routes(
                deployment,
                &mut current_route_id,
                &mut deployment_routes,
            );

            validate_final_http_api_router(
                &deployment.domain,
                &deployment_routes,
                security_schemes,
                errors,
            );

            deployment_routes.sort_by_cached_key(|route| match &route.behaviour {
                golem_service_base::custom_api::RouteBehaviour::HttpRouter(router) => (
                    false,
                    route
                        .path
                        .iter()
                        .filter_map(|segment| segment.literal_value().map(str::to_owned))
                        .collect::<Vec<_>>()
                        .join("/"),
                    router.agent_type.0.clone(),
                    router.component_id,
                ),
                _ => (
                    true,
                    String::new(),
                    String::new(),
                    golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                ),
            });
            for (offset, route) in deployment_routes.iter_mut().enumerate() {
                route.route_id = first_route_id
                    .checked_add(i32::try_from(offset).unwrap())
                    .unwrap();
            }
            all_routes.append(&mut deployment_routes);
        }

        all_routes
    }

    pub fn compile_mcp_deployments(
        &self,
        account_id: AccountId,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        security_schemes: &HashMap<SecuritySchemeName, SecuritySchemeDetails>,
        compiled_tools: &CompiledTools,
        middleware_chains: &[golem_common::model::tool_middleware::CompiledToolMiddlewareChain],
        errors: &mut Vec<DeployValidationError>,
    ) -> Vec<golem_service_base::mcp::CompiledMcp> {
        let mut all_compiled_mcps = Vec::new();

        for (domain, mcp_deployment) in &self.mcp_deployments {
            let mut registered_agent_types = Vec::new();
            let mut native_tools = Vec::new();

            if mcp_deployment.agents.is_empty() && mcp_deployment.tools.is_empty() {
                errors.push(DeployValidationError::McpDeploymentEmpty {
                    mcp_deployment_domain: domain.clone(),
                });
                continue;
            }

            let mut unique_scheme_names: HashSet<&SecuritySchemeName> = HashSet::new();
            for (agent_type, agent_options) in &mcp_deployment.agents {
                let registered_agent_type = ok_or_continue!(
                    self.registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::McpDeploymentMissingAgentType {
                            mcp_deployment_domain: domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                registered_agent_types.push(RegisteredAgentTypeSchema {
                    agent_type: registered_agent_type.agent_type.clone(),
                    implemented_by: registered_agent_type.implemented_by.clone(),
                });

                if let Some(name) = &agent_options.security_scheme {
                    unique_scheme_names.insert(name);
                }
            }

            for (tool_name, options) in &mcp_deployment.tools {
                if options.include.is_some() && options.exclude.is_some() {
                    errors.push(DeployValidationError::McpDeploymentInvalidTool {
                        mcp_deployment_domain: domain.clone(),
                        tool_name: tool_name.clone(),
                        error: "include and exclude are mutually exclusive".to_string(),
                    });
                    continue;
                }
                let Some(component) = self.components.get(&options.owner_component) else {
                    errors.push(DeployValidationError::McpDeploymentInvalidTool {
                        mcp_deployment_domain: domain.clone(),
                        tool_name: tool_name.clone(),
                        error: format!(
                            "owner component {} is not in this deployment",
                            options.owner_component.0
                        ),
                    });
                    continue;
                };
                let bound = compiled_tools.agent_tool_bindings.iter().any(|binding| {
                    binding.tool_name == *tool_name
                        && binding.owner
                            == ToolBindingOwner::ComponentBaseline {
                                component_id: component.id,
                            }
                        && binding.deployment_revision == deployment_revision
                });
                if !bound {
                    errors.push(DeployValidationError::McpDeploymentInvalidTool {
                        mcp_deployment_domain: domain.clone(),
                        tool_name: tool_name.clone(),
                        error: "effective component-baseline binding is missing".to_string(),
                    });
                    continue;
                }
                let Some(tool) = compiled_tools.registered_tools.iter().find(|tool| {
                    tool.deployment_revision == deployment_revision
                        && tool
                            .definition
                            .name()
                            .is_some_and(|name| name == tool_name.as_str())
                }) else {
                    errors.push(DeployValidationError::McpDeploymentInvalidTool {
                        mcp_deployment_domain: domain.clone(),
                        tool_name: tool_name.clone(),
                        error: "compiled tool definition is missing".to_string(),
                    });
                    continue;
                };
                let definition = middleware_chains
                    .iter()
                    .find(|chain| {
                        chain.owner
                            == ToolBindingOwner::ComponentBaseline {
                                component_id: component.id,
                            }
                            && chain.tool_name == *tool_name
                            && chain.deployment_revision == deployment_revision
                    })
                    .map(|chain| &chain.effective_definition)
                    .unwrap_or(&tool.definition);
                match golem_service_base::mcp::native_tool::compile_native_tool_exports(
                    component.id,
                    component.component_name.clone(),
                    tool_name.clone(),
                    definition,
                    options.include.as_deref(),
                    options.exclude.as_deref(),
                ) {
                    Ok(exports) => native_tools.extend(exports),
                    Err(error) => errors.push(DeployValidationError::McpDeploymentInvalidTool {
                        mcp_deployment_domain: domain.clone(),
                        tool_name: tool_name.clone(),
                        error,
                    }),
                }
                if let Some(name) = &options.security_scheme {
                    unique_scheme_names.insert(name);
                }
            }

            let mut names = mcp_deployment
                .agents
                .keys()
                .filter_map(|agent_name| self.registered_agent_types.get(agent_name))
                .flat_map(|agent| {
                    agent.agent_type.methods.iter().filter_map(|method| {
                        let has_user_input = method.input_schema.fields().iter().any(|field| {
                            matches!(
                                field.source,
                                golem_common::schema::agent::FieldSource::UserSupplied
                            )
                        });
                        (has_user_input || method.read_only.is_none())
                            .then(|| format!("{}-{}", agent.agent_type.type_name.0, method.name))
                    })
                })
                .collect::<HashSet<_>>();
            for export in &native_tools {
                if !names.insert(export.mcp_name.clone()) {
                    errors.push(DeployValidationError::McpDeploymentToolNameCollision {
                        mcp_deployment_domain: domain.clone(),
                        name: export.mcp_name.clone(),
                    });
                }
            }

            let security_scheme_name = if unique_scheme_names.len() > 1 {
                errors.push(
                    DeployValidationError::McpDeploymentConflictingSecuritySchemes {
                        mcp_deployment_domain: domain.clone(),
                    },
                );
                None
            } else if let Some(scheme_name) = unique_scheme_names.into_iter().next() {
                // Just validate that the security scheme exists, don't resolve it
                if !security_schemes.contains_key(scheme_name) {
                    errors.push(DeployValidationError::McpDeploymentUnknownSecurityScheme {
                        mcp_deployment_domain: domain.clone(),
                        security_scheme: scheme_name.clone(),
                    });
                }
                Some(scheme_name.clone())
            } else {
                None
            };

            let compiled_mcp = golem_service_base::mcp::CompiledMcp {
                account_id,
                account_email: self.environment.owner_account_email.clone(),
                environment_id: self.environment.id,
                application_name: self.environment.application_name.clone(),
                environment_name: self.environment.name.clone(),
                deployment_revision,
                domain: domain.clone(),
                security_scheme_name,
                security_scheme: None, // Will be resolved at runtime
                registered_agent_types,
                tools: native_tools,
            };
            all_compiled_mcps.push(compiled_mcp);
        }

        all_compiled_mcps
    }

    /// Get all environment level agent secret updates that need to be executed as part of the deployment
    pub fn deployment_agent_secret_creations_and_updates(
        &self,
        agent_secrets_in_environment: Vec<AgentSecret>,
        agent_secret_defaults_as_part_of_deployment: Vec<DeploymentAgentSecretDefault>,
        replace_incompatible_agent_secrets: bool,
        errors: &mut Vec<DeployValidationError>,
    ) -> (
        Vec<DeploymentAgentSecretCreation>,
        Vec<DeploymentAgentSecretUpdate>,
        Vec<DeploymentAgentSecretReplacement>,
    ) {
        let env_secrets: HashMap<&CanonicalAgentSecretPath, &AgentSecret> =
            agent_secrets_in_environment
                .iter()
                .map(|s| (&s.path, s))
                .collect();

        let defaults: HashMap<CanonicalAgentSecretPath, &DeploymentAgentSecretDefault> =
            agent_secret_defaults_as_part_of_deployment
                .iter()
                .map(|d| (d.path.clone().into(), d))
                .collect();

        let mut creations = Vec::new();
        let mut updates = Vec::new();
        let mut replacements = Vec::new();
        let mut seen_secrets = HashMap::new();

        let component_declarations = self.components.values().flat_map(|component| {
            let schema = component.metadata.config_schema();
            schema
                .declarations
                .iter()
                .map(move |declaration| (&schema.schema, declaration))
        });
        let agent_declarations = self.registered_agent_types.values().flat_map(|agent_type| {
            agent_type
                .agent_type
                .config
                .iter()
                .map(move |declaration| (&agent_type.agent_type.schema, declaration))
        });

        for (declaration_graph, config) in component_declarations.chain(agent_declarations) {
            if config.source != AgentConfigSource::Secret {
                continue;
            }

            let canonical_agent_secret_path =
                CanonicalAgentSecretPath::from_path_in_unknown_casing(&config.path);

            // The agent-type-declared secret value type is already a
            // schema-native `SchemaType`; pair it with the agent's shared
            // graph defs so any `SchemaType::Ref` inside resolves.
            let config_secret_schema = ok_or_continue!(
                stored_agent_secret_schema(
                    &canonical_agent_secret_path,
                    declaration_graph,
                    &config.value_type,
                ),
                errors
            );

            match seen_secrets.entry(canonical_agent_secret_path.clone()) {
                hash_map::Entry::Vacant(e) => {
                    e.insert(config_secret_schema.clone());
                }
                hash_map::Entry::Occupied(e) => {
                    let seen_secret_schema = e.get();
                    // Compare the two agent-declared secret types
                    // structurally across their own graphs: each agent type
                    // carries its own `defs`, so a raw `SchemaGraph` equality
                    // would spuriously differ even when the secret type is
                    // logically identical.
                    if !is_equivalent_cross_graph(
                        seen_secret_schema,
                        &seen_secret_schema.root,
                        &config_secret_schema,
                        &config_secret_schema.root,
                    ) {
                        ok_or_continue!(
                            Err(DeployValidationError::AgentSecretTypeConflict {
                                path: canonical_agent_secret_path
                            }),
                            errors
                        );
                    }
                    // we already processed this secret previously, nothing to do here
                    continue;
                }
            }

            if let Some(environment_agent_secret_declaration) =
                env_secrets.get(&canonical_agent_secret_path)
            {
                // secret does exist in environment, we need to check that types are compatible with deployment
                if !is_equivalent_cross_graph(
                    &environment_agent_secret_declaration.secret_type,
                    &environment_agent_secret_declaration.secret_type.root,
                    &config_secret_schema,
                    &config_secret_schema.root,
                ) {
                    if replace_incompatible_agent_secrets {
                        let agent_secret_default = defaults.get(&canonical_agent_secret_path);

                        let agent_secret_value = ok_or_continue!(
                            parse_default_secret_value(
                                &canonical_agent_secret_path,
                                agent_secret_default,
                                &config_secret_schema,
                            ),
                            errors
                        );

                        replacements.push(DeploymentAgentSecretReplacement {
                            agent_secret_id: environment_agent_secret_declaration.id,
                            current_revision: environment_agent_secret_declaration.revision,
                            path: canonical_agent_secret_path.clone(),
                            secret_type: config_secret_schema,
                            secret_value: agent_secret_value,
                        });
                    } else {
                        errors.push(
                            DeployValidationError::AgentSecretNotCompatibleWithEnvironmentSecret {
                                path: canonical_agent_secret_path.clone(),
                                agent_secret_type: Box::new(config_secret_schema),
                                environment_secret_type: Box::new(
                                    environment_agent_secret_declaration.secret_type.clone(),
                                ),
                            },
                        );
                    }

                    continue;
                }

                // declaration exists in environment but has no value.
                // if default was provided as part of deployment we can set it now.
                if environment_agent_secret_declaration.secret_value.is_none() {
                    let agent_secret_default = defaults.get(&canonical_agent_secret_path);

                    let agent_secret_value = ok_or_continue!(
                        parse_default_secret_value(
                            &canonical_agent_secret_path,
                            agent_secret_default,
                            &config_secret_schema,
                        ),
                        errors
                    );

                    if let Some(secret_value) = agent_secret_value {
                        updates.push(DeploymentAgentSecretUpdate {
                            agent_secret_id: environment_agent_secret_declaration.id,
                            current_revision: environment_agent_secret_declaration.revision,
                            new_secret_value: secret_value,
                        });
                    }
                }
            } else {
                // secret does not yet exist in environment, create it with optional default.
                let agent_secret_default = defaults.get(&canonical_agent_secret_path);

                let agent_secret_value = ok_or_continue!(
                    parse_default_secret_value(
                        &canonical_agent_secret_path,
                        agent_secret_default,
                        &config_secret_schema,
                    ),
                    errors
                );

                creations.push(DeploymentAgentSecretCreation {
                    path: canonical_agent_secret_path,
                    secret_type: config_secret_schema,
                    secret_value: agent_secret_value,
                });
            }
        }

        (creations, updates, replacements)
    }

    pub fn deployment_resource_definition_creations(
        &self,
        resource_definitions_in_environment: Vec<ResourceDefinition>,
        resource_definition_defaults_in_deployment: Vec<ResourceDefinitionCreation>,
        errors: &mut Vec<DeployValidationError>,
    ) -> Vec<ResourceDefinitionCreation> {
        let resources_existing_in_env: HashSet<&ResourceName> = resource_definitions_in_environment
            .iter()
            .map(|s| &s.name)
            .collect();

        let mut creations = Vec::new();
        let mut seen_resources = HashSet::new();

        for resource_default in resource_definition_defaults_in_deployment {
            if !seen_resources.insert(resource_default.name.clone()) {
                ok_or_continue!(
                    Err(DeployValidationError::ConflictingResourceDefinitions {
                        name: resource_default.name.clone()
                    }),
                    errors
                );
            }

            if !resources_existing_in_env.contains(&resource_default.name) {
                creations.push(resource_default);
            }
        }
        creations
    }

    /// Get all environment level retry policy creations that need to be executed as part of the deployment.
    /// Policies that already exist by name in the environment are skipped (warn+skip, never overwrite).
    pub fn deployment_retry_policy_creations(
        &self,
        retry_policies_in_environment: Vec<StoredRetryPolicy>,
        retry_policy_defaults_in_deployment: Vec<DeploymentRetryPolicyDefault>,
        actor: AccountId,
        errors: &mut Vec<DeployValidationError>,
    ) -> Result<Vec<RetryPolicyCreationRecord>, DeploymentWriteError> {
        let existing_names: HashSet<String> = retry_policies_in_environment
            .iter()
            .map(|p| p.name.clone())
            .collect();

        let mut creations = Vec::new();
        let mut seen_names = HashSet::new();

        for rpd in retry_policy_defaults_in_deployment {
            if !seen_names.insert(rpd.name.clone()) {
                ok_or_continue!(
                    Err(DeployValidationError::ConflictingRetryPolicyDefaults {
                        name: rpd.name.clone()
                    }),
                    errors
                );
            }

            if existing_names.contains(&rpd.name) {
                tracing::warn!(
                    "Retry policy '{}' already exists in environment, skipping deployment default",
                    rpd.name
                );
                continue;
            }

            creations.push(RetryPolicyCreationRecord::new(
                RetryPolicyId::new(),
                self.environment.id,
                rpd.name,
                rpd.priority,
                serde_json::to_string(&golem_common::model::retry_policy::Predicate::from(
                    rpd.predicate,
                ))
                .map_err(|e| DeploymentWriteError::InternalError(e.into()))?,
                serde_json::to_string(&golem_common::model::retry_policy::RetryPolicy::from(
                    rpd.policy,
                ))
                .map_err(|e| DeploymentWriteError::InternalError(e.into()))?,
                actor,
            ));
        }

        Ok(creations)
    }
}

fn validate_tool_binding<'a>(
    tool_name: &ToolName,
    agent_type: Option<&AgentTypeName>,
    binding: &'a ToolBindingInput,
    owner_account_email: &AccountEmail,
    tool_version: &str,
    errors: &mut Vec<DeployValidationError>,
) -> Option<&'a ToolBindingInput> {
    let mut valid = true;
    if agent_type.is_none() && binding.middleware_merge_mode.is_some() {
        valid = false;
        errors.push(
            DeployValidationError::ToolBindingEnvironmentMiddlewareMergeMode {
                tool_name: tool_name.clone(),
            },
        );
    }
    if let Some(version) = &binding.version
        && version != tool_version
    {
        valid = false;
        errors.push(DeployValidationError::ToolBindingVersionMismatch {
            tool_name: tool_name.clone(),
            agent_type: agent_type.cloned(),
            requested_version: version.clone(),
            tool_version: tool_version.to_string(),
        });
    }
    if let Some(account) = &binding.account
        && account != owner_account_email
    {
        valid = false;
        errors.push(DeployValidationError::ToolBindingAccountMismatch {
            tool_name: tool_name.clone(),
            agent_type: agent_type.cloned(),
            requested_account: account.to_string(),
            owner_account: owner_account_email.to_string(),
        });
    }
    if !binding.parameters.0.is_object() {
        valid = false;
        errors.push(DeployValidationError::ToolBindingParametersMustBeObject {
            tool_name: tool_name.clone(),
            agent_type: agent_type.cloned(),
        });
    }
    valid.then_some(binding)
}

#[allow(clippy::too_many_arguments)]
fn compile_tool_binding(
    deployment_revision: golem_common::model::deployment::DeploymentRevision,
    owner: ToolBindingOwner,
    tool_name: &ToolName,
    environment: Option<&ToolBindingInput>,
    agent: Option<&ToolBindingInput>,
    release_id: Option<ToolReleaseId>,
    owner_account_id: AccountId,
    owner_account_email: &AccountEmail,
    source: ToolSource,
    version: &str,
    metadata_version: &str,
    metadata_digest: golem_common::model::diff::Hash,
    warnings: &mut Vec<super::DeployValidationWarning>,
) -> Option<CompiledToolBinding> {
    let (binding, revealable_scope_narrowed) = diff::effective_tool_binding(environment, agent)?;
    if revealable_scope_narrowed && let ToolBindingOwner::AgentType { agent_type_name } = &owner {
        warnings.push(super::DeployValidationWarning::ToolRevealableSecretKeysDropped(
            golem_common::base_model::deploy_validation_warning::ToolRevealableSecretKeysDropped {
                agent_type: agent_type_name.clone(),
                tool_name: tool_name.clone(),
            },
        ));
    }

    Some(CompiledToolBinding {
        deployment_revision,
        release_id,
        owner,
        tool_name: tool_name.clone(),
        version: version.to_string(),
        metadata_version: metadata_version.to_string(),
        metadata_digest,
        account_id: owner_account_id,
        account_email: owner_account_email.clone(),
        parameters: binding.parameters,
        config_keys_readable: binding.config_keys_readable,
        secret_keys_readable: binding.secret_keys_readable,
        secret_keys_revealable: binding.secret_keys_revealable,
        filesystem_access: binding.filesystem_access,
        source,
    })
}

/// Parse the optional JSON-encoded default for an agent secret against the
/// agent's declared schema graph.
///
/// Returns `Ok(None)` when no default was supplied. Returns
/// [`DeployValidationError::AgentSecretDefaultTypeMismatch`] when the JSON
/// payload cannot be decoded into a [`SchemaValue`] for the given graph.
///
/// The deployment request DTO carries ergonomic, human-shaped JSON (raw
/// scalars, field-named record objects). It is decoded directly into a
/// schema-native [`SchemaValue`] via
/// [`golem_schema::schema::render::from_untrusted_json_value`], which both
/// type-checks the payload against the agent-declared schema and produces the
/// value in one step.
fn parse_default_secret_value(
    path: &CanonicalAgentSecretPath,
    default: Option<&&DeploymentAgentSecretDefault>,
    schema: &SchemaGraph,
) -> Result<Option<golem_common::schema::schema_value::SchemaValue>, DeployValidationError> {
    default
        .map(|sd| {
            golem_schema::schema::render::from_untrusted_json_value(
                schema,
                &schema.root,
                &sd.secret_value,
            )
            .map_err(|e| DeployValidationError::AgentSecretDefaultTypeMismatch {
                path: path.clone(),
                errors: vec![e.to_string()],
            })
        })
        .transpose()
}

fn stored_agent_secret_schema(
    path: &CanonicalAgentSecretPath,
    agent_graph: &SchemaGraph,
    config_type: &SchemaType,
) -> Result<SchemaGraph, DeployValidationError> {
    let root = match resolve_schema_ref(agent_graph, config_type) {
        SchemaType::Secret { spec, .. } => (*spec.inner).clone(),
        SchemaType::Option { inner, .. } => match resolve_schema_ref(agent_graph, inner) {
            SchemaType::Secret { spec, .. } => (*spec.inner).clone(),
            _ => {
                return Err(DeployValidationError::AgentSecretInvalidConfigType {
                    path: path.clone(),
                });
            }
        },
        _ => {
            return Err(DeployValidationError::AgentSecretInvalidConfigType { path: path.clone() });
        }
    };

    let schema = SchemaGraph {
        defs: reachable_defs(agent_graph, &root),
        root,
    };

    if schema_contains_host_managed_capability(&schema) {
        Err(DeployValidationError::AgentSecretInvalidConfigType { path: path.clone() })
    } else {
        Ok(schema)
    }
}

fn resolve_schema_ref<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
    let mut seen = std::collections::HashSet::new();
    while let SchemaType::Ref { id, .. } = ty {
        if !seen.insert(id.clone()) {
            break;
        }
        match graph.lookup(id) {
            Some(def) => ty = &def.body,
            None => break,
        }
    }
    ty
}

pub fn extract_registered_agent_types(
    components: &BTreeMap<ComponentName, Component>,
    http_api_deployments: &BTreeMap<Domain, HttpApiDeployment>,
) -> Result<HashMap<AgentTypeName, InProgressDeployedRegisteredAgentType>, DeploymentWriteError> {
    let mut agent_types = HashMap::new();
    let mut errors = Vec::new();

    for component in components.values() {
        for agent_type in component.metadata.agent_types() {
            let agent_type_name = agent_type.type_name.clone();
            let implementer = RegisteredAgentTypeImplementer {
                component_id: component.id,
                component_revision: component.revision,
                component_name: component.component_name.0.clone(),
                account_id: component.account_id,
                account_email: component.account_email.clone(),
            };

            let webhook_domain_and_segments = ok_or_continue!(
                build_agent_http_api_deployment_details(
                    &agent_type_name,
                    agent_type,
                    &implementer,
                    http_api_deployments
                ),
                errors
            );

            let registered_agent_type = InProgressDeployedRegisteredAgentType {
                agent_type: agent_type.clone(),
                implemented_by: RegisteredAgentTypeImplementer {
                    component_id: component.id,
                    component_revision: component.revision,
                    component_name: component.component_name.0.clone(),
                    account_id: component.account_id,
                    account_email: component.account_email.clone(),
                },
                webhook_domain_and_segments,
            };

            // Agent types can only be implemented once per deployments
            ok_or_continue!(
                if agent_types
                    .insert(agent_type_name, registered_agent_type)
                    .is_some()
                {
                    Err(DeployValidationError::AmbiguousAgentTypeName(
                        agent_type.type_name.clone(),
                    ))
                } else {
                    Ok(())
                },
                errors
            )
        }
    }

    // Check for kebab-case collisions
    let mut kebab_map: BTreeMap<String, AgentTypeName> = BTreeMap::new();
    for agent_type_name in agent_types.keys() {
        let kebab = agent_type_name.0.to_kebab_case();
        if let Some(existing) = kebab_map.get(&kebab) {
            errors.push(DeployValidationError::ConflictingAgentTypeNames {
                name1: existing.clone(),
                name2: agent_type_name.clone(),
                normalized: kebab.clone(),
            });
        } else {
            kebab_map.insert(kebab, agent_type_name.clone());
        }
    }

    if !errors.is_empty() {
        return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
    };

    Ok(agent_types)
}

fn validate_final_http_api_router(
    domain: &Domain,
    compiled_routes: &[UnboundCompiledRoute],
    security_schemes: &HashMap<SecuritySchemeName, SecuritySchemeDetails>,
    errors: &mut Vec<DeployValidationError>,
) {
    use golem_service_base::custom_api::{PathSegment, RouteBehaviour, RouteMatch};
    let invalid =
        |path: &[PathSegment], error: &str| DeployValidationError::HttpApiDeploymentInvalidRoute {
            domain: domain.clone(),
            path: path.to_vec(),
            error: error.into(),
        };
    let mut reserved = compiled_routes
        .iter()
        .filter(|route| {
            matches!(
                route.behaviour,
                RouteBehaviour::WebhookCallback(_) | RouteBehaviour::OpenApiSpec(_)
            )
        })
        .map(|route| (route.route_match.clone(), route.path.clone()))
        .collect::<Vec<_>>();
    let used_schemes = compiled_routes
        .iter()
        .filter_map(UnboundCompiledRoute::security_scheme)
        .collect::<HashSet<_>>();
    for name in used_schemes {
        if let Some(scheme) = security_schemes.get(&name) {
            match golem_common::model::agent::http_files::HttpRequestTarget::parse(
                scheme.redirect_url.url().path(),
            ) {
                Ok(target) => reserved.push((
                    RouteMatch::Method {
                        method: golem_common::model::agent::HttpMethod::Get(
                            golem_common::model::Empty {},
                        ),
                        trailing_slash: target.trailing_slash(),
                    },
                    target
                        .segments()
                        .iter()
                        .map(|value| PathSegment::Literal {
                            value: value.clone(),
                        })
                        .collect(),
                )),
                _ => errors.push(invalid(&[], "Invalid OIDC callback path")),
            }
        }
    }
    let providers = compiled_routes.iter().filter(|route| matches!(&route.behaviour, RouteBehaviour::HttpRouter(router) if router.openapi_provider_method.is_some())).count();
    if providers > 64 {
        errors.push(invalid(
            &[],
            "A domain may have at most 64 OpenAPI providers",
        ));
    }

    let mut mounts: Vec<&UnboundCompiledRoute> = Vec::new();
    let mut routers = [
        golem_service_base::custom_api::router::Router::new(),
        golem_service_base::custom_api::router::Router::new(),
    ];

    for compiled_route in compiled_routes {
        if let Err(error) = validate_path_segments(&compiled_route.path, domain) {
            errors.push(invalid(&compiled_route.path, error));
            continue;
        }
        if matches!(compiled_route.behaviour, RouteBehaviour::CorsPreflight(_)) {
            continue;
        }
        if matches!(compiled_route.route_match, RouteMatch::MountPrefix) {
            if mounts.iter().any(|other| {
                other.path.len() == compiled_route.path.len()
                    && other
                        .path
                        .iter()
                        .zip(&compiled_route.path)
                        .all(|(a, b)| match (a, b) {
                            (
                                PathSegment::Literal { value: a },
                                PathSegment::Literal { value: b },
                            ) => a == b,
                            (PathSegment::Variable { .. }, PathSegment::Variable { .. }) => true,
                            _ => false,
                        })
            }) {
                errors.push(invalid(
                    &compiled_route.path,
                    "Equally specific overlapping fallback mounts",
                ));
            }
            mounts.push(compiled_route);
            continue;
        }
        let Some(route_method) = compiled_route.route_match.method() else {
            continue;
        };
        if matches!(compiled_route.behaviour, RouteBehaviour::CallAgent(_))
            && reserved.iter().any(|(route_match, path)| {
                let same_method = match (route_match, &compiled_route.route_match) {
                    (
                        RouteMatch::Method {
                            method: a,
                            trailing_slash: a_slash,
                        },
                        RouteMatch::Method {
                            method: b,
                            trailing_slash: b_slash,
                        },
                    ) => {
                        a_slash == b_slash
                            && super::route_compilation::render_http_method(a)
                                == super::route_compilation::render_http_method(b)
                    }
                    _ => false,
                };
                same_method
                    && path.len() == compiled_route.path.len()
                    && path
                        .iter()
                        .zip(&compiled_route.path)
                        .all(|(reserved, typed)| match (reserved, typed) {
                            (
                                PathSegment::Literal { value: a },
                                PathSegment::Literal { value: b },
                            ) => a == b,
                            (
                                PathSegment::Variable { .. },
                                PathSegment::Literal { .. } | PathSegment::Variable { .. },
                            ) => true,
                            _ => false,
                        })
            })
        {
            errors.push(invalid(
                &compiled_route.path,
                "Typed endpoint collides with a reserved HTTP binding",
            ));
            continue;
        }
        let method: http::Method = ok_or_continue!(
            route_method.clone().try_into().map_err(|_| {
                DeployValidationError::InvalidHttpMethod {
                    method: route_method.clone(),
                }
            }),
            errors
        );

        let trailing_slash = matches!(
            compiled_route.route_match,
            RouteMatch::Method {
                trailing_slash: true,
                ..
            }
        );
        if !routers[usize::from(trailing_slash)].add_route(method, compiled_route.path.clone(), ())
        {
            errors.push(DeployValidationError::RouteIsAmbiguous {
                domain: domain.clone(),
                method: route_method.clone(),
                path: compiled_route.path.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests;
