// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::DeploymentWriteError;
use super::http_parameter_conversion::{
    build_http_agent_constructor_parameters, build_http_agent_method_parameters,
};
use crate::model::api_definition::UnboundCompiledRoute;
use crate::model::component::Component;
use crate::services::deployment::write::DeployValidationError;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentMethod, AgentType, AgentTypeName, DataSchema, ElementSchema, HttpEndpointDetails,
    HttpMethod, HttpMountDetails, NamedElementSchemas, RegisteredAgentType,
    RegisteredAgentTypeImplementer, SystemVariable,
};
use golem_common::model::component::ComponentName;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_deployment::HttpApiDeployment;
use golem_service_base::custom_api::{ConstructorParameter, PathSegment, RouteBehaviour};
use itertools::Itertools;
use std::collections::{BTreeMap, HashMap};

macro_rules! ok_or_continue {
    ($expr:expr, $errors:ident) => {{
        match ($expr) {
            Ok(v) => v,
            Err(e) => {
                $errors.push(e);
                continue;
            }
        }
    }};
}

#[derive(Debug)]
pub struct DeploymentContext {
    pub components: BTreeMap<ComponentName, Component>,
    pub http_api_deployments: BTreeMap<Domain, HttpApiDeployment>,
}

impl DeploymentContext {
    pub fn new(components: Vec<Component>, http_api_deployments: Vec<HttpApiDeployment>) -> Self {
        Self {
            components: components
                .into_iter()
                .map(|c| (c.component_name.clone(), c))
                .collect(),
            http_api_deployments: http_api_deployments
                .into_iter()
                .map(|had| (had.domain.clone(), had))
                .collect(),
        }
    }

    pub fn hash(&self) -> diff::Hash {
        let diffable = diff::Deployment {
            components: self
                .components
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
            // Fixme: code-first routes
            http_api_definitions: BTreeMap::new(),
            http_api_deployments: self
                .http_api_deployments
                .iter()
                .map(|(k, v)| (k.0.clone(), HashOf::from_hash(v.hash)))
                .collect(),
        };
        diffable.hash()
    }

    pub fn extract_registered_agent_types(
        &self,
    ) -> Result<HashMap<AgentTypeName, RegisteredAgentType>, DeploymentWriteError> {
        let mut agent_types = HashMap::new();

        for component in self.components.values() {
            for agent_type in component.metadata.agent_types() {
                let agent_type_name = agent_type.type_name.to_wit_naming();
                let registered_agent_type = RegisteredAgentType {
                    agent_type: agent_type.clone(),
                    implemented_by: RegisteredAgentTypeImplementer {
                        component_id: component.id,
                        component_revision: component.revision,
                    },
                };

                if agent_types
                    .insert(agent_type_name, registered_agent_type)
                    .is_some()
                {
                    return Err(DeploymentWriteError::DeploymentValidationFailed(vec![
                        DeployValidationError::AmbiguousAgentTypeName(agent_type.type_name.clone()),
                    ]));
                };
            }
        }
        Ok(agent_types)
    }

    pub fn compile_http_api_routes(
        &self,
        registered_agent_types: &HashMap<AgentTypeName, RegisteredAgentType>,
    ) -> Result<Vec<UnboundCompiledRoute>, DeploymentWriteError> {
        let mut current_route_id: i32 = 0;
        let mut compiled_routes = Vec::new();
        let mut errors = Vec::new();

        for deployment in self.http_api_deployments.values() {
            for agent_type in &deployment.agent_types {
                let registered_agent_type = ok_or_continue!(
                    registered_agent_types.get(agent_type).ok_or(
                        DeployValidationError::HttpApiDeploymentMissingAgentType {
                            http_api_deployment_domain: deployment.domain.clone(),
                            missing_agent_type: agent_type.clone(),
                        }
                    ),
                    errors
                );

                if let Some(http_mount) = &registered_agent_type.agent_type.http_mount {
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

                    let mut compiled_agent_routes = self.compile_agent_methods_http_routes(
                        &mut current_route_id,
                        deployment,
                        &registered_agent_type.agent_type,
                        &registered_agent_type.implemented_by,
                        http_mount,
                        &registered_agent_type.agent_type.methods,
                        constructor_parameters,
                        &mut errors,
                    );

                    compiled_routes.append(&mut compiled_agent_routes);
                };
            }
        }

        // Fixme: code-first routes
        // * SwaggerUi and WebHook routes
        // * Validation of final router

        if !errors.is_empty() {
            return Err(DeploymentWriteError::DeploymentValidationFailed(errors));
        };

        Ok(compiled_routes)
    }

