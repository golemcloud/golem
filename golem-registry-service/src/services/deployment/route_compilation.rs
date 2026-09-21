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
use super::DeployValidationWarning;
use super::deployment_context::InProgressDeployedRegisteredAgentType;
use super::http_parameter_conversion::build_http_agent_method_parameters;
use super::ok_or_continue;
use crate::model::api_definition::{
    UnboundCompiledRoute, UnboundRouteSecurity, UnboundSecuritySchemeRouteSecurity,
};
use golem_common::model::Empty;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, CachePolicy, HttpEndpointDetails, HttpMethod, HttpMountDetails,
    RegisteredAgentTypeImplementer, SystemVariable,
};
use golem_common::model::deployment::{
    HttpApiReadOnlyMethodBoundToNonGetVerb, HttpApiReadOnlyTtlBelowOneSecond,
};
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentAgentOptions, HttpApiDeploymentAgentSecurity,
};
use golem_common::schema::multimodal::is_multimodal_schema_type;
use golem_common::schema::{
    AgentMethodSchema, AgentTypeSchema, InputSchema, NamedFieldType, OutputSchema, SchemaGraph,
    SchemaType,
};
use golem_service_base::custom_api::{
    AgentFilesystemBehaviour, AgentRouteMode, CallAgentBehaviour, CompiledInputSchema,
    CompiledOutputSchema, ConstructorParameter, CorsOptions, CorsPreflightBehaviour,
    CorsPreflightMethodPolicy, HttpRouterBehaviour, OpenApiSpecBehaviour, OpenApiSpecFormat,
    OriginPattern, PathSegment, RequestBodySchema, RouteBehaviour, RouteMatch, RouterMethod,
    SessionFromHeaderRouteSecurity, WebhookCallbackBehaviour,
};
use heck::ToKebabCase;
use itertools::Itertools;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use url::Url;

/// Build a self-contained [`SchemaGraph`] whose `root` is `root` and whose
/// `defs` are the agent's shared named-type definitions, so any
/// [`SchemaType::Ref`] inside `root` resolves at runtime without an
/// `AgentTypeSchema` lookup.
fn graph_with_agent_defs(agent: &AgentTypeSchema, root: SchemaType) -> SchemaGraph {
    SchemaGraph {
        defs: agent.schema.defs.clone(),
        root,
    }
}

/// The record root describing the full positional input of a constructor or
/// method, including auto-injected fields (so the runtime can reconstruct the
/// complete `SchemaValue::Record` in declaration order).
fn input_record_root(input: &InputSchema) -> SchemaType {
    let fields = input
        .fields()
        .iter()
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    SchemaType::record(fields)
}

fn compiled_input(agent: &AgentTypeSchema, input: &InputSchema) -> CompiledInputSchema {
    CompiledInputSchema {
        graph: graph_with_agent_defs(agent, input_record_root(input)),
        input_schema: input.clone(),
    }
}

fn compiled_output(agent: &AgentTypeSchema, output: &OutputSchema) -> CompiledOutputSchema {
    let root = match output {
        OutputSchema::Unit => SchemaType::record(Vec::new()),
        OutputSchema::Single(ty) => (**ty).clone(),
    };
    CompiledOutputSchema {
        graph: graph_with_agent_defs(agent, root),
        output_schema: output.clone(),
    }
}

pub fn compile_fallback_mount(
    environment: &Environment,
    deployment: &HttpApiDeployment,
    agent: &AgentTypeSchema,
    implementer: &RegisteredAgentTypeImplementer,
    mount: &HttpMountDetails,
    constructor_parameters: Vec<ConstructorParameter>,
    options: &HttpApiDeploymentAgentOptions,
    route_id: i32,
) -> Result<Option<UnboundCompiledRoute>, DeployValidationError> {
    let invalid = make_invalid_agent_mount_error_maker(deployment, mount, agent);
    let behavior = if agent.kind == golem_common::schema::AgentTypeKind::HttpRouter {
        let compile_method = |method: &AgentMethodSchema| RouterMethod {
            method_name: method.name.clone(),
            input: compiled_input(agent, &method.input_schema),
            output: compiled_output(agent, &method.output_schema),
        };
        RouteBehaviour::HttpRouter(HttpRouterBehaviour {
            component_id: implementer.component_id,
            component_revision: implementer.component_revision,
            agent_type: agent.type_name.clone(),
            constructor_input: compiled_input(agent, &agent.constructor.input_schema),
            handler: agent
                .methods
                .iter()
                .find(|method| !method.http_endpoint.is_empty())
                .map(compile_method),
            openapi_provider_method: mount
                .openapi_provider_method
                .as_ref()
                .and_then(|name| agent.methods.iter().find(|method| &method.name == name))
                .map(compile_method),
            static_bindings: mount.static_bindings.clone(),
            file_index: Vec::new(),
        })
    } else if !mount.filesystem_bindings.is_empty() {
        let captures = mount
            .path_prefix
            .iter()
            .filter(|segment| {
                matches!(
                    segment,
                    golem_common::model::agent::PathSegment::PathVariable(_)
                )
            })
            .count();
        if captures != constructor_parameters.len() {
            return Err(invalid(
                "Filesystem mount captures must bind exactly the constructor parameters".into(),
            ));
        }
        RouteBehaviour::AgentFilesystem(AgentFilesystemBehaviour {
            component_id: implementer.component_id,
            component_revision: implementer.component_revision,
            agent_type: agent.type_name.clone(),
            constructor_input: compiled_input(agent, &agent.constructor.input_schema),
            constructor_parameters,
            filesystem_bindings: mount.filesystem_bindings.clone(),
        })
    } else {
        return Ok(None);
    };
    let path = mount
        .path_prefix
        .iter()
        .map(|segment| compile_agent_path_segment(agent, implementer, segment))
        .collect::<Vec<_>>();
    RouteMatch::MountPrefix
        .validate(&path, &behavior)
        .map_err(invalid)?;
    let security = resolve_route_security(environment, options, agent, mount, None)?;
    let mut allowed_patterns = mount
        .cors_options
        .allowed_patterns
        .iter()
        .cloned()
        .map(OriginPattern)
        .collect::<Vec<_>>();
    allowed_patterns.sort();
    allowed_patterns.dedup();
    Ok(Some(UnboundCompiledRoute {
        domain: deployment.domain.clone(),
        route_id,
        route_match: RouteMatch::MountPrefix,
        path,
        behaviour: behavior,
        security,
        cors: CorsOptions { allowed_patterns },
    }))
}

