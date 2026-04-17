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
use super::deployment_context::InProgressDeployedRegisteredAgentType;
use super::http_parameter_conversion::build_http_agent_method_parameters;
use super::ok_or_continue;
use crate::model::api_definition::{
    UnboundCompiledRoute, UnboundRouteSecurity, UnboundSecuritySchemeRouteSecurity,
};
use golem_common::model::Empty;
use golem_common::model::agent::{
    AgentMethod, AgentType, AgentTypeName, DataSchema, ElementSchema, HttpEndpointDetails,
    HttpMethod, HttpMountDetails, NamedElementSchemas, RegisteredAgentTypeImplementer,
    SystemVariable,
};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentAgentOptions, HttpApiDeploymentAgentSecurity,
};
use golem_service_base::custom_api::{
    CallAgentBehaviour, ConstructorParameter, CorsOptions, CorsPreflightBehaviour,
    CorsPreflightMethodPolicy, MethodParameter, OpenApiSpecBehaviour, OpenApiSpecFormat,
    OriginPattern, PathSegment, RequestBodySchema, RouteBehaviour,
    SessionFromHeaderRouteSecurity, WebhookCallbackBehaviour,
};
use heck::ToKebabCase;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use url::Url;

pub fn add_agent_method_http_routes(
    environment: &Environment,
    deployment: &HttpApiDeployment,
    agent: &AgentType,
    implementer: &RegisteredAgentTypeImplementer,
    http_mount: &HttpMountDetails,
    agent_methods: &[AgentMethod],
    constructor_parameters: Vec<ConstructorParameter>,
    deployment_agent_options: &HttpApiDeploymentAgentOptions,
    current_route_id: &mut i32,
    compiled_routes: &mut Vec<UnboundCompiledRoute>,
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

            let security = ok_or_continue!(
                resolve_route_security(
                    environment,
                    deployment_agent_options,
                    agent,
                    http_mount,
                    http_endpoint,
                ),
                errors
            );

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
                security,
                cors,
            };

            compiled_routes.push(compiled);
        }
    }
}

pub fn add_cors_preflight_http_routes(
    deployment: &HttpApiDeployment,
    current_route_id: &mut i32,
    compiled_routes: &mut Vec<UnboundCompiledRoute>,
) {
    struct PreflightMethodPolicyEntry {
        allowed_origins: BTreeSet<OriginPattern>,
        allowed_headers: BTreeSet<String>,
    }

    struct PreflightMapEntry {
        method_policies: BTreeMap<HttpMethod, PreflightMethodPolicyEntry>,
    }

    impl PreflightMapEntry {
        fn new() -> Self {
            PreflightMapEntry {
                method_policies: BTreeMap::new(),
            }
        }
    }

    let mut preflight_map: HashMap<Vec<PathSegment>, PreflightMapEntry> = HashMap::new();

    for compiled_route in compiled_routes.iter() {
        if !compiled_route.cors.allowed_patterns.is_empty() {
            let entry = preflight_map
                .entry(compiled_route.path.clone())
                .or_insert(PreflightMapEntry::new());

            let method_policy = entry
                .method_policies
                .entry(compiled_route.method.clone())
                .or_insert_with(|| PreflightMethodPolicyEntry {
                    allowed_origins: BTreeSet::new(),
                    allowed_headers: BTreeSet::new(),
                });

            method_policy
                .allowed_origins
                .extend(compiled_route.cors.allowed_patterns.iter().cloned());
            method_policy
                .allowed_headers
                .extend(collect_allowed_request_headers(compiled_route));
        }
    }

    // Generate synthetic OPTIONS routes for preflight requests
    for (path_segments, PreflightMapEntry { method_policies }) in preflight_map {
        let route_id = *current_route_id;
        *current_route_id = current_route_id.checked_add(1).unwrap();

        let method_policies = method_policies
            .into_iter()
            .map(|(method, policy)| CorsPreflightMethodPolicy {
                method,
                allowed_origins: policy.allowed_origins,
                allowed_headers: policy.allowed_headers,
            })
            .collect();

        compiled_routes.push(UnboundCompiledRoute {
            route_id,
            domain: deployment.domain.clone(),
            method: HttpMethod::Options(Empty {}),
            path: path_segments,
            body: RequestBodySchema::Unused,
            behaviour: RouteBehaviour::CorsPreflight(CorsPreflightBehaviour { method_policies }),
            security: UnboundRouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: vec![],
            },
        });
    }
}

