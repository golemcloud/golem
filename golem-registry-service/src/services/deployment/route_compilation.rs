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
use golem_common::model::Empty;
use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    AgentMethod, AgentType, AgentTypeName, DataSchema, ElementSchema, HttpEndpointDetails,
    HttpMethod, HttpMountDetails, NamedElementSchemas,
    RegisteredAgentTypeImplementer, SystemVariable,
};
use golem_common::model::component::ComponentName;
use golem_common::model::diff::{self, HashOf, Hashable};
use golem_common::model::domain_registration::Domain;
use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentAgentOptions};
use golem_service_base::custom_api::{
    CallAgentBehaviour, ConstructorParameter, CorsOptions, CorsPreflightBehaviour, OriginPattern,
    PathSegment, RequestBodySchema, RouteBehaviour, WebhookCallbackBehaviour,
};
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use golem_common::model::agent::DeployedRegisteredAgentType;
use url::Url;
use crate::services::deployment::ok_or_continue;
use super::deployment_context::InProgressDeployedRegisteredAgentType;

pub fn add_agent_method_http_routes(
    deployment: &HttpApiDeployment,
    agent: &AgentType,
    implementer: &RegisteredAgentTypeImplementer,
    http_mount: &HttpMountDetails,
    agent_methods: &[AgentMethod],
    constructor_parameters: Vec<ConstructorParameter>,
    deployment_agent_options: &HttpApiDeploymentAgentOptions,
    current_route_id: &mut i32,
    compiled_routes: &mut HashMap<(HttpMethod, Vec<PathSegment>), UnboundCompiledRoute>,
    errors: &mut Vec<DeployValidationError>,
) {
    for agent_method in agent_methods {
        for http_endpoint in &agent_method.http_endpoint {
            let make_route_validation_error = make_invalid_agent_route_error_maker(
                deployment,
                http_mount,
                http_endpoint,
                agent,
                agent_method,
            );

            let mut cors = CorsOptions {
                allowed_patterns: vec![],
            };

            if !http_mount.cors_options.allowed_patterns.is_empty() {
                cors.allowed_patterns.extend(
                    http_mount
                        .cors_options
                        .allowed_patterns
                        .iter()
                        .cloned()
                        .map(OriginPattern),
                );
            }
            if !http_endpoint.cors_options.allowed_patterns.is_empty() {
                cors.allowed_patterns.extend(
                    http_endpoint
                        .cors_options
                        .allowed_patterns
                        .iter()
                        .cloned()
                        .map(OriginPattern),
                );
            }
            cors.allowed_patterns.sort();
            cors.allowed_patterns.dedup();

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

            let path_segments: Vec<PathSegment> = http_mount
                .path_prefix
                .iter()
                .chain(http_endpoint.path_suffix.iter())
                .map(|p| compile_agent_path_segment(agent, implementer, p))
                .collect();

            let mut auth_required = false;
            if let Some(auth_details) = &http_mount.auth_details {
                auth_required = auth_details.required;
            }
            if let Some(auth_details) = &http_endpoint.auth_details {
                auth_required = auth_details.required;
            }

            let security_scheme = if auth_required {
                let security_scheme = ok_or_continue!(
                    deployment_agent_options.security_scheme.clone().ok_or(
                        DeployValidationError::NoSecuritySchemeConfigured(
                            agent.type_name.clone()
                        )
                    ),
                    errors
                );

                Some(security_scheme)
            } else {
                None
            };

            // TODO: check whether a security scheme with this name currently exists in the environment
            // and emit a warning to the cli if it doesn't.

            let compiled = UnboundCompiledRoute {
                route_id,
                domain: deployment.domain.clone(),
                method: http_endpoint.http_method.clone(),
                path: path_segments.clone(),
                body,
                behaviour: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    component_id: implementer.component_id,
                    component_revision: implementer.component_revision,
                    agent_type: agent.type_name.clone(),
                    method_name: agent_method.name.clone(),
                    phantom: http_mount.phantom_agent,
                    constructor_parameters: constructor_parameters.clone(),
                    method_parameters,
                    expected_agent_response: agent_method.output_schema.clone(),
                }),
                security_scheme,
                cors,
            };

            {
                let key = (http_endpoint.http_method.clone(), path_segments);
                if let std::collections::hash_map::Entry::Vacant(e) = compiled_routes.entry(key)
                {
                    e.insert(compiled);
                } else {
                    errors.push(make_route_validation_error(
                        "Duplicate route detected".into(),
                    ));
                }
            }
        }
    }
}