pub fn add_agent_method_http_routes(
    environment: &Environment,
    deployment: &HttpApiDeployment,
    agent: &AgentTypeSchema,
    implementer: &RegisteredAgentTypeImplementer,
    http_mount: &HttpMountDetails,
    agent_methods: &[AgentMethodSchema],
    constructor_parameters: Vec<ConstructorParameter>,
    deployment_agent_options: &HttpApiDeploymentAgentOptions,
    current_route_id: &mut i32,
    compiled_routes: &mut Vec<UnboundCompiledRoute>,
    errors: &mut Vec<DeployValidationError>,
    warnings: &mut Vec<DeployValidationWarning>,
) {
    let constructor_input = compiled_input(agent, &agent.constructor.input_schema);

    for agent_method in agent_methods {
        let route_mode = if agent_method.uses_streams(&agent.schema) {
            AgentRouteMode::DurableStreams
        } else {
            AgentRouteMode::Rest
        };
        collect_read_only_warnings(implementer, agent, http_mount, agent_method, warnings);

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
            *current_route_id = ok_or_continue!(
                current_route_id.checked_add(1).ok_or_else(|| {
                    make_route_validation_error("HTTP route ID capacity exceeded".into())
                }),
                errors
            );

            let http_input = if route_mode == AgentRouteMode::DurableStreams {
                ok_or_continue!(
                    super::durable_streams::validate_route(
                        agent,
                        agent_method,
                        http_mount,
                        http_endpoint
                    )
                    .map_err(&make_route_validation_error),
                    errors
                )
            } else {
                agent_method.input_schema.clone()
            };

            ok_or_continue!(
                validate_http_method_agent_response_type(
                    &agent.schema,
                    &agent_method.output_schema,
                    &make_route_validation_error
                ),
                errors
            );

            let (body, method_parameters) = ok_or_continue!(
                build_http_agent_method_parameters(
                    http_mount,
                    http_endpoint,
                    &agent.schema,
                    &http_input,
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
                    Some(http_endpoint),
                ),
                errors
            );

            let behaviour = CallAgentBehaviour {
                route_mode,
                base_path_variables: path_segments
                    .iter()
                    .filter(|segment| {
                        matches!(
                            segment,
                            PathSegment::Variable { .. } | PathSegment::CatchAll { .. }
                        )
                    })
                    .count() as u32,
                component_id: implementer.component_id,
                component_revision: implementer.component_revision,
                agent_type: agent.type_name.clone(),
                agent_mode: agent.mode,
                method_name: agent_method.name.clone(),
                phantom: http_mount.phantom_agent || agent.mode == AgentMode::Ephemeral,
                constructor_input: constructor_input.clone(),
                constructor_parameters: constructor_parameters.clone(),
                method_input: compiled_input(agent, &agent_method.input_schema),
                body,
                method_parameters,
                expected_agent_response: compiled_output(agent, &agent_method.output_schema),
                method_description: Some(agent_method.description.clone()),
                read_only: agent_method.read_only.clone(),
            };
            let compiled = UnboundCompiledRoute {
                route_id,
                domain: deployment.domain.clone(),
                route_match: http_endpoint.http_method.clone().into(),
                path: path_segments,
                behaviour: RouteBehaviour::CallAgent(behaviour.clone()),
                security,
                cors,
            };

            if route_mode == AgentRouteMode::DurableStreams {
                ok_or_continue!(
                    add_durable_stream_route_family(
                        compiled,
                        behaviour,
                        current_route_id,
                        compiled_routes,
                    )
                    .map_err(|error| make_route_validation_error(error.into())),
                    errors
                );
            } else {
                compiled_routes.push(compiled);
            }
        }
    }
}

