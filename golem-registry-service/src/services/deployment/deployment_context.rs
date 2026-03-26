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
use crate::model::agent_secret::{DeploymentAgentSecretCreation, DeploymentAgentSecretUpdate};
use crate::model::api_definition::UnboundCompiledRoute;
use golem_common::base_model::account::AccountId;
use golem_common::model::agent::{
    AgentConfigSource, AgentType, AgentTypeName, DeployedRegisteredAgentType,
    RegisteredAgentTypeImplementer,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::DeploymentAgentSecretDefault;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_common::model::resource_definition::{
    ResourceDefinition, ResourceDefinitionCreation, ResourceName,
};
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_service_base::custom_api::SecuritySchemeDetails;
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::component::Component;
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use heck::ToKebabCase;
use std::collections::{BTreeMap, HashMap, HashSet, hash_map};

#[derive(Debug)]
pub struct InProgressDeployedRegisteredAgentType {
    pub agent_type: AgentType,
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

    pub fn hash(&self) -> diff::Hash {
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
        };
        diffable.hash()
    }

    pub fn compile_http_api_routes(
        &self,
        errors: &mut Vec<DeployValidationError>,
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
                );

                add_webhook_callback_routes(
                    deployment,
                    registered_agent_type,
                    &mut current_route_id,
                    &mut deployment_routes,
                );
            }

            add_openapi_spec_routes(
                &deployment.domain,
                &mut current_route_id,
                &mut deployment_routes,
            );

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
            let mut agent_type_implementers: golem_service_base::mcp::AgentTypeImplementers =
                HashMap::new();

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

                agent_type_implementers.insert(
                    agent_type.clone(),
                    (
                        registered_agent_type.implemented_by.component_id,
                        registered_agent_type.implemented_by.component_revision,
                    ),
                );

                if let Some(name) = &agent_options.security_scheme {
                    unique_scheme_names.insert(name);
                }
            }

            let security_scheme = if unique_scheme_names.len() > 1 {
                errors.push(
                    DeployValidationError::McpDeploymentConflictingSecuritySchemes {
                        mcp_deployment_domain: domain.clone(),
                    },
                );
                None
            } else if let Some(scheme_name) = unique_scheme_names.into_iter().next() {
                match security_schemes.get(scheme_name) {
                    Some(details) => Some(details.clone()),
                    None => {
                        errors.push(DeployValidationError::McpDeploymentUnknownSecurityScheme {
                            mcp_deployment_domain: domain.clone(),
                            security_scheme: scheme_name.clone(),
                        });
                        None
                    }
                }
            } else {
                None
            };

            let compiled_mcp = golem_service_base::mcp::CompiledMcp {
                account_id,
                environment_id: self.environment.id,
                deployment_revision,
                domain: domain.clone(),
                agent_type_implementers,
                security_scheme,
                registered_agent_types: Vec::new(),
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
        errors: &mut Vec<DeployValidationError>,
    ) -> (
        Vec<DeploymentAgentSecretCreation>,
        Vec<DeploymentAgentSecretUpdate>,
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
        let mut seen_secrets = HashMap::new();

        for agent_type in self.registered_agent_types.values() {
            for config in &agent_type.agent_type.config {
                if config.source != AgentConfigSource::Secret {
                    continue;
                }

                let canonical_agent_secret_path =
                    CanonicalAgentSecretPath::from_path_in_unknown_casing(&config.path);

                match seen_secrets.entry(canonical_agent_secret_path.clone()) {
                    hash_map::Entry::Vacant(e) => {
                        e.insert(config.value_type.clone());
                    }
                    hash_map::Entry::Occupied(e) => {
                        if *e.get() != config.value_type {
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
                    if environment_agent_secret_declaration.secret_type != config.value_type {
                        errors.push(
                            DeployValidationError::AgentSecretNotCompatibleWithEnvironmentSecret {
                                path: canonical_agent_secret_path.clone(),
                                agent_secret_type: config.value_type.clone(),
                                environment_secret_type: environment_agent_secret_declaration
                                    .secret_type
                                    .clone(),
                            },
                        );
                    }

                    // declaration exists in environment but has no value.
                    // if default was provided as part of deployment we can set it now.
                    if environment_agent_secret_declaration.secret_value.is_none() {
                        let agent_secret_default = defaults.get(&canonical_agent_secret_path);

                        let agent_secret_value = ok_or_continue!(
                            agent_secret_default
                                .map(|sd| ValueAndType::parse_with_type(
                                    &sd.secret_value,
                                    &config.value_type
                                ))
                                .transpose()
                                .map_err(|errors| {
                                    DeployValidationError::AgentSecretDefaultTypeMismatch {
                                        path: canonical_agent_secret_path,
                                        errors,
                                    }
                                }),
                            errors
                        )
                        .map(|vat| vat.value);

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
                        agent_secret_default
                            .map(|sd| ValueAndType::parse_with_type(
                                &sd.secret_value,
                                &config.value_type
                            ))
                            .transpose()
                            .map_err(|errors| {
                                DeployValidationError::AgentSecretDefaultTypeMismatch {
                                    path: canonical_agent_secret_path.clone(),
                                    errors,
                                }
                            }),
                        errors
                    )
                    .map(|vat| vat.value);

                    creations.push(DeploymentAgentSecretCreation {
                        path: canonical_agent_secret_path,
                        secret_type: config.value_type.clone(),
                        secret_value: agent_secret_value,
                    });
                }
            }
        }

        (creations, updates)
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

        if !router.add_route(method, compiled_route.path.clone(), ()) {
            errors.push(DeployValidationError::RouteIsAmbiguous {
                domain: domain.clone(),
                method: compiled_route.method.clone(),
                path: compiled_route.path.clone(),
            })
        }
    }
}