pub fn add_cors_preflight_http_routes(
    deployment: &HttpApiDeployment,
    current_route_id: &mut i32,
    compiled_routes: &mut HashMap<(HttpMethod, Vec<PathSegment>), UnboundCompiledRoute>,
) {
    struct PreflightMapEntry {
        allowed_methods: BTreeSet<HttpMethod>,
        allowed_origins: BTreeSet<OriginPattern>,
    }

    impl PreflightMapEntry {
        fn new() -> Self {
            PreflightMapEntry {
                allowed_methods: BTreeSet::new(),
                allowed_origins: BTreeSet::new(),
            }
        }
    }

    let mut preflight_map: HashMap<Vec<PathSegment>, PreflightMapEntry> = HashMap::new();

    for (_, compiled_route) in compiled_routes.iter() {
        if !compiled_route.cors.allowed_patterns.is_empty() {
            let entry = preflight_map
                .entry(compiled_route.path.clone())
                .or_insert(PreflightMapEntry::new());

            entry
                .allowed_methods
                .insert(compiled_route.method.clone());
            for allowed_pattern in &compiled_route.cors.allowed_patterns {
                entry.allowed_origins.insert(allowed_pattern.clone());
            }
        }
    }

    // Generate synthetic OPTIONS routes for preflight requests
    for (
        path_segments,
        PreflightMapEntry {
            allowed_methods,
            allowed_origins,
        },
    ) in preflight_map
    {
        let key = (HttpMethod::Options(Empty {}), path_segments.clone());
        if compiled_routes.contains_key(&key) {
            // Skip synthetic OPTIONS if user already defined one
            // TODO: Emit to the cli as warning
            continue;
        }

        let route_id = *current_route_id;
        *current_route_id = current_route_id.checked_add(1).unwrap();

        compiled_routes.insert(
            key,
            UnboundCompiledRoute {
                route_id,
                domain: deployment.domain.clone(),
                method: HttpMethod::Options(Empty {}),
                path: path_segments,
                body: RequestBodySchema::Unused,
                behaviour: RouteBehaviour::CorsPreflight(CorsPreflightBehaviour {
                    allowed_origins,
                    allowed_methods,
                }),
                security_scheme: None,
                cors: CorsOptions {
                    allowed_patterns: vec![],
                },
            },
        );
    }
}

pub fn add_webhook_callback_routes(
    deployment: &HttpApiDeployment,
    agent_type: &InProgressDeployedRegisteredAgentType,
    current_route_id: &mut i32,
    compiled_routes: &mut HashMap<(HttpMethod, Vec<PathSegment>), UnboundCompiledRoute>,
) {
    if let Some((_, segments)) = &agent_type.webhook_domain_and_segments {
        let route_id = *current_route_id;
        *current_route_id = current_route_id.checked_add(1).unwrap();

        let mut typed_segments: Vec<PathSegment> = segments.iter().cloned().map(|value| PathSegment::Literal { value }).collect();
        // final segment for promise id
        typed_segments.push(PathSegment::Variable);

        let compiled = UnboundCompiledRoute {
            route_id,
            domain: deployment.domain.clone(),
            method: HttpMethod::Post(Empty {  }),
            path: typed_segments,
            body: RequestBodySchema::UnrestrictedBinary,
            behaviour: RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                component_id: agent_type.implemented_by.component_id,
            }),
            security_scheme: None,
            cors: CorsOptions { allowed_patterns: Vec::new() },
        };

        compiled_routes.insert(
            (compiled.method.clone(), compiled.path.clone()),
            compiled
        );
    }
}