    fn compile_agent_methods_http_routes(
        &self,
        current_route_id: &mut i32,
        deployment: &HttpApiDeployment,
        agent: &AgentType,
        implementer: &RegisteredAgentTypeImplementer,
        http_mount: &HttpMountDetails,
        agent_methods: &[AgentMethod],
        constructor_parameters: Vec<ConstructorParameter>,
        errors: &mut Vec<DeployValidationError>,
    ) -> Vec<UnboundCompiledRoute> {
        let mut compiled_routes = Vec::new();

        for agent_method in agent_methods {
            for http_endpoint in &agent_method.http_endpoint {
                let make_route_validation_error = make_invalid_agent_route_error_maker(
                    deployment,
                    http_mount,
                    http_endpoint,
                    agent,
                    agent_method,
                );

                let cors = if !http_endpoint.cors_options.allowed_patterns.is_empty() {
                    http_endpoint.cors_options.clone()
                } else {
                    http_mount.cors_options.clone()
                };

                let route_id = *current_route_id;
                *current_route_id = current_route_id.checked_add(1).unwrap();

                ok_or_continue!(
                    validate_http_method_agent_response_type(
                        &agent_method.output_schema,
                        &make_route_validation_error
                    ),
                    errors
                );

                let (body, method_parameters) = ok_or_continue!(
                    build_http_agent_method_parameters(
                        http_mount,
                        http_endpoint,
                        &agent_method.input_schema,
                        &make_route_validation_error
                    ),
                    errors
                );

                let path = http_mount
                    .path_prefix
                    .iter()
                    .cloned()
                    .chain(http_endpoint.path_suffix.iter().cloned())
                    .map(|p| compile_agent_path_segment(agent, implementer, p))
                    .collect();

                let compiled = UnboundCompiledRoute {
                    route_id,
                    domain: deployment.domain.clone(),
                    method: http_endpoint.http_method.clone(),
                    path,
                    body,
                    behaviour: RouteBehaviour::CallAgent {
                        component_id: implementer.component_id,
                        component_revision: implementer.component_revision,
                        agent_type: agent.type_name.clone(),
                        method_name: agent_method.name.clone(),
                        phantom: http_mount.phantom_agent,
                        constructor_parameters: constructor_parameters.clone(),
                        method_parameters,
                        expected_agent_response: agent_method.output_schema.clone(),
                    },
                    security_scheme: None,
                    cors,
                };

                compiled_routes.push(compiled);
            }
        }

        compiled_routes
    }
}

fn validate_http_method_agent_response_type(
    schema: &DataSchema,
    make_error: &impl Fn(String) -> DeployValidationError,
) -> Result<(), DeployValidationError> {
    match schema {
        DataSchema::Multimodal(_) => Err(make_error(
            "Multimodal responses are not supported in http apis".into(),
        )),
        DataSchema::Tuple(NamedElementSchemas { elements }) => {
            match elements.len() {
                0 => {
                    // no-content response
                    Ok(())
                }
                1 => {
                    let element = elements.iter().next().unwrap();
                    match element.schema {
                        ElementSchema::ComponentModel(_) => {
                            // Json body response
                            Ok(())
                        }

                        ElementSchema::UnstructuredBinary(_) => {
                            // Full body taken from agent response
                            Ok(())
                        }

                        _ => Err(make_error(
                            "Unsupported return type from agent method".to_string(),
                        )),
                    }
                }
                n => Err(make_error(format!(
                    "Agent method should have 0 or 1 return values, found {n}"
                ))),
            }
        }
    }
}