fn collect_allowed_request_headers(compiled_route: &UnboundCompiledRoute) -> BTreeSet<String> {
    let mut headers = BTreeSet::new();

    if let RouteBehaviour::CallAgent(CallAgentBehaviour {
        method_parameters, ..
    }) = &compiled_route.behaviour
    {
        headers.extend(
            method_parameters
                .iter()
                .filter_map(|parameter| match parameter {
                    MethodParameter::Header { header_name, .. } => {
                        Some(normalize_header_name(header_name))
                    }
                    _ => None,
                }),
        );
    }

    if !matches!(compiled_route.body, RequestBodySchema::Unused) {
        headers.insert(http::header::CONTENT_TYPE.as_str().to_string());
    }

    if let UnboundRouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity { header_name }) =
        &compiled_route.security
    {
        headers.insert(normalize_header_name(header_name));
    }

    headers
}

fn normalize_header_name(header_name: &str) -> String {
    let trimmed = header_name.trim();

    http::HeaderName::from_bytes(trimmed.as_bytes())
        .map(|header_name| header_name.as_str().to_string())
        .unwrap_or_else(|_| trimmed.to_ascii_lowercase())
}

pub fn add_webhook_callback_routes(
    deployment: &HttpApiDeployment,
    agent_type: &InProgressDeployedRegisteredAgentType,
    current_route_id: &mut i32,
    compiled_routes: &mut Vec<UnboundCompiledRoute>,
) {
    if let Some((_, segments)) = &agent_type.webhook_domain_and_segments {
        let route_id = *current_route_id;
        *current_route_id = current_route_id.checked_add(1).unwrap();

        let mut typed_segments: Vec<PathSegment> = segments
            .iter()
            .cloned()
            .map(|value| PathSegment::Literal { value })
            .collect();

        // final segment for promise id
        typed_segments.push(PathSegment::Variable {
            display_name: "promise-id".to_string(),
        });

        let compiled = UnboundCompiledRoute {
            route_id,
            domain: deployment.domain.clone(),
            method: HttpMethod::Post(Empty {}),
            path: typed_segments,
            body: RequestBodySchema::UnrestrictedBinary,
            behaviour: RouteBehaviour::WebhookCallback(WebhookCallbackBehaviour {
                component_id: agent_type.implemented_by.component_id,
            }),
            security: UnboundRouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: Vec::new(),
            },
        };

        compiled_routes.push(compiled);
    }
}

pub fn add_openapi_spec_routes(
    deployment: &HttpApiDeployment,
    current_route_id: &mut i32,
    compiled_routes: &mut Vec<UnboundCompiledRoute>,
    errors: &mut Vec<DeployValidationError>,
) {
    let openapi_prefix = match parse_openapi_endpoint_path_segments(deployment) {
        Ok(path) => path,
        Err(error) => {
            errors.push(error);
            return;
        }
    };

    for (format, openapi_path) in [
        (OpenApiSpecFormat::Json, "openapi.json"),
        (OpenApiSpecFormat::Yaml, "openapi.yaml"),
    ] {
        let route_id = *current_route_id;
        *current_route_id = current_route_id.checked_add(1).unwrap();

        let mut path = openapi_prefix.clone();
        path.push(PathSegment::Literal {
            value: openapi_path.to_string(),
        });

        compiled_routes.push(UnboundCompiledRoute {
            route_id,
            domain: deployment.domain.clone(),
            method: HttpMethod::Get(Empty {}),
            path,
            body: RequestBodySchema::Unused,
            behaviour: RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour { format }),
            security: UnboundRouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: Vec::new(),
            },
        });
    }
}