fn add_durable_stream_route_family(
    mut base: UnboundCompiledRoute,
    behaviour: CallAgentBehaviour,
    current_route_id: &mut i32,
    routes: &mut Vec<UnboundCompiledRoute>,
) -> Result<(), &'static str> {
    base.route_match = HttpMethod::Put(Empty {}).into();
    let mut session_path = base.path.clone();
    session_path.push(PathSegment::Literal {
        value: "invocations".into(),
    });
    session_path.push(PathSegment::Variable {
        display_name: "session".into(),
    });
    let mut stream_path = session_path.clone();
    stream_path.push(PathSegment::Literal {
        value: "streams".into(),
    });
    stream_path.push(PathSegment::Variable {
        display_name: "slot".into(),
    });
    let endpoints = [
        (&session_path, HttpMethod::Put(Empty {})),
        (&session_path, HttpMethod::Head(Empty {})),
        (&session_path, HttpMethod::Get(Empty {})),
        (&session_path, HttpMethod::Delete(Empty {})),
        (&stream_path, HttpMethod::Put(Empty {})),
        (&stream_path, HttpMethod::Head(Empty {})),
        (&stream_path, HttpMethod::Get(Empty {})),
        (&stream_path, HttpMethod::Delete(Empty {})),
        (&stream_path, HttpMethod::Post(Empty {})),
    ];
    let base_endpoint_count = endpoints.len();
    const FORK_ENDPOINT_COUNT: usize = 7;
    let next_route_id = current_route_id
        .checked_add((base_endpoint_count + FORK_ENDPOINT_COUNT) as i32)
        .ok_or("HTTP route ID capacity exceeded")?;
    for (route_id, (path, method)) in (*current_route_id..).zip(endpoints) {
        routes.push(UnboundCompiledRoute {
            domain: base.domain.clone(),
            route_id,
            route_match: method.into(),
            path: path.clone(),
            behaviour: RouteBehaviour::CallAgent(behaviour.clone()),
            security: base.security.clone(),
            cors: base.cors.clone(),
        });
    }
    let mut fork_path = base.path.clone();
    fork_path.extend([
        PathSegment::Literal {
            value: "forks".into(),
        },
        PathSegment::Variable {
            display_name: "fork".into(),
        },
        PathSegment::Literal {
            value: "invocations".into(),
        },
        PathSegment::Variable {
            display_name: "session".into(),
        },
    ]);
    let mut fork_route_id = *current_route_id + base_endpoint_count as i32;
    for stream in [false, true] {
        if stream {
            fork_path.extend([
                PathSegment::Literal {
                    value: "streams".into(),
                },
                PathSegment::Variable {
                    display_name: "slot".into(),
                },
            ]);
        }
        let mut methods = vec![HttpMethod::Head(Empty {}), HttpMethod::Get(Empty {})];
        if stream {
            methods.extend([
                HttpMethod::Put(Empty {}),
                HttpMethod::Post(Empty {}),
                HttpMethod::Delete(Empty {}),
            ]);
        }
        for method in methods {
            routes.push(UnboundCompiledRoute {
                domain: base.domain.clone(),
                route_id: fork_route_id,
                route_match: method.into(),
                path: fork_path.clone(),
                behaviour: RouteBehaviour::CallAgent(behaviour.clone()),
                security: base.security.clone(),
                cors: base.cors.clone(),
            });
            fork_route_id += 1;
        }
    }
    *current_route_id = next_route_id;
    routes.push(base);
    Ok(())
}

/// Collects non-fatal warnings for a read-only `AgentMethod` and its HTTP
/// bindings. Currently produces:
/// - one warning per non-`GET`/`HEAD` HTTP binding (cache headers are only
///   emitted for cacheable HTTP methods),
/// - one warning when the method's `CachePolicy::Ttl` rounds down to zero
///   seconds (`max-age=0` would force revalidation every time).
fn collect_read_only_warnings(
    implementer: &RegisteredAgentTypeImplementer,
    agent: &AgentTypeSchema,
    http_mount: &HttpMountDetails,
    agent_method: &AgentMethodSchema,
    warnings: &mut Vec<DeployValidationWarning>,
) {
    let Some(read_only) = agent_method.read_only.as_ref() else {
        return;
    };

    if let CachePolicy::Ttl(ttl) = &read_only.cache_policy
        && ttl.duration_nanos < 1_000_000_000
    {
        warnings.push(DeployValidationWarning::HttpApiReadOnlyTtlBelowOneSecond(
            HttpApiReadOnlyTtlBelowOneSecond {
                component_id: implementer.component_id,
                agent_type: agent.type_name.clone(),
                method_name: agent_method.name.clone(),
                ttl_nanos: ttl.duration_nanos,
            },
        ));
    }

    // The whitelist below MUST stay consistent with the worker-service's
    // `is_cacheable_method` predicate
    // (`golem-worker-service/src/custom_api/call_agent/mod.rs`). If a future
    // HTTP method becomes cacheable on the worker-service side, this warning
    // must accept it too — otherwise users would be warned about a binding
    // that is actually fine.
    for http_endpoint in &agent_method.http_endpoint {
        if !matches!(
            http_endpoint.http_method,
            HttpMethod::Get(_) | HttpMethod::Head(_)
        ) {
            let rendered = render_agent_http_path(
                http_mount
                    .path_prefix
                    .iter()
                    .chain(http_endpoint.path_suffix.iter()),
            );
            let path = format!("/{rendered}");

            warnings.push(
                DeployValidationWarning::HttpApiReadOnlyMethodBoundToNonGetVerb(
                    HttpApiReadOnlyMethodBoundToNonGetVerb {
                        component_id: implementer.component_id,
                        agent_type: agent.type_name.clone(),
                        method_name: agent_method.name.clone(),
                        http_method: http_endpoint.http_method.clone(),
                        path,
                    },
                ),
            );
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
        let Some(method) = compiled_route.route_match.method() else {
            continue;
        };
        if !compiled_route.cors.allowed_patterns.is_empty() {
            let entry = preflight_map
                .entry(compiled_route.path.clone())
                .or_insert(PreflightMapEntry::new());

            let method_policy = entry
                .method_policies
                .entry(method.clone())
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

    // These entries project preflight policy into OpenAPI, not the runtime router.
    for (path_segments, PreflightMapEntry { method_policies }) in preflight_map {
        if compiled_routes.iter().any(|route| {
            route.path == path_segments
                && route
                    .route_match
                    .method()
                    .cloned()
                    .and_then(|m| http::Method::try_from(m).ok())
                    == Some(http::Method::OPTIONS)
        }) {
            continue;
        }
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
            route_match: HttpMethod::Options(Empty {}).into(),
            path: path_segments,
            behaviour: RouteBehaviour::CorsPreflight(CorsPreflightBehaviour { method_policies }),
            security: UnboundRouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: vec![],
            },
        });
    }
}