fn make_invalid_agent_mount_error_maker(
    deployment: &HttpApiDeployment,
    http_mount: &HttpMountDetails,
    agent: &AgentType,
) -> impl Fn(String) -> DeployValidationError {
    let rendered_path: String = render_agent_http_path(http_mount.path_prefix.iter());
    move |msg: String| DeployValidationError::HttpApiDeploymentAgentConstructorInvalid {
        domain: deployment.domain.clone(),
        path: rendered_path.clone(),
        agent_type: agent.type_name.clone(),
        error: msg,
    }
}

fn make_invalid_agent_route_error_maker(
    deployment: &HttpApiDeployment,
    http_mount: &HttpMountDetails,
    http_endpoint: &HttpEndpointDetails,
    agent: &AgentType,
    agent_method: &AgentMethod,
) -> impl Fn(String) -> DeployValidationError {
    let rendered_method = match &http_endpoint.http_method {
        HttpMethod::Get(_) => "GET".to_string(),
        HttpMethod::Head(_) => "HEAD".to_string(),
        HttpMethod::Post(_) => "POST".to_string(),
        HttpMethod::Put(_) => "PUT".to_string(),
        HttpMethod::Delete(_) => "DELETE".to_string(),
        HttpMethod::Connect(_) => "CONNECT".to_string(),
        HttpMethod::Options(_) => "OPTIONS".to_string(),
        HttpMethod::Trace(_) => "TRACE".to_string(),
        HttpMethod::Patch(_) => "PATCH".to_string(),
        HttpMethod::Custom(custom) => custom.value.clone(),
    };

    let rendered_path: String = render_agent_http_path(
        http_mount
            .path_prefix
            .iter()
            .chain(http_endpoint.path_suffix.iter()),
    );

    move |msg: String| DeployValidationError::HttpApiDeploymentAgentMethodInvalid {
        domain: deployment.domain.clone(),
        method: rendered_method.clone(),
        path: rendered_path.clone(),
        agent_type: agent.type_name.clone(),
        agent_method: agent_method.name.to_string(),
        error: msg,
    }
}

fn render_agent_http_path<'a>(
    path: impl Iterator<Item = &'a golem_common::model::agent::PathSegment>,
) -> String {
    use golem_common::model::agent::{PathSegment, SystemVariable, SystemVariableSegment};
    path.map(|p| match p {
        PathSegment::Literal(v) => v.value.clone(),
        PathSegment::PathVariable(v) => {
            let name = &v.variable_name;
            format!("{{{name}}}")
        }
        PathSegment::RemainingPathVariable(v) => {
            let name = &v.variable_name;
            format!("{{{name}}}+")
        }
        PathSegment::SystemVariable(SystemVariableSegment {
            value: SystemVariable::AgentType,
        }) => "{agent-type}!".to_string(),
        PathSegment::SystemVariable(SystemVariableSegment {
            value: SystemVariable::AgentVersion,
        }) => "{agent-version}!".to_string(),
    })
    .join("/")
}

fn compile_agent_path_segment(
    agent: &AgentType,
    implementer: &RegisteredAgentTypeImplementer,
    path_segment: golem_common::model::agent::PathSegment,
) -> PathSegment {
    use golem_common::model::agent::PathSegment as AgentPathSegment;

    match path_segment {
        AgentPathSegment::Literal(lit) => PathSegment::Literal { value: lit.value },
        AgentPathSegment::PathVariable(_) => PathSegment::Variable,
        AgentPathSegment::RemainingPathVariable(_) => PathSegment::CatchAll,
        AgentPathSegment::SystemVariable(system_var) => {
            let literal = match system_var.value {
                SystemVariable::AgentType => agent.type_name.0.clone(),
                SystemVariable::AgentVersion => implementer.component_revision.get().to_string(),
            };
            PathSegment::Literal { value: literal }
        }
    }
}
