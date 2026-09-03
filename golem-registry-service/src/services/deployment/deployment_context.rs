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
    add_webhook_callback_routes, build_agent_http_api_deployment_details,
    make_invalid_agent_mount_error_maker,
};
use crate::model::agent_secret::{
    DeploymentAgentSecretCreation, DeploymentAgentSecretReplacement, DeploymentAgentSecretUpdate,
};
use crate::model::api_definition::UnboundCompiledRoute;
use crate::repo::model::retry_policy::RetryPolicyCreationRecord;
use crate::services::agent_secret::schema_contains_host_managed_capability;
use crate::services::deployment::route_compilation::validate_path_segments;
use crate::services::environment_tool_grant::ResolvedGrantedToolRelease;
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
use golem_common::model::quota::{ResourceDefinition, ResourceDefinitionCreation, ResourceName};
use golem_common::model::retry_policy::RetryPolicyId;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::model::tool::{
    CompiledToolBinding, RegisteredTool, RemoteToolDeployment, TOOL_METADATA_WIT_VERSION,
    ToolBindingInput, ToolDeploymentMetadata, ToolName, ToolSource,
};
use golem_common::model::tool_release::ToolReleaseId;
use golem_common::schema::agent::reachable_defs;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::render;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::tool::validation::validate_tool;
use golem_common::schema::validation::is_equivalent_cross_graph;
use golem_common::schema::{AgentTypeSchema, RegisteredAgentTypeSchema};
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::Component;
use golem_service_base::model::retry_policy::StoredRetryPolicy;
use heck::ToKebabCase;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map};