pub fn build_agent_http_api_deployment_details(
    agent_type_name: &AgentTypeName,
    agent_type: &AgentType,
    implementer: &RegisteredAgentTypeImplementer,
    http_api_deployments: &BTreeMap<Domain, HttpApiDeployment>
) -> Result<Option<(Domain, Vec<String>)>, DeployValidationError> {
    let agent_http_api_deployments: Vec<(&Domain, &HttpApiDeployment)> = http_api_deployments.iter().filter(|(_, d)| d.agents.contains_key(agent_type_name)).collect();

    if agent_http_api_deployments.len() > 1 {
        return Err(DeployValidationError::HttpApiDeploymentMultipleDeploymentsForAgentType {
            agent_type: agent_type_name.clone(),
        })
    }

    let (domain, agent_http_api_deployment) = if let Some(v) = agent_http_api_deployments.iter().next() {
        *v
    } else {
        return Ok(None)
    };

    let agent_http_mount = if let Some(v) = &agent_type.http_mount {
        v
    } else {
        return Err(DeployValidationError::HttpApiDeploymentAgentTypeMissingHttpMount { agent_type: agent_type_name.clone() })
    };

    let agent_webhook_prefix: Vec<PathSegment> = parse_literal_only_path_segments(&agent_http_api_deployment.webhooks_url);
    let agent_webhook_suffix = agent_http_mount.webhook_suffix.iter().map(|s| compile_agent_path_segment(&agent_type, implementer, s));

    let agent_webhook = agent_webhook_prefix
        .into_iter()
        .chain(agent_webhook_suffix)
        .map(|segment| {
            match segment {
                PathSegment::Literal { value } => Ok(value),
                PathSegment::Variable { .. } | PathSegment::CatchAll => Err(DeployValidationError::HttpApiDeploymentInvalidAgentWebhookSegmentType { agent_type: agent_type_name.clone() })
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    // check final webhook url forms a valid url.
    {
        let mut url_to_validate = format!("http://{}/", domain.0);

        for segment in &agent_webhook {
            url_to_validate.push_str(segment);
            url_to_validate.push('/');
        }

        Url::parse(&url_to_validate).map_err(|_| {
            DeployValidationError::HttpApiDeploymentInvalidWebhookUrl {
                agent_type: agent_type_name.clone(),
                url: url_to_validate,
            }
        })?;
    }

    Ok(Some((domain.clone(), agent_webhook)))
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

pub fn make_invalid_agent_mount_error_maker(
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
    path_segment: &golem_common::model::agent::PathSegment,
) -> PathSegment {
    use golem_common::model::agent::PathSegment as AgentPathSegment;

    match path_segment {
        AgentPathSegment::Literal(lit) => PathSegment::Literal { value: lit.value.clone() },
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

fn parse_literal_only_path_segments(input: &str) -> Vec<PathSegment> {
    input
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|segment| PathSegment::Literal {
            value: segment.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn parses_mixed_segments_as_literals() {
        let result = parse_literal_only_path_segments("/foo/{id}/*/bar");

        let expected = vec![
            PathSegment::Literal { value: "foo".into() },
            PathSegment::Literal { value: "{id}".into() },
            PathSegment::Literal { value: "*".into() },
            PathSegment::Literal { value: "bar".into() },
        ];

        assert_eq!(result, expected);
    }

    #[test]
    fn ignores_leading_trailing_and_repeated_slashes() {
        let result = parse_literal_only_path_segments("///");

        let expected: Vec<PathSegment> = Vec::new();

        assert_eq!(result, expected);
    }

    #[test]
    fn parses_single_segment() {
        let result = parse_literal_only_path_segments("foo");

        let expected = vec![
            PathSegment::Literal { value: "foo".into() },
        ];

        assert_eq!(result, expected);
    }

    #[test]
    fn parses_path_without_leading_slash() {
        let result = parse_literal_only_path_segments("foo/bar");

        let expected = vec![
            PathSegment::Literal { value: "foo".into() },
            PathSegment::Literal { value: "bar".into() },
        ];

        assert_eq!(result, expected);
    }
}