fn collect_allowed_request_headers(compiled_route: &UnboundCompiledRoute) -> BTreeSet<String> {
    let (body, parameters) = match &compiled_route.behaviour {
        RouteBehaviour::CallAgent(agent) => (&agent.body, agent.method_parameters.as_slice()),
        _ => (&RequestBodySchema::Unused, &[] as &[_]),
    };
    let session = match &compiled_route.security {
        UnboundRouteSecurity::SessionFromHeader(s) => Some(s.header_name.as_str()),
        _ => None,
    };
    let mut headers =
        golem_service_base::custom_api::cors_allowed_request_headers(body, parameters, session);
    if matches!(&compiled_route.behaviour, RouteBehaviour::CallAgent(agent)
        if agent.route_mode == AgentRouteMode::DurableStreams)
    {
        headers.extend(
            golem_service_base::custom_api::DURABLE_STREAM_REQUEST_HEADERS
                .iter()
                .map(|h| (*h).to_owned()),
        );
    }
    headers
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
            route_match: HttpMethod::Post(Empty {}).into(),
            path: typed_segments,
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
) -> Result<(), DeployValidationError> {
    let openapi_prefix = parse_literal_only_path_segments(&deployment.openapi_endpoint_prefix)
        .map_err(DeployValidationError::HttpApiDefinitionInvalidPathPattern)?;

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
            route_match: HttpMethod::Get(Empty {}).into(),
            path,
            behaviour: RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format,
                scheme: deployment.scheme,
            }),
            security: UnboundRouteSecurity::None,
            cors: CorsOptions {
                allowed_patterns: Vec::new(),
            },
        });
    }
    Ok(())
}

pub fn build_agent_http_api_deployment_details(
    agent_type_name: &AgentTypeName,
    agent_type: &AgentTypeSchema,
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
        parse_literal_only_path_segments(&agent_http_api_deployment.webhooks_prefix)
            .map_err(DeployValidationError::HttpApiDefinitionInvalidPathPattern)?;

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

    Ok(Some((domain.clone(), agent_webhook)))
}

fn validate_http_method_agent_response_type(
    graph: &SchemaGraph,
    schema: &OutputSchema,
    make_error: &impl Fn(String) -> DeployValidationError,
) -> Result<(), DeployValidationError> {
    match schema {
        // no-content response
        OutputSchema::Unit => Ok(()),
        OutputSchema::Single(ty) => {
            let multimodal = is_multimodal_schema_type(graph, ty)
                .map_err(|e| make_error(format!("Invalid output schema: {e}")))?;
            if multimodal {
                Err(make_error(
                    "Multimodal responses are not supported in http apis".into(),
                ))
            } else {
                // Json body response, or a full body taken from the agent
                // response (text/binary) — all handled by the runtime.
                Ok(())
            }
        }
    }
}