#[derive(Debug)]
pub struct CompiledTools {
    pub registered_tools: Vec<RegisteredTool>,
    pub agent_tool_bindings: Vec<CompiledToolBinding>,
}

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
    ) -> Result<diff::Hash, diff::DiffError> {
        let local_component_revisions = self
            .components
            .values()
            .map(|component| (component.id, component.revision))
            .collect::<BTreeSet<_>>();
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
                &local_component_revisions,
            ),
            published_tools: published_tools.iter().map(ToString::to_string).collect(),
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
                .map(|resolved| format!("registry release {}", resolved.release.id))
                .unwrap_or_else(|| format!("registry reference {}", index + 1));
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

            let (component, metadata, valid, (environment_binding, valid_agent_bindings)) =
                implementations
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
                    agent_type,
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
                    agent_type,
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
        (environment_binding, agent_bindings)
    }

    pub fn compile_http_api_routes(
        &self,
        errors: &mut Vec<DeployValidationError>,
        warnings: &mut Vec<super::DeployValidationWarning>,
    ) -> Vec<UnboundCompiledRoute> {
        let mut current_route_id: i32 = 0;
        let mut all_routes = Vec::new();
        let mut seen_agent_types = HashSet::new();

        for deployment in self.http_api_deployments.values() {
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

                let constructor_parameters = ok_or_continue!(
                    build_http_agent_constructor_parameters(
                        http_mount,
                        &registered_agent_type.agent_type.schema,
                        &registered_agent_type.agent_type.constructor.input_schema,
                        &make_mount_validation_error
                    ),
                    errors
                );

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

                add_webhook_callback_routes(
                    deployment,
                    registered_agent_type,
                    &mut current_route_id,
                    &mut deployment_routes,
                );
            }

            add_openapi_spec_routes(deployment, &mut current_route_id, &mut deployment_routes);

            add_cors_preflight_http_routes(
                deployment,
                &mut current_route_id,
                &mut deployment_routes,
            );

            validate_final_http_api_router(&deployment.domain, &deployment_routes, errors);

            all_routes.append(&mut deployment_routes);
        }

        all_routes
    }

    pub fn compile_mcp_deployments(
        &self,
        account_id: AccountId,
        deployment_revision: golem_common::model::deployment::DeploymentRevision,
        security_schemes: &HashMap<SecuritySchemeName, SecuritySchemeDetails>,
        errors: &mut Vec<DeployValidationError>,
    ) -> Vec<golem_service_base::mcp::CompiledMcp> {
        let mut all_compiled_mcps = Vec::new();

        for (domain, mcp_deployment) in &self.mcp_deployments {
            let mut registered_agent_types = Vec::new();

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
                deployment_revision,
                domain: domain.clone(),
                security_scheme_name,
                security_scheme: None, // Will be resolved at runtime
                registered_agent_types,
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

        for agent_type in self.registered_agent_types.values() {
            for config in &agent_type.agent_type.config {
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
                        &agent_type.agent_type.schema,
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
                                        environment_agent_secret_declaration
                                            .secret_type
                                            .clone(),
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
    agent_type: &AgentTypeName,
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
    if revealable_scope_narrowed {
        warnings.push(super::DeployValidationWarning::ToolRevealableSecretKeysDropped(
            golem_common::base_model::deploy_validation_warning::ToolRevealableSecretKeysDropped {
                agent_type: agent_type.clone(),
                tool_name: tool_name.clone(),
            },
        ));
    }

    Some(CompiledToolBinding {
        deployment_revision,
        release_id,
        agent_type_name: agent_type.clone(),
        tool_name: tool_name.clone(),
        version: version.to_string(),
        metadata_version: metadata_version.to_string(),
        metadata_digest,
        account_id: owner_account_id,
        account_email: owner_account_email.clone(),
        parameters: binding.parameters,
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
/// schema-native [`SchemaValue`] via [`render::from_json_value`], which both
/// type-checks the payload against the agent-declared schema and produces the
/// value in one step.
fn parse_default_secret_value(
    path: &CanonicalAgentSecretPath,
    default: Option<&&DeploymentAgentSecretDefault>,
    schema: &SchemaGraph,
) -> Result<Option<golem_common::schema::schema_value::SchemaValue>, DeployValidationError> {
    default
        .map(|sd| {
            render::from_json_value(schema, &schema.root, &sd.secret_value).map_err(|e| {
                DeployValidationError::AgentSecretDefaultTypeMismatch {
                    path: path.clone(),
                    errors: vec![e.to_string()],
                }
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
    errors: &mut Vec<DeployValidationError>,
) {
    let mut router = golem_service_base::custom_api::router::Router::new();

    for compiled_route in compiled_routes {
        let method: http::Method = ok_or_continue!(
            compiled_route.method.clone().try_into().map_err(|_| {
                DeployValidationError::InvalidHttpMethod {
                    method: compiled_route.method.clone(),
                }
            }),
            errors
        );

        ok_or_continue!(
            validate_path_segments(&compiled_route.path, domain).map_err(|e| {
                DeployValidationError::HttpApiDeploymentInvalidRoute {
                    domain: domain.clone(),
                    path: compiled_route.path.clone(),
                    error: e.to_string(),
                }
            },),
            errors
        );

        if !router.add_route(method, compiled_route.path.clone(), ()) {
            errors.push(DeployValidationError::RouteIsAmbiguous {
                domain: domain.clone(),
                method: compiled_route.method.clone(),
                path: compiled_route.path.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::model::deployment::{CompiledMcpData, DeploymentCompiledMcpRecord};
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId, AccountSummary};
    use golem_common::model::agent::{AgentMode, Snapshotting};
    use golem_common::model::agent_secret::{AgentSecretId, AgentSecretPath, AgentSecretRevision};
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
    use golem_common::model::environment::{EnvironmentId, EnvironmentName, EnvironmentRevision};
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::mcp_deployment::{
        McpDeployment, McpDeploymentAgentOptions, McpDeploymentId, McpDeploymentRevision,
    };
    use golem_common::model::tool::{RemoteToolDeployment, SecretKeyScope, ToolProvisionConfig};
    use golem_common::model::tool_release::{
        ToolRelease, ToolReleaseById, ToolReleaseId, ToolReleaseLifecycle, ToolReleaseOrigin,
        ToolReleaseReference,
    };
    use golem_common::schema::agent::{
        AgentConfigDeclarationSchema, AgentConstructorSchema, InputSchema,
    };
    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::metadata::TypeId;
    use golem_common::schema::schema_type::{QuotaTokenSpec, SchemaType, SecretSpec};
    use golem_common::schema::schema_value::SchemaValue;
    use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
    use golem_service_base::mcp::CompiledMcp;
    use golem_service_base::repo::Blob;
    use serde_json::json;
    use std::collections::BTreeSet;
    use test_r::test;

    fn test_environment() -> Environment {
        Environment {
            id: EnvironmentId::new(),
            revision: EnvironmentRevision::INITIAL,
            application_id: ApplicationId::new(),
            application_name: ApplicationName::try_from("app").unwrap(),
            name: EnvironmentName::try_from("dev").unwrap(),
            diff_model_version: 0,
            compatibility_check: false,
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId::new(),
            owner_account_email: AccountEmail::new("owner@example.com"),
            current_deployment: None,
        }
    }

    fn test_implementer() -> RegisteredAgentTypeImplementer {
        RegisteredAgentTypeImplementer {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: "component".to_string(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
        }
    }

    fn agent_type_with_secret_config(
        agent_type_name: AgentTypeName,
        path: Vec<String>,
        value_type: SchemaType,
    ) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: agent_type_name,
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::parameters([]),
            },
            methods: Vec::new(),
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![AgentConfigDeclarationSchema {
                source: AgentConfigSource::Secret,
                path,
                value_type,
            }],
        }
    }

    fn stored_agent_secret(
        path: &[String],
        secret_type: SchemaGraph,
        secret_value: Option<SchemaValue>,
    ) -> AgentSecret {
        AgentSecret {
            id: AgentSecretId::new(),
            environment_id: EnvironmentId::new(),
            path: CanonicalAgentSecretPath::from_path_in_unknown_casing(path),
            revision: AgentSecretRevision::INITIAL,
            secret_type,
            secret_value,
        }
    }

    fn test_tool(name: &str) -> Tool {
        Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: Vec::new(),
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        }
    }

    fn test_tool_component(
        name: &str,
        tools: BTreeMap<ToolName, ToolDeploymentMetadata>,
    ) -> Component {
        Component {
            id: ComponentId::new(),
            revision: ComponentRevision::INITIAL,
            environment_id: EnvironmentId::new(),
            component_name: ComponentName(name.to_string()),
            hash: diff::Hash::empty(),
            application_id: ApplicationId::new(),
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            application_name: ApplicationName::try_from("app").unwrap(),
            environment_name: EnvironmentName::try_from("dev").unwrap(),
            component_size: 0,
            metadata: ComponentMetadata::from_parts_with_tools(
                KnownExports {
                    tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
                    ..KnownExports::default()
                },
                Vec::new(),
                None,
                None,
                Vec::new(),
                BTreeMap::new(),
                tools,
            ),
            created_at: chrono::Utc::now(),
            wasm_hash: diff::Hash::empty(),
            object_store_key: String::new(),
        }
    }

    fn test_remote_tool(
        name: &str,
        environment_binding: Option<ToolBindingInput>,
        agent_bindings: BTreeMap<AgentTypeName, ToolBindingInput>,
    ) -> (RemoteToolDeployment, Option<ResolvedGrantedToolRelease>) {
        let name = ToolName::try_from(name).unwrap();
        let owner_account_id = AccountId::new();
        let owner_email = AccountEmail::new("publisher@example.com");
        let release_id = ToolReleaseId::new();
        let definition = test_tool(name.as_str());
        let release = ToolRelease {
            id: release_id,
            owner_account_id,
            name: name.clone(),
            version: definition.version.clone(),
            source: ToolSource::Component {
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                component_name: ComponentName("publisher-tools".to_string()),
            },
            definition: definition.clone(),
            metadata_version: TOOL_METADATA_WIT_VERSION.to_string(),
            metadata_digest: golem_common::model::tool_release::tool_metadata_digest(
                TOOL_METADATA_WIT_VERSION,
                &definition,
            )
            .unwrap(),
            lifecycle: ToolReleaseLifecycle::Published,
            origin: ToolReleaseOrigin::Ordinary,
            system_availability: None,
            created_at: chrono::Utc::now(),
            created_by: owner_account_id,
            state_changed_at: chrono::Utc::now(),
            state_changed_by: owner_account_id,
        };
        (
            RemoteToolDeployment {
                name,
                release: ToolReleaseReference::ById(ToolReleaseById { release_id }),
                provision: ToolProvisionConfig {
                    config: NormalizedJsonValue::new(json!({ "consumer": true })),
                    ..ToolProvisionConfig::default()
                },
                environment_binding,
                agent_bindings,
            },
            Some(ResolvedGrantedToolRelease {
                release,
                owner: AccountSummary {
                    id: owner_account_id,
                    name: "Publisher".to_string(),
                    email: owner_email,
                },
            }),
        )
    }

    fn test_registered_agent_type(
        agent_type_name: &str,
    ) -> (AgentTypeName, InProgressDeployedRegisteredAgentType) {
        let agent_type_name = AgentTypeName(agent_type_name.to_string());
        let agent_type = agent_type_with_secret_config(
            agent_type_name.clone(),
            vec!["apiKey".to_string()],
            SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: None,
            }),
        );
        (
            agent_type_name,
            InProgressDeployedRegisteredAgentType {
                agent_type,
                implemented_by: test_implementer(),
                webhook_domain_and_segments: None,
            },
        )
    }

    fn compile_test_mcp() -> (CompiledMcp, Vec<RegisteredAgentTypeSchema>) {
        let environment = test_environment();
        let domain = Domain("mcp.example.com".to_string());
        let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
        let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
        let (agent_c_name, agent_c) = test_registered_agent_type("AgentC");
        let expected = vec![
            RegisteredAgentTypeSchema {
                agent_type: agent_a.agent_type.clone(),
                implemented_by: agent_a.implemented_by.clone(),
            },
            RegisteredAgentTypeSchema {
                agent_type: agent_b.agent_type.clone(),
                implemented_by: agent_b.implemented_by.clone(),
            },
        ];
        let mcp_deployment = McpDeployment {
            id: McpDeploymentId::new(),
            revision: McpDeploymentRevision::INITIAL,
            environment_id: environment.id,
            domain: domain.clone(),
            hash: diff::Hash::empty(),
            agents: BTreeMap::from([
                (agent_b_name.clone(), McpDeploymentAgentOptions::default()),
                (agent_a_name.clone(), McpDeploymentAgentOptions::default()),
            ]),
            created_at: chrono::Utc::now(),
        };
        let context = DeploymentContext {
            environment,
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::from([(domain, mcp_deployment)]),
            registered_agent_types: HashMap::from([
                (agent_a_name, agent_a),
                (agent_b_name, agent_b),
                (agent_c_name, agent_c),
            ]),
        };
        let mut errors = Vec::new();
        let mut compiled = context.compile_mcp_deployments(
            AccountId::new(),
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &HashMap::new(),
            &mut errors,
        );

        assert!(errors.is_empty());
        assert_eq!(compiled.len(), 1);

        (compiled.pop().unwrap(), expected)
    }

    #[test]
    fn compile_mcp_deployments_includes_selected_registered_agent_types() {
        let (compiled, expected) = compile_test_mcp();

        assert_eq!(compiled.registered_agent_types, expected);
    }

    #[test]
    fn compiled_mcp_blob_round_trip_preserves_registered_agent_types() {
        let (compiled, expected) = compile_test_mcp();
        let record = DeploymentCompiledMcpRecord::from_model(compiled);
        let serialized = record.mcp_data.serialize().unwrap().clone();
        let mcp_data: Blob<CompiledMcpData> = Blob::deserialze(serialized).unwrap();
        let restored = CompiledMcp::try_from(DeploymentCompiledMcpRecord {
            account_id: record.account_id,
            account_email: record.account_email,
            environment_id: record.environment_id,
            deployment_revision_id: record.deployment_revision_id,
            domain: record.domain,
            mcp_data,
        })
        .unwrap();

        assert_eq!(restored.registered_agent_types, expected);
    }

    #[test]
    fn compile_tools_registers_unbound_tool_without_agent_bindings() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let component = test_tool_component(
            "tools",
            BTreeMap::from([(
                tool_name.clone(),
                ToolDeploymentMetadata {
                    definition: test_tool(tool_name.as_str()),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::new(),
                },
            )]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(component.component_name.clone(), component)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let compiled = context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert!(errors.is_empty());
        assert!(warnings.is_empty());
        assert_eq!(compiled.registered_tools.len(), 1);
        assert_eq!(compiled.registered_tools[0].definition.name(), Some("grep"));
        assert!(compiled.agent_tool_bindings.is_empty());
    }

    #[test]
    fn compile_tools_registers_remote_source_with_consumer_provision_and_bindings() {
        let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
        let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
        let remote = test_remote_tool(
            "grep",
            Some(ToolBindingInput {
                parameters: NormalizedJsonValue::new(json!({ "scope": "environment" })),
                ..ToolBindingInput::default()
            }),
            BTreeMap::from([(
                agent_a_name.clone(),
                ToolBindingInput {
                    parameters: NormalizedJsonValue::new(json!({ "scope": "agent" })),
                    ..ToolBindingInput::default()
                },
            )]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::from([
                (agent_a_name.clone(), agent_a),
                (agent_b_name.clone(), agent_b),
            ]),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let compiled = context.compile_tools_with_remote(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            std::slice::from_ref(&remote),
            &mut errors,
            &mut warnings,
        );

        assert!(errors.is_empty());
        assert!(warnings.is_empty());
        assert!(context.components.is_empty());
        assert_eq!(compiled.registered_tools.len(), 1);
        let registered = &compiled.registered_tools[0];
        assert_eq!(
            registered.release_id,
            Some(remote.1.as_ref().unwrap().release.id)
        );
        assert_eq!(registered.source, remote.1.as_ref().unwrap().release.source);
        assert_eq!(registered.provision, remote.0.provision);
        assert_eq!(
            registered.owner_account_email.as_str(),
            "publisher@example.com"
        );
        assert_eq!(compiled.agent_tool_bindings.len(), 2);
        let bindings = compiled
            .agent_tool_bindings
            .iter()
            .map(|binding| {
                (
                    binding.agent_type_name.clone(),
                    binding.parameters.0.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(bindings[&agent_a_name], json!({ "scope": "agent" }));
        assert_eq!(bindings[&agent_b_name], json!({ "scope": "environment" }));

        let unbound = test_remote_tool("git", None, BTreeMap::new());
        let compiled = context.compile_tools_with_remote(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &[unbound],
            &mut Vec::new(),
            &mut Vec::new(),
        );
        assert_eq!(compiled.registered_tools.len(), 1);
        assert!(compiled.agent_tool_bindings.is_empty());
    }

    #[test]
    fn compile_tools_accumulates_remote_collisions_and_unavailable_references() {
        let grep = ToolName::try_from("grep").unwrap();
        let local = test_tool_component(
            "local-tools",
            BTreeMap::from([(
                grep.clone(),
                ToolDeploymentMetadata {
                    definition: test_tool(grep.as_str()),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::new(),
                },
            )]),
        );
        let mut unavailable_a = test_remote_tool("missing-a", None, BTreeMap::new());
        unavailable_a.1 = None;
        let mut unavailable_b = test_remote_tool("missing-b", None, BTreeMap::new());
        unavailable_b.1 = None;
        let remote_tools = vec![
            test_remote_tool("grep", None, BTreeMap::new()),
            test_remote_tool("git", None, BTreeMap::new()),
            test_remote_tool("git", None, BTreeMap::new()),
            unavailable_a,
            unavailable_b,
        ];
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(local.component_name.clone(), local)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();

        let compiled = context.compile_tools_with_remote(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &remote_tools,
            &mut errors,
            &mut Vec::new(),
        );

        assert!(compiled.registered_tools.is_empty());
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(error, DeployValidationError::ToolSourceCollision { .. }))
                .count(),
            2
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(
                    error,
                    DeployValidationError::RemoteToolUnavailable { .. }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn compile_tools_inherits_environment_tools_and_adds_agent_tools() {
        let grep = ToolName::try_from("grep").unwrap();
        let git = ToolName::try_from("git").unwrap();
        let (agent_a_name, agent_a) = test_registered_agent_type("AgentA");
        let (agent_b_name, agent_b) = test_registered_agent_type("AgentB");
        let component = test_tool_component(
            "tools",
            BTreeMap::from([
                (
                    grep.clone(),
                    ToolDeploymentMetadata {
                        definition: test_tool(grep.as_str()),
                        provision: ToolProvisionConfig::default(),
                        environment_binding: Some(ToolBindingInput::default()),
                        agent_bindings: BTreeMap::new(),
                    },
                ),
                (
                    git.clone(),
                    ToolDeploymentMetadata {
                        definition: test_tool(git.as_str()),
                        provision: ToolProvisionConfig::default(),
                        environment_binding: None,
                        agent_bindings: BTreeMap::from([(
                            agent_a_name.clone(),
                            ToolBindingInput::default(),
                        )]),
                    },
                ),
            ]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(component.component_name.clone(), component)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::from([
                (agent_a_name.clone(), agent_a),
                (agent_b_name.clone(), agent_b),
            ]),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let compiled = context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );
        let bindings = compiled
            .agent_tool_bindings
            .iter()
            .map(|binding| (binding.agent_type_name.clone(), binding.tool_name.clone()))
            .collect::<BTreeSet<_>>();

        assert!(errors.is_empty());
        assert!(warnings.is_empty());
        assert_eq!(compiled.registered_tools.len(), 2);
        assert_eq!(
            bindings,
            BTreeSet::from([
                (agent_a_name.clone(), grep.clone()),
                (agent_a_name, git),
                (agent_b_name, grep),
            ])
        );
    }

    #[test]
    fn compile_tools_accumulates_independent_binding_errors() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let (agent_name, agent_type) = test_registered_agent_type("AgentA");
        let invalid_binding = ToolBindingInput {
            version: Some("2.0.0".to_string()),
            parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
            account: Some(AccountEmail::new("other@example.com")),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        };
        let component = test_tool_component(
            "tools",
            BTreeMap::from([(
                tool_name.clone(),
                ToolDeploymentMetadata {
                    definition: test_tool(tool_name.as_str()),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: Some(invalid_binding.clone()),
                    agent_bindings: BTreeMap::from([(agent_name.clone(), invalid_binding)]),
                },
            )]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(component.component_name.clone(), component)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::from([(agent_name, agent_type)]),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let compiled = context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert_eq!(compiled.registered_tools.len(), 1);
        assert!(compiled.agent_tool_bindings.is_empty());
        assert!(warnings.is_empty());
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(
                    error,
                    DeployValidationError::ToolBindingVersionMismatch { .. }
                ))
                .count(),
            2
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(
                    error,
                    DeployValidationError::ToolBindingAccountMismatch { .. }
                ))
                .count(),
            2
        );
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(
                    error,
                    DeployValidationError::ToolBindingParametersMustBeObject { .. }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn compile_tools_accumulates_binding_errors_for_unknown_agent() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let unknown_agent = AgentTypeName("MissingAgent".to_string());
        let invalid_binding = ToolBindingInput {
            version: Some("2.0.0".to_string()),
            parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
            account: Some(AccountEmail::new("other@example.com")),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        };
        let component = test_tool_component(
            "tools",
            BTreeMap::from([(
                tool_name,
                ToolDeploymentMetadata {
                    definition: test_tool("grep"),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
                },
            )]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(component.component_name.clone(), component)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert!(
            errors.iter().any(|error| matches!(
                error,
                DeployValidationError::ToolBindingUnknownAgent { .. }
            ))
        );
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingVersionMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingAccountMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingParametersMustBeObject { .. }
        )));
    }

    #[test]
    fn compile_tools_rejects_duplicate_implementations() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let metadata = ToolDeploymentMetadata {
            definition: test_tool(tool_name.as_str()),
            provision: ToolProvisionConfig::default(),
            environment_binding: None,
            agent_bindings: BTreeMap::new(),
        };
        let first = test_tool_component(
            "tools-a",
            BTreeMap::from([(tool_name.clone(), metadata.clone())]),
        );
        let second =
            test_tool_component("tools-b", BTreeMap::from([(tool_name.clone(), metadata)]));
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([
                (first.component_name.clone(), first),
                (second.component_name.clone(), second),
            ]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        let compiled = context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert!(compiled.registered_tools.is_empty());
        assert!(compiled.agent_tool_bindings.is_empty());
        assert!(warnings.is_empty());
        assert!(matches!(
            errors.as_slice(),
            [DeployValidationError::DuplicateToolImplementation {
                tool_name: duplicate,
                components,
            }] if duplicate == &tool_name && components.len() == 2
        ));
    }

    #[test]
    fn compile_tools_accumulates_binding_errors_for_duplicate_implementations() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let unknown_agent = AgentTypeName("MissingAgent".to_string());
        let invalid_binding = ToolBindingInput {
            version: Some("2.0.0".to_string()),
            parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
            account: Some(AccountEmail::new("other@example.com")),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        };
        let metadata = ToolDeploymentMetadata {
            definition: test_tool(tool_name.as_str()),
            provision: ToolProvisionConfig::default(),
            environment_binding: None,
            agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
        };
        let first = test_tool_component(
            "tools-a",
            BTreeMap::from([(tool_name.clone(), metadata.clone())]),
        );
        let second = test_tool_component("tools-b", BTreeMap::from([(tool_name, metadata)]));
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([
                (first.component_name.clone(), first),
                (second.component_name.clone(), second),
            ]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::DuplicateToolImplementation { .. }
        )));
        assert!(
            errors.iter().any(|error| matches!(
                error,
                DeployValidationError::ToolBindingUnknownAgent { .. }
            ))
        );
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingVersionMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingAccountMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingParametersMustBeObject { .. }
        )));
    }

    #[test]
    fn compile_tools_accumulates_binding_errors_for_name_mismatched_definition() {
        let tool_name = ToolName::try_from("grep").unwrap();
        let unknown_agent = AgentTypeName("MissingAgent".to_string());
        let invalid_binding = ToolBindingInput {
            version: Some("2.0.0".to_string()),
            parameters: NormalizedJsonValue::new(json!(["not", "an", "object"])),
            account: Some(AccountEmail::new("other@example.com")),
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        };
        let component = test_tool_component(
            "tools",
            BTreeMap::from([(
                tool_name,
                ToolDeploymentMetadata {
                    definition: test_tool("git"),
                    provision: ToolProvisionConfig::default(),
                    environment_binding: None,
                    agent_bindings: BTreeMap::from([(unknown_agent, invalid_binding)]),
                },
            )]),
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::from([(component.component_name.clone(), component)]),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types: HashMap::new(),
        };
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        context.compile_tools(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &mut errors,
            &mut warnings,
        );

        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolDefinitionNameMismatch { .. }
        )));
        assert!(
            errors.iter().any(|error| matches!(
                error,
                DeployValidationError::ToolBindingUnknownAgent { .. }
            ))
        );
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingVersionMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingAccountMismatch { .. }
        )));
        assert!(errors.iter().any(|error| matches!(
            error,
            DeployValidationError::ToolBindingParametersMustBeObject { .. }
        )));
    }

    #[test]
    fn compile_tool_binding_merges_parameters_and_narrows_revealable_secrets() {
        let readable_path = CanonicalAgentSecretPath(vec!["readable".to_string()]);
        let dropped_path = CanonicalAgentSecretPath(vec!["dropped".to_string()]);
        let environment = ToolBindingInput {
            version: None,
            parameters: NormalizedJsonValue::new(json!({
                "nested": { "environment": true },
                "environment": true
            })),
            account: None,
            secret_keys_readable: SecretKeyScope::Keys(BTreeSet::from([readable_path.clone()])),
            secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::from([
                readable_path.clone(),
                dropped_path,
            ])),
        };
        let agent = ToolBindingInput {
            version: None,
            parameters: NormalizedJsonValue::new(json!({
                "nested": { "agent": true },
                "agent": true
            })),
            account: None,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
        };
        let component = test_tool_component("tools", BTreeMap::new());
        let source = ToolSource::Component {
            component_id: component.id,
            component_revision: component.revision,
            component_name: component.component_name.clone(),
        };
        let mut warnings = Vec::new();

        let binding = compile_tool_binding(
            golem_common::model::deployment::DeploymentRevision::INITIAL,
            &AgentTypeName("AgentA".to_string()),
            &ToolName::try_from("grep").unwrap(),
            Some(&environment),
            Some(&agent),
            None,
            component.account_id,
            &component.account_email,
            source,
            "1.0.0",
            TOOL_METADATA_WIT_VERSION,
            Default::default(),
            &mut warnings,
        )
        .unwrap();

        assert_eq!(
            binding.parameters.0,
            json!({
                "nested": { "agent": true },
                "environment": true,
                "agent": true
            })
        );
        assert_eq!(
            binding.secret_keys_readable,
            SecretKeyScope::Keys(BTreeSet::from([readable_path.clone()]))
        );
        assert_eq!(
            binding.secret_keys_revealable,
            SecretKeyScope::Keys(BTreeSet::from([readable_path]))
        );
        assert!(matches!(
            warnings.as_slice(),
            [crate::services::deployment::DeployValidationWarning::ToolRevealableSecretKeysDropped(
                _
            )]
        ));
    }

    #[test]
    fn secret_default_plaintext_value_is_parsed_against_secret_inner_type() {
        let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
        let default = DeploymentAgentSecretDefault {
            path: AgentSecretPath(vec!["apiKey".to_string()]),
            secret_value: json!("s3cr3t"),
        };
        let schema = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        }));
        let schema = stored_agent_secret_schema(&path, &schema, &schema.root)
            .expect("secret<T> config declarations should be accepted");

        let parsed = parse_default_secret_value(&path, Some(&&default), &schema)
            .expect("plaintext defaults for secret<T> should be parsed as T");

        assert_eq!(parsed, Some(SchemaValue::String("s3cr3t".to_string())));
    }

    #[test]
    fn optional_secret_default_plaintext_value_is_parsed_against_secret_inner_type() {
        let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
        let default = DeploymentAgentSecretDefault {
            path: AgentSecretPath(vec!["apiKey".to_string()]),
            secret_value: json!("s3cr3t"),
        };
        let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        })));
        let schema = stored_agent_secret_schema(&path, &schema, &schema.root)
            .expect("option<secret<T>> config declarations should be accepted");

        let parsed = parse_default_secret_value(&path, Some(&&default), &schema)
            .expect("plaintext defaults for option<secret<T>> should be parsed as T");

        assert_eq!(parsed, Some(SchemaValue::String("s3cr3t".to_string())));
    }

    #[test]
    fn non_secret_config_declaration_is_rejected() {
        let agent_type_name = AgentTypeName("vault".to_string());
        let config_path = vec!["apiKey".to_string()];
        let agent_type = agent_type_with_secret_config(
            agent_type_name.clone(),
            config_path.clone(),
            SchemaType::string(),
        );
        let mut registered_agent_types = HashMap::new();
        registered_agent_types.insert(
            agent_type_name,
            InProgressDeployedRegisteredAgentType {
                agent_type,
                implemented_by: test_implementer(),
                webhook_domain_and_segments: None,
            },
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types,
        };
        let mut errors = Vec::new();

        let (creations, updates, replacements) = context
            .deployment_agent_secret_creations_and_updates(
                Vec::new(),
                Vec::new(),
                false,
                &mut errors,
            );

        assert!(creations.is_empty());
        assert!(updates.is_empty());
        assert!(replacements.is_empty());
        assert_eq!(
            errors,
            vec![DeployValidationError::AgentSecretInvalidConfigType {
                path: CanonicalAgentSecretPath::from_path_in_unknown_casing(&config_path),
            }]
        );
    }

    #[test]
    fn nested_secret_payload_config_declaration_is_rejected() {
        let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
        let schema = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: None,
            })),
            category: None,
        }));

        let result = stored_agent_secret_schema(&path, &schema, &schema.root);

        assert_eq!(
            result,
            Err(DeployValidationError::AgentSecretInvalidConfigType { path })
        );
    }

    #[test]
    fn nested_quota_token_payload_config_declaration_is_rejected() {
        let path = CanonicalAgentSecretPath(vec!["quota".to_string()]);
        let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::quota_token(QuotaTokenSpec {
                resource_name: Some("credits".to_string()),
            })),
            category: None,
        })));

        let result = stored_agent_secret_schema(&path, &schema, &schema.root);

        assert_eq!(
            result,
            Err(DeployValidationError::AgentSecretInvalidConfigType { path })
        );
    }

    #[test]
    fn referenced_secret_declaration_projects_away_outer_secret_def() {
        let path = CanonicalAgentSecretPath(vec!["apiKey".to_string()]);
        let outer_id = TypeId::new("api-key-secret");
        let inner_id = TypeId::new("api-key-inner");
        let schema = SchemaGraph {
            defs: vec![
                SchemaTypeDef {
                    id: outer_id.clone(),
                    name: None,
                    body: SchemaType::secret(SecretSpec {
                        inner: Box::new(SchemaType::ref_to(inner_id.clone())),
                        category: None,
                    }),
                },
                SchemaTypeDef {
                    id: inner_id.clone(),
                    name: None,
                    body: SchemaType::string(),
                },
            ],
            root: SchemaType::string(),
        };

        let stored_schema =
            stored_agent_secret_schema(&path, &schema, &SchemaType::ref_to(outer_id))
                .expect("ref to secret<T> should be accepted and stored as plaintext T");

        assert!(matches!(stored_schema.root, SchemaType::Ref { .. }));
        assert_eq!(stored_schema.defs.len(), 1);
        assert_eq!(stored_schema.defs[0].id, inner_id);
        assert!(matches!(
            stored_schema.defs[0].body,
            SchemaType::String { .. }
        ));
    }

    #[test]
    fn optional_secret_default_creation_stores_plaintext_inner_schema_not_option_schema() {
        let agent_type_name = AgentTypeName("vault".to_string());
        let config_path = vec!["apiKey".to_string()];
        let agent_type = agent_type_with_secret_config(
            agent_type_name.clone(),
            config_path.clone(),
            SchemaType::option(SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: None,
            })),
        );
        let mut registered_agent_types = HashMap::new();
        registered_agent_types.insert(
            agent_type_name,
            InProgressDeployedRegisteredAgentType {
                agent_type,
                implemented_by: test_implementer(),
                webhook_domain_and_segments: None,
            },
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types,
        };
        let default = DeploymentAgentSecretDefault {
            path: AgentSecretPath(config_path),
            secret_value: json!("s3cr3t"),
        };
        let mut errors = Vec::new();

        let (creations, updates, replacements) = context
            .deployment_agent_secret_creations_and_updates(
                Vec::new(),
                vec![default],
                false,
                &mut errors,
            );

        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
        assert_eq!(updates.len(), 0);
        assert_eq!(replacements.len(), 0);
        assert_eq!(creations.len(), 1);
        assert_eq!(
            creations[0].secret_value,
            Some(SchemaValue::String("s3cr3t".to_string()))
        );
        match resolve_schema_ref(&creations[0].secret_type, &creations[0].secret_type.root) {
            SchemaType::String { .. } => {}
            other => panic!(
                "deployment-created agent secrets must be stored as plaintext T, not {other:?}"
            ),
        }
    }

    #[test]
    fn optional_secret_declaration_accepts_existing_stored_plaintext_schema() {
        let agent_type_name = AgentTypeName("vault".to_string());
        let config_path = vec!["apiKey".to_string()];
        let agent_type = agent_type_with_secret_config(
            agent_type_name.clone(),
            config_path.clone(),
            SchemaType::option(SchemaType::secret(SecretSpec {
                inner: Box::new(SchemaType::string()),
                category: None,
            })),
        );
        let mut registered_agent_types = HashMap::new();
        registered_agent_types.insert(
            agent_type_name,
            InProgressDeployedRegisteredAgentType {
                agent_type,
                implemented_by: test_implementer(),
                webhook_domain_and_segments: None,
            },
        );
        let context = DeploymentContext {
            environment: test_environment(),
            components: BTreeMap::new(),
            http_api_deployments: BTreeMap::new(),
            mcp_deployments: BTreeMap::new(),
            registered_agent_types,
        };
        let existing_secret = stored_agent_secret(
            &config_path,
            SchemaGraph::anonymous(SchemaType::string()),
            Some(SchemaValue::String("already-set".to_string())),
        );
        let mut errors = Vec::new();

        let (creations, updates, replacements) = context
            .deployment_agent_secret_creations_and_updates(
                vec![existing_secret],
                Vec::new(),
                false,
                &mut errors,
            );

        assert!(
            errors.is_empty(),
            "option<secret<T>> declarations should accept existing stored plaintext T; got {errors:?}"
        );
        assert_eq!(creations.len(), 0);
        assert_eq!(updates.len(), 0);
        assert_eq!(replacements.len(), 0);
    }
}