pub fn build_agent_http_api_deployment_details(
    agent_type_name: &AgentTypeName,
    agent_type: &AgentType,
    implementer: &RegisteredAgentTypeImplementer,
    http_api_deployments: &BTreeMap<Domain, HttpApiDeployment>,
) -> Result<Option<(Domain, Vec<String>)>, DeployValidationError> {
    let agent_http_api_deployments: Vec<(&Domain, &HttpApiDeployment)> = http_api_deployments
        .iter()
        .filter(|(_, d)| d.agents.contains_key(agent_type_name))
        .collect();

    if agent_http_api_deployments.len() > 1 {
        return Err(
            DeployValidationError::HttpApiDeploymentMultipleDeploymentsForAgentType {
                agent_type: agent_type_name.clone(),
            },
        );
    }

    let (domain, agent_http_api_deployment) = if let Some(v) = agent_http_api_deployments.first() {
        *v
    } else {
        return Ok(None);
    };

    let agent_http_mount = if let Some(v) = &agent_type.http_mount {
        v
    } else {
        return Err(
            DeployValidationError::HttpApiDeploymentAgentTypeMissingHttpMount {
                agent_type: agent_type_name.clone(),
            },
        );
    };

    let agent_webhook_prefix: Vec<PathSegment> =
        parse_literal_only_path_segments(&agent_http_api_deployment.webhooks_url);

    let mut agent_webhook_suffix: Vec<PathSegment> = agent_http_mount
        .webhook_suffix
        .iter()
        .map(|s| compile_agent_path_segment(agent_type, implementer, s))
        .collect();

    if agent_webhook_suffix.is_empty() {
        agent_webhook_suffix.push(PathSegment::Literal {
            value: agent_type_name.0.to_kebab_case(),
        });
    }

    let agent_webhook = agent_webhook_prefix
        .into_iter()
        .chain(agent_webhook_suffix)
        .map(|segment| match segment {
            PathSegment::Literal { value } => Ok(value),
            PathSegment::Variable { .. } | PathSegment::CatchAll { .. } => Err(
                DeployValidationError::HttpApiDeploymentInvalidAgentWebhookSegmentType {
                    agent_type: agent_type_name.clone(),
                },
            ),
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
    let rendered_method = render_http_method(&http_endpoint.http_method);

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

pub fn render_http_method(method: &HttpMethod) -> String {
    match &method {
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
        AgentPathSegment::Literal(inner) => PathSegment::Literal {
            value: inner.value.clone(),
        },
        AgentPathSegment::PathVariable(inner) => PathSegment::Variable {
            display_name: inner.variable_name.to_kebab_case(),
        },
        AgentPathSegment::RemainingPathVariable(inner) => PathSegment::CatchAll {
            display_name: inner.variable_name.to_kebab_case(),
        },
        AgentPathSegment::SystemVariable(system_var) => {
            let literal = match system_var.value {
                SystemVariable::AgentType => agent.type_name.0.to_kebab_case(),
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

fn parse_openapi_endpoint_path_segments(
    deployment: &HttpApiDeployment,
) -> Result<Vec<PathSegment>, DeployValidationError> {
    let Some(openapi_endpoint) = deployment.openapi_endpoint.as_deref() else {
        return Ok(Vec::new());
    };

    let make_error = |error: &str| DeployValidationError::HttpApiDeploymentInvalidOpenApiEndpoint {
        domain: deployment.domain.clone(),
        openapi_endpoint: openapi_endpoint.to_string(),
        error: error.to_string(),
    };

    if openapi_endpoint.contains('?') {
        return Err(make_error("query parameters are not allowed"));
    }

    if openapi_endpoint.contains('#') {
        return Err(make_error("fragments are not allowed"));
    }

    let trimmed = openapi_endpoint.trim_matches('/');

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.contains("//") {
        return Err(make_error("empty path segments are not allowed"));
    }

    trimmed
        .split('/')
        .map(|segment| {
            if segment == "openapi.json" || segment == "openapi.yaml" {
                Err(make_error(
                    "the prefix must not include openapi.json or openapi.yaml",
                ))
            } else if segment == "*" || (segment.starts_with('{') && segment.ends_with('}')) {
                Err(make_error("only literal path segments are allowed"))
            } else {
                Ok(PathSegment::Literal {
                    value: segment.to_string(),
                })
            }
        })
        .collect()
}

fn resolve_route_security(
    environment: &Environment,
    deployment_agent_options: &HttpApiDeploymentAgentOptions,
    agent: &AgentType,
    http_mount: &HttpMountDetails,
    http_endpoint: &HttpEndpointDetails,
) -> Result<UnboundRouteSecurity, DeployValidationError> {
    let mut auth_required = false;

    if let Some(auth_details) = &http_mount.auth_details {
        auth_required = auth_details.required;
    }

    if let Some(auth_details) = &http_endpoint.auth_details {
        auth_required = auth_details.required;
    }

    match (auth_required, &deployment_agent_options.security) {
        (true, Some(HttpApiDeploymentAgentSecurity::SecurityScheme(inner))) => {
            let security_scheme = inner.security_scheme.clone();

            // TODO: check whether a security scheme with this name currently exists in the environment
            // and emit a warning to the cli if it doesn't.

            Ok(UnboundRouteSecurity::SecurityScheme(
                UnboundSecuritySchemeRouteSecurity { security_scheme },
            ))
        }
        (true, Some(HttpApiDeploymentAgentSecurity::TestSessionHeader(inner))) => {
            if !environment.security_overrides {
                return Err(DeployValidationError::SecurityOverrideDisabled);
            }

            Ok(UnboundRouteSecurity::SessionFromHeader(
                SessionFromHeaderRouteSecurity {
                    header_name: inner.header_name.clone(),
                },
            ))
        }
        (true, None) => Err(DeployValidationError::NoSecuritySchemeConfigured(
            agent.type_name.clone(),
        )),
        (false, _) => Ok(UnboundRouteSecurity::None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::api_definition::UnboundRouteSecurity;
    use chrono::Utc;
    use golem_common::model::Empty;
    use golem_common::model::diff::Hash;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::http_api_deployment::{HttpApiDeployment, HttpApiDeploymentId};
    use std::collections::BTreeMap;
    use test_r::test;

    #[test]
    fn parses_mixed_segments_as_literals() {
        let result = parse_literal_only_path_segments("/foo/{id}/*/bar");

        let expected = vec![
            PathSegment::Literal {
                value: "foo".into(),
            },
            PathSegment::Literal {
                value: "{id}".into(),
            },
            PathSegment::Literal { value: "*".into() },
            PathSegment::Literal {
                value: "bar".into(),
            },
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

        let expected = vec![PathSegment::Literal {
            value: "foo".into(),
        }];

        assert_eq!(result, expected);
    }

    #[test]
    fn parses_path_without_leading_slash() {
        let result = parse_literal_only_path_segments("foo/bar");

        let expected = vec![
            PathSegment::Literal {
                value: "foo".into(),
            },
            PathSegment::Literal {
                value: "bar".into(),
            },
        ];

        assert_eq!(result, expected);
    }

    #[test]
    fn preflight_routes_keep_origins_and_headers_per_method() {
        let path = vec![PathSegment::Literal {
            value: "notes".to_string(),
        }];
        let mut compiled_routes = vec![
            UnboundCompiledRoute {
                domain: Domain("example.com".to_string()),
                route_id: 1,
                method: HttpMethod::Get(Empty {}),
                path: path.clone(),
                body: RequestBodySchema::Unused,
                behaviour: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                    component_revision:
                        golem_common::model::component::ComponentRevision::try_from(0u64).unwrap(),
                    agent_type: AgentTypeName("note-agent".to_string()),
                    constructor_parameters: vec![],
                    phantom: false,
                    method_name: "list".to_string(),
                    method_parameters: vec![MethodParameter::Header {
                        header_name: "X-List-Token".to_string(),
                        parameter_type:
                            golem_service_base::custom_api::QueryOrHeaderType::Primitive(
                                golem_service_base::custom_api::PathSegmentType::Str,
                            ),
                    }],
                    expected_agent_response: DataSchema::Tuple(NamedElementSchemas::empty()),
                }),
                security: UnboundRouteSecurity::None,
                cors: CorsOptions {
                    allowed_patterns: vec![OriginPattern("https://public.example.com".to_string())],
                },
            },
            UnboundCompiledRoute {
                domain: Domain("example.com".to_string()),
                route_id: 2,
                method: HttpMethod::Post(Empty {}),
                path: path.clone(),
                body: RequestBodySchema::JsonBody {
                    expected_type: golem_wasm::analysis::analysed_type::str(),
                },
                behaviour: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                    component_revision:
                        golem_common::model::component::ComponentRevision::try_from(0u64).unwrap(),
                    agent_type: AgentTypeName("note-agent".to_string()),
                    constructor_parameters: vec![],
                    phantom: false,
                    method_name: "add".to_string(),
                    method_parameters: vec![],
                    expected_agent_response: DataSchema::Tuple(NamedElementSchemas::empty()),
                }),
                security: UnboundRouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                    header_name: "X-Session".to_string(),
                }),
                cors: CorsOptions {
                    allowed_patterns: vec![OriginPattern("https://admin.example.com".to_string())],
                },
            },
        ];

        let deployment = HttpApiDeployment {
            id: HttpApiDeploymentId::new(),
            revision:
                golem_common::model::http_api_deployment::HttpApiDeploymentRevision::try_from(0u64)
                    .unwrap(),
            environment_id: EnvironmentId(uuid::Uuid::nil()),
            domain: Domain("example.com".to_string()),
            hash: Hash::empty(),
            agents: BTreeMap::new(),
            webhooks_url: "/webhooks".to_string(),
            created_at: Utc::now(),
        };

        let mut route_id = 3;
        add_cors_preflight_http_routes(&deployment, &mut route_id, &mut compiled_routes);

        let preflight = compiled_routes
            .into_iter()
            .find(|route| matches!(route.behaviour, RouteBehaviour::CorsPreflight(_)))
            .expect("expected generated preflight route");

        let RouteBehaviour::CorsPreflight(preflight) = preflight.behaviour else {
            panic!("expected preflight route");
        };

        assert_eq!(preflight.method_policies.len(), 2);

        let get_policy = preflight
            .method_policies
            .iter()
            .find(|policy| matches!(policy.method, HttpMethod::Get(_)))
            .expect("missing GET policy");
        assert_eq!(
            get_policy.allowed_origins,
            BTreeSet::from([OriginPattern("https://public.example.com".to_string())])
        );
        assert_eq!(
            get_policy.allowed_headers,
            BTreeSet::from(["x-list-token".to_string()])
        );

        let post_policy = preflight
            .method_policies
            .iter()
            .find(|policy| matches!(policy.method, HttpMethod::Post(_)))
            .expect("missing POST policy");
        assert_eq!(
            post_policy.allowed_origins,
            BTreeSet::from([OriginPattern("https://admin.example.com".to_string())])
        );
        assert_eq!(
            post_policy.allowed_headers,
            BTreeSet::from(["content-type".to_string(), "x-session".to_string(),])
        );
    }
}