fn make_invalid_agent_route_error_maker(
    deployment: &HttpApiDeployment,
    http_mount: &HttpMountDetails,
    http_endpoint: &HttpEndpointDetails,
    agent: &AgentTypeSchema,
    agent_method: &AgentMethodSchema,
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
    agent: &AgentTypeSchema,
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
        HttpMethod::Any(_) => "<any>".to_string(),
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
    agent: &AgentTypeSchema,
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

fn parse_literal_only_path_segments(input: &str) -> Result<Vec<PathSegment>, String> {
    let target = golem_common::model::agent::http_files::HttpRequestTarget::parse(input)?;
    if target.query().is_some() {
        return Err("Configured HTTP prefix cannot contain a query".into());
    }
    Ok(target
        .segments()
        .iter()
        .map(|segment| PathSegment::Literal {
            value: segment.clone(),
        })
        .collect())
}

fn resolve_route_security(
    environment: &Environment,
    deployment_agent_options: &HttpApiDeploymentAgentOptions,
    agent: &AgentTypeSchema,
    http_mount: &HttpMountDetails,
    http_endpoint: Option<&HttpEndpointDetails>,
) -> Result<UnboundRouteSecurity, DeployValidationError> {
    let mut auth_required = false;

    if let Some(auth_details) = &http_mount.auth_details {
        auth_required = auth_details.required;
    }

    if let Some(auth_details) = http_endpoint.and_then(|endpoint| endpoint.auth_details.as_ref()) {
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

pub fn validate_path_segments(
    segments: &[PathSegment],
    domain: &Domain,
) -> Result<(), &'static str> {
    for segment in segments {
        if let PathSegment::Literal { value } = segment
            && !golem_common::model::agent::http_files::valid_decoded_segment(value)
        {
            return Err("Invalid decoded path segment");
        }
    }
    let url_to_validate = format!("http://{}/", domain.0);

    let Ok(url) = Url::parse(&url_to_validate) else {
        return Err("Does not form a valid url");
    };

    if url.query().is_some() || url.fragment().is_some() {
        return Err("may not contain query or fragment");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::api_definition::UnboundRouteSecurity;
    use chrono::Utc;
    use golem_common::model::Empty;
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::{
        AgentMode, CorsOptions as AgentCorsOptions, HttpMountDetails, LiteralSegment, Snapshotting,
    };
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::component::{ComponentId, ComponentRevision};
    use golem_common::model::diff::Hash;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{
        Environment, EnvironmentId, EnvironmentName, EnvironmentRevision,
    };
    use golem_common::model::http_api_deployment::{
        HttpApiDeployment, HttpApiDeploymentAgentOptions, HttpApiDeploymentId,
    };
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, OutputSchema,
    };
    use golem_service_base::custom_api::{CompiledSchema, MethodParameter};
    use std::collections::{BTreeMap, BTreeSet};
    use test_r::test;
    use uuid::Uuid;

    fn empty_compiled_input() -> CompiledInputSchema {
        CompiledInputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            input_schema: InputSchema::Parameters(vec![]),
        }
    }

    fn empty_compiled_output() -> CompiledOutputSchema {
        CompiledOutputSchema {
            graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
            output_schema: OutputSchema::Unit,
        }
    }

    fn test_environment(environment_id: EnvironmentId) -> Environment {
        Environment {
            id: environment_id,
            revision: EnvironmentRevision::INITIAL,
            application_id: ApplicationId(Uuid::new_v4()),
            application_name: ApplicationName::try_from("test-app").unwrap(),
            name: EnvironmentName::try_from("prod").unwrap(),
            diff_model_version: 0,
            compatibility_check: false,
            tool_compatibility_mode: Default::default(),
            version_check: false,
            security_overrides: false,
            owner_account_id: AccountId(Uuid::new_v4()),
            owner_account_email: golem_common::model::account::AccountEmail::new("test@golem"),
            current_deployment: None,
        }
    }

    fn test_agent(mode: AgentMode, phantom_agent: bool) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: AgentTypeName("note-agent".to_string()),
            kind: golem_common::schema::AgentTypeKind::Regular,
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods: vec![AgentMethodSchema {
                name: "fetch".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![HttpEndpointDetails {
                    http_method: HttpMethod::Get(Empty {}),
                    path_suffix: vec![],
                    header_vars: vec![],
                    query_vars: vec![],
                    auth_details: None,
                    cors_options: AgentCorsOptions {
                        allowed_patterns: vec![],
                    },
                }],
                read_only: None,
            }],
            dependencies: vec![],
            mode,
            http_mount: Some(HttpMountDetails {
                path_prefix: vec![golem_common::model::agent::PathSegment::Literal(
                    LiteralSegment {
                        value: "notes".to_string(),
                    },
                )],
                auth_details: None,
                phantom_agent,
                cors_options: AgentCorsOptions {
                    allowed_patterns: vec![],
                },
                webhook_suffix: vec![],
                static_bindings: vec![],
                filesystem_bindings: vec![],
                openapi_provider_method: None,
            }),
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn test_deployment(environment_id: EnvironmentId) -> HttpApiDeployment {
        HttpApiDeployment {
            scheme: Default::default(),
            id: HttpApiDeploymentId::new(),
            revision:
                golem_common::model::http_api_deployment::HttpApiDeploymentRevision::try_from(0u64)
                    .unwrap(),
            environment_id,
            domain: Domain("example.com".to_string()),
            hash: Hash::empty(),
            agents: BTreeMap::new(),
            webhooks_prefix: "/webhooks".to_string(),
            openapi_endpoint_prefix: "/".to_string(),
            created_at: Utc::now(),
        }
    }

    fn compiled_call_agent_behaviour(mode: AgentMode, phantom_agent: bool) -> CallAgentBehaviour {
        let (compiled_routes, errors) = compile_test_routes(&test_agent(mode, phantom_agent));
        assert!(errors.is_empty());
        let compiled_route = compiled_routes.into_iter().next().unwrap();
        let RouteBehaviour::CallAgent(call_agent) = compiled_route.behaviour else {
            panic!("expected call-agent route");
        };
        call_agent
    }

    fn compile_test_routes(
        agent: &AgentTypeSchema,
    ) -> (Vec<UnboundCompiledRoute>, Vec<DeployValidationError>) {
        compile_test_routes_from(agent, 0)
    }

    fn compile_test_routes_from(
        agent: &AgentTypeSchema,
        mut current_route_id: i32,
    ) -> (Vec<UnboundCompiledRoute>, Vec<DeployValidationError>) {
        let environment_id = EnvironmentId(Uuid::new_v4());
        let environment = test_environment(environment_id);
        let deployment = test_deployment(environment_id);
        let http_mount = agent.http_mount.clone().unwrap();
        let implementer = RegisteredAgentTypeImplementer {
            component_id: ComponentId(Uuid::new_v4()),
            component_revision: ComponentRevision::INITIAL,
            component_name: "test-component".to_string(),
            account_id: AccountId(Uuid::new_v4()),
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
        };
        let mut compiled_routes = Vec::new();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        add_agent_method_http_routes(
            &environment,
            &deployment,
            agent,
            &implementer,
            &http_mount,
            &agent.methods,
            vec![],
            &HttpApiDeploymentAgentOptions::default(),
            &mut current_route_id,
            &mut compiled_routes,
            &mut errors,
            &mut warnings,
        );

        assert!(warnings.is_empty());
        (compiled_routes, errors)
    }

    #[test]
    fn durable_stream_routes_register_family_and_roundtrip_mode() {
        use golem_common::schema::NamedField;
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].input_schema = InputSchema::parameters([
            NamedField::user_supplied("events", SchemaType::stream(Some(SchemaType::string()))),
            NamedField::user_supplied("limit", SchemaType::u32()),
        ]);
        let (routes, errors) = compile_test_routes(&agent);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(routes.len(), 17);
        let session = "notes/invocations/{session}";
        let slot = "notes/invocations/{session}/streams/{slot}";
        let fork_session = "notes/forks/{fork}/invocations/{session}";
        let fork_slot = "notes/forks/{fork}/invocations/{session}/streams/{slot}";
        assert_eq!(
            routes
                .iter()
                .map(|route| (
                    route.route_id,
                    render_http_method(route.route_match.method().unwrap()),
                    route.path.iter().map(ToString::to_string).join("/")
                ))
                .collect::<Vec<_>>(),
            [
                (1, "PUT", session),
                (2, "HEAD", session),
                (3, "GET", session),
                (4, "DELETE", session),
                (5, "PUT", slot),
                (6, "HEAD", slot),
                (7, "GET", slot),
                (8, "DELETE", slot),
                (9, "POST", slot),
                (10, "HEAD", fork_session),
                (11, "GET", fork_session),
                (12, "HEAD", fork_slot),
                (13, "GET", fork_slot),
                (14, "PUT", fork_slot),
                (15, "POST", fork_slot),
                (16, "DELETE", fork_slot),
                (0, "PUT", "notes"),
            ]
            .map(|(id, method, path)| (id, method.to_owned(), path.to_owned()))
        );
        let mut identities = BTreeSet::new();
        for route in routes {
            assert!(identities.insert((
                render_http_method(route.route_match.method().unwrap()),
                route.path.iter().map(ToString::to_string).join("/")
            )));
            let RouteBehaviour::CallAgent(ref call) = route.behaviour else {
                panic!()
            };
            assert_eq!(call.route_mode, AgentRouteMode::DurableStreams);
            assert_eq!(call.method_parameters.len(), 1);
            assert_eq!(call.method_input.input_schema.fields().len(), 2);
            let RequestBodySchema::JsonBody { ref expected } = call.body else {
                panic!()
            };
            assert_eq!(
                expected.graph.root,
                SchemaType::record(vec![NamedFieldType {
                    name: "limit".into(),
                    body: SchemaType::u32(),
                    metadata: Default::default(),
                }])
            );
            let bytes = desert_rust::serialize(&route, Vec::new()).unwrap();
            let restored: UnboundCompiledRoute = desert_rust::deserialize(&bytes).unwrap();
            let proto: golem_api_grpc::proto::golem::customapi::RouteBehaviour =
                restored.behaviour.into();
            let restored: RouteBehaviour = proto.try_into().unwrap();
            let RouteBehaviour::CallAgent(call) = restored else {
                panic!()
            };
            assert_eq!(call.route_mode, AgentRouteMode::DurableStreams);
        }
        assert!(identities.contains(&("PUT".into(), "notes".into())));
        assert!(identities.contains(&(
            "POST".into(),
            "notes/invocations/{session}/streams/{slot}".into()
        )));
        assert!(identities.contains(&(
            "PUT".into(),
            "notes/forks/{fork}/invocations/{session}/streams/{slot}".into()
        )));
        assert!(!identities.contains(&("GET".into(), "notes".into())));
        let rest = compiled_call_agent_behaviour(AgentMode::Durable, false);
        assert_eq!(rest.route_mode, AgentRouteMode::Rest);
    }

    #[test]
    fn route_id_capacity_reports_validation_errors_without_partial_families() {
        let rest = test_agent(AgentMode::Durable, false);
        let mut streaming = rest.clone();
        streaming.methods[0].output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::string()))));

        for (agent, count) in [(&rest, 1), (&streaming, 17)] {
            let (routes, errors) = compile_test_routes_from(agent, i32::MAX - count);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(routes.len(), count as usize);
            assert_eq!(
                routes.iter().map(|route| route.route_id).max(),
                Some(i32::MAX - 1)
            );

            for start in [i32::MAX - count + 1, i32::MAX] {
                let (routes, errors) = compile_test_routes_from(agent, start);
                assert!(routes.is_empty());
                assert_eq!(errors.len(), 1);
                assert!(matches!(
                    &errors[0],
                    DeployValidationError::HttpApiDeploymentAgentMethodInvalid { error, .. }
                        if error == "HTTP route ID capacity exceeded"
                ));
            }
        }
    }

    #[test]
    fn durable_stream_base_capture_count_survives_route_family_and_serialization() {
        use golem_common::model::agent::{PathSegment as AgentPathSegment, PathVariable};
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::string()))));
        agent.http_mount.as_mut().unwrap().path_prefix = vec![
            AgentPathSegment::PathVariable(PathVariable {
                variable_name: "tenant".into(),
            }),
            AgentPathSegment::Literal(LiteralSegment {
                value: "invocations".into(),
            }),
            AgentPathSegment::PathVariable(PathVariable {
                variable_name: "session".into(),
            }),
        ];
        let (routes, errors) = compile_test_routes(&agent);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(routes.len(), 17);
        for route in routes {
            let headers = collect_allowed_request_headers(&route);
            assert_eq!(
                headers,
                golem_service_base::custom_api::DURABLE_STREAM_REQUEST_HEADERS
                    .iter()
                    .map(|h| (*h).to_owned())
                    .collect::<BTreeSet<_>>()
            );
            for header in [
                "stream-closed",
                "producer-id",
                "producer-epoch",
                "producer-seq",
            ] {
                assert!(headers.contains(header), "missing {header}");
            }
            let bytes = desert_rust::serialize(&route, Vec::new()).unwrap();
            let restored: UnboundCompiledRoute = desert_rust::deserialize(&bytes).unwrap();
            let proto: golem_api_grpc::proto::golem::customapi::RouteBehaviour =
                restored.behaviour.into();
            let RouteBehaviour::CallAgent(call) = RouteBehaviour::try_from(proto).unwrap() else {
                panic!()
            };
            assert_eq!(call.base_path_variables, 2);
        }
    }

    #[test]
    fn durable_stream_invalid_slot_reports_method_and_slot() {
        for name in ["invocations", "$result", "__ds"] {
            let mut agent = test_agent(AgentMode::Durable, false);
            agent.methods[0].input_schema =
                InputSchema::parameters([golem_common::schema::NamedField::user_supplied(
                    name,
                    SchemaType::stream(Some(SchemaType::string())),
                )]);
            let (routes, errors) = compile_test_routes(&agent);
            assert!(routes.is_empty());
            assert_eq!(errors.len(), 1);
            let error = format!("{:?}", errors[0]);
            assert!(error.contains("fetch"), "{error}");
            assert!(error.contains(name), "{error}");
        }
    }

    fn test_deployment_with_openapi(openapi_endpoint: &str) -> HttpApiDeployment {
        HttpApiDeployment {
            scheme: Default::default(),
            id: HttpApiDeploymentId::new(),
            revision:
                golem_common::model::http_api_deployment::HttpApiDeploymentRevision::try_from(0u64)
                    .unwrap(),
            environment_id: EnvironmentId(uuid::Uuid::nil()),
            domain: Domain("example.com".to_string()),
            hash: Hash::empty(),
            agents: BTreeMap::new(),
            webhooks_prefix: "/webhooks".to_string(),
            openapi_endpoint_prefix: openapi_endpoint.to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn parses_mixed_segments_as_literals() {
        let result = parse_literal_only_path_segments("/foo/%7Bid%7D/*/bar").unwrap();

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
    fn rejects_unsafe_configured_prefixes() {
        for path in [
            "///",
            "/a//b",
            "/a/%2f",
            "/a/%2e%2e",
            "/a?query",
            "/a#fragment",
            "/a/%zz",
        ] {
            assert!(parse_literal_only_path_segments(path).is_err(), "{path}");
        }
    }

    #[test]
    fn parses_single_segment() {
        let result = parse_literal_only_path_segments("/foo").unwrap();

        let expected = vec![PathSegment::Literal {
            value: "foo".into(),
        }];

        assert_eq!(result, expected);
    }

    #[test]
    fn configured_prefix_requires_absolute_path_and_decodes_once() {
        assert!(parse_literal_only_path_segments("foo/bar").is_err());
        assert_eq!(
            parse_literal_only_path_segments("/foo/%252e/").unwrap(),
            vec![
                PathSegment::Literal {
                    value: "foo".into()
                },
                PathSegment::Literal {
                    value: "%2e".into()
                }
            ]
        );
    }

    #[test]
    fn add_openapi_spec_routes_uses_custom_prefix_and_formats() {
        let mut deployment = test_deployment_with_openapi("/docs");
        deployment.scheme = golem_common::model::http_api_deployment::HttpApiDeploymentScheme::Http;
        let mut route_id = 1;
        let mut compiled_routes = Vec::new();

        add_openapi_spec_routes(&deployment, &mut route_id, &mut compiled_routes).unwrap();

        assert_eq!(compiled_routes.len(), 2);

        assert_eq!(
            compiled_routes[0].path,
            vec![
                PathSegment::Literal {
                    value: "docs".to_string(),
                },
                PathSegment::Literal {
                    value: "openapi.json".to_string(),
                }
            ]
        );
        assert!(matches!(
            &compiled_routes[0].behaviour,
            RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Json,
                scheme: golem_common::model::http_api_deployment::HttpApiDeploymentScheme::Http,
            })
        ));

        assert_eq!(
            compiled_routes[1].path,
            vec![
                PathSegment::Literal {
                    value: "docs".to_string(),
                },
                PathSegment::Literal {
                    value: "openapi.yaml".to_string(),
                }
            ]
        );
        assert!(matches!(
            &compiled_routes[1].behaviour,
            RouteBehaviour::OpenApiSpec(OpenApiSpecBehaviour {
                format: OpenApiSpecFormat::Yaml,
                scheme: golem_common::model::http_api_deployment::HttpApiDeploymentScheme::Http,
            })
        ));
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
                route_match: HttpMethod::Get(Empty {}).into(),
                path: path.clone(),
                behaviour: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    route_mode: AgentRouteMode::Rest,
                    base_path_variables: 0,
                    component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                    component_revision:
                        golem_common::model::component::ComponentRevision::try_from(0u64).unwrap(),
                    agent_type: AgentTypeName("note-agent".to_string()),
                    agent_mode: AgentMode::Durable,
                    constructor_input: empty_compiled_input(),
                    constructor_parameters: vec![],
                    phantom: false,
                    method_name: "list".to_string(),
                    method_input: empty_compiled_input(),
                    body: RequestBodySchema::Unused,
                    method_parameters: vec![MethodParameter::Header {
                        header_name: "X-List-Token".to_string(),
                        parameter_type:
                            golem_service_base::custom_api::QueryOrHeaderType::Primitive(
                                golem_service_base::custom_api::PathSegmentType::Str,
                            ),
                    }],
                    expected_agent_response: empty_compiled_output(),
                    method_description: None,
                    read_only: None,
                }),
                security: UnboundRouteSecurity::None,
                cors: CorsOptions {
                    allowed_patterns: vec![OriginPattern("https://public.example.com".to_string())],
                },
            },
            UnboundCompiledRoute {
                domain: Domain("example.com".to_string()),
                route_id: 2,
                route_match: HttpMethod::Post(Empty {}).into(),
                path: path.clone(),
                behaviour: RouteBehaviour::CallAgent(CallAgentBehaviour {
                    route_mode: AgentRouteMode::Rest,
                    base_path_variables: 0,
                    component_id: golem_common::model::component::ComponentId(uuid::Uuid::nil()),
                    component_revision:
                        golem_common::model::component::ComponentRevision::try_from(0u64).unwrap(),
                    agent_type: AgentTypeName("note-agent".to_string()),
                    agent_mode: AgentMode::Durable,
                    constructor_input: empty_compiled_input(),
                    constructor_parameters: vec![],
                    phantom: false,
                    method_name: "add".to_string(),
                    method_input: empty_compiled_input(),
                    body: RequestBodySchema::JsonBody {
                        expected: CompiledSchema {
                            graph: SchemaGraph::anonymous(SchemaType::string()),
                        },
                    },
                    method_parameters: vec![],
                    expected_agent_response: empty_compiled_output(),
                    method_description: None,
                    read_only: None,
                }),
                security: UnboundRouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                    header_name: "X-Session".to_string(),
                }),
                cors: CorsOptions {
                    allowed_patterns: vec![OriginPattern("https://admin.example.com".to_string())],
                },
            },
        ];

        let environment_id = EnvironmentId(Uuid::new_v4());
        let deployment = test_deployment(environment_id);

        let mut route_id = 3;
        add_cors_preflight_http_routes(&deployment, &mut route_id, &mut compiled_routes);

        let preflight = compiled_routes
            .iter()
            .find(|route| matches!(route.behaviour, RouteBehaviour::CorsPreflight(_)))
            .expect("expected generated preflight route");

        let RouteBehaviour::CorsPreflight(preflight) = &preflight.behaviour else {
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
        compiled_routes
            .retain(|route| !matches!(route.behaviour, RouteBehaviour::CorsPreflight(_)));
        for method in [
            HttpMethod::Options(Empty {}),
            HttpMethod::Custom(golem_common::model::agent::CustomHttpMethod {
                value: "OPTIONS".into(),
            }),
        ] {
            compiled_routes[0].route_match = method.into();
            add_cors_preflight_http_routes(&deployment, &mut route_id, &mut compiled_routes);
            assert_eq!(
                compiled_routes.len(),
                2,
                "Explicit OPTIONS must retain its OpenAPI operation"
            );
        }
    }

    #[test]
    fn ephemeral_agents_force_http_routes_to_be_phantom() {
        let behaviour = compiled_call_agent_behaviour(AgentMode::Ephemeral, false);
        assert!(behaviour.phantom);
        assert_eq!(behaviour.agent_mode, AgentMode::Ephemeral);
    }

    #[test]
    fn durable_agents_keep_explicit_http_phantom_flag() {
        let behaviour = compiled_call_agent_behaviour(AgentMode::Durable, true);
        assert!(behaviour.phantom);
        assert_eq!(behaviour.agent_mode, AgentMode::Durable);
    }

    fn run_route_compilation_for_warnings(agent: AgentTypeSchema) -> Vec<DeployValidationWarning> {
        let environment_id = EnvironmentId(Uuid::new_v4());
        let environment = test_environment(environment_id);
        let deployment = test_deployment(environment_id);
        let http_mount = agent.http_mount.clone().unwrap();
        let implementer = RegisteredAgentTypeImplementer {
            component_id: ComponentId(Uuid::new_v4()),
            component_revision: ComponentRevision::INITIAL,
            component_name: "test-component".to_string(),
            account_id: AccountId(Uuid::new_v4()),
            account_email: golem_common::model::account::AccountEmail::new("test@golem"),
        };
        let mut current_route_id = 0;
        let mut compiled_routes = Vec::new();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        add_agent_method_http_routes(
            &environment,
            &deployment,
            &agent,
            &implementer,
            &http_mount,
            &agent.methods,
            vec![],
            &HttpApiDeploymentAgentOptions::default(),
            &mut current_route_id,
            &mut compiled_routes,
            &mut errors,
            &mut warnings,
        );

        assert!(errors.is_empty());
        warnings
    }

    fn read_only_until_write() -> golem_common::model::agent::ReadOnlyConfig {
        golem_common::model::agent::ReadOnlyConfig {
            cache_policy: CachePolicy::UntilWrite(Empty {}),
            uses_principal: false,
        }
    }

    fn read_only_ttl(nanos: u64) -> golem_common::model::agent::ReadOnlyConfig {
        golem_common::model::agent::ReadOnlyConfig {
            cache_policy: CachePolicy::Ttl(golem_common::model::agent::CachePolicyTtl {
                duration_nanos: nanos,
            }),
            uses_principal: false,
        }
    }

    #[test]
    fn warning_emitted_for_read_only_method_bound_to_post() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].http_endpoint[0].http_method = HttpMethod::Post(Empty {});
        agent.methods[0].read_only = Some(read_only_until_write());

        let warnings = run_route_compilation_for_warnings(agent);

        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            DeployValidationWarning::HttpApiReadOnlyMethodBoundToNonGetVerb(_)
        ));
    }

    #[test]
    fn no_warning_for_read_only_method_bound_to_get() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].read_only = Some(read_only_until_write());

        let warnings = run_route_compilation_for_warnings(agent);

        assert!(warnings.is_empty());
    }

    #[test]
    fn warning_emitted_for_sub_second_ttl() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].read_only = Some(read_only_ttl(500_000_000));

        let warnings = run_route_compilation_for_warnings(agent);

        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            DeployValidationWarning::HttpApiReadOnlyTtlBelowOneSecond(_)
        ));
    }

    #[test]
    fn warning_emitted_for_zero_ttl() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].read_only = Some(read_only_ttl(0));

        let warnings = run_route_compilation_for_warnings(agent);

        assert_eq!(warnings.len(), 1);
        assert!(matches!(
            &warnings[0],
            DeployValidationWarning::HttpApiReadOnlyTtlBelowOneSecond(_)
        ));
    }

    #[test]
    fn no_warning_for_one_second_ttl() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].read_only = Some(read_only_ttl(1_000_000_000));

        let warnings = run_route_compilation_for_warnings(agent);

        assert!(warnings.is_empty());
    }

    #[test]
    fn no_warning_for_non_read_only_method_bound_to_post() {
        let mut agent = test_agent(AgentMode::Durable, false);
        agent.methods[0].http_endpoint[0].http_method = HttpMethod::Post(Empty {});

        let warnings = run_route_compilation_for_warnings(agent);

        assert!(warnings.is_empty());
    }
}
