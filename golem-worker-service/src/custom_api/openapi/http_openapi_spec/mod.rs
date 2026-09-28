// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

//! Emits the OpenAPI 3.1 document for a deployed HTTP API as a
//! [`serde_json::Value`].
//!
//! All request `SchemaType` rendering goes through [`render_input_schema`] (the Wave-1
//! renderer); named types lowered from the routes are emitted once into
//! `components/schemas`. The legacy compiled-route schema types are touched
//! only by the boundary adapter [`build_document_schema`].

use super::response_schema::{
    ResponseBodyOpenApiSchema, RouteResponseOpenApiSchema, get_route_response_schema,
};
use super::route_schema::{RequestBodyModel, RouteSchema, build_document_schema};
use super::schema_mapping::{
    arbitrary_binary_schema, render_input_schema, string_enum_schema, string_schema,
};
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour, RichRouteSecurity};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use golem_common::schema::graph::SchemaGraph;
use golem_service_base::custom_api::{AgentRouteMode, PathSegment, RouteMatch};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashSet};

mod durable_streams;

pub struct HttpApiOpenApiSpec;

impl HttpApiOpenApiSpec {
    #[cfg(test)]
    pub fn from_routes(
        routes: &[&RichCompiledRoute],
        public_origin: &str,
    ) -> Result<Value, String> {
        let spec = Self::contribution_from_routes(routes, public_origin)?;
        super::merge::merge(spec, vec![], public_origin)
            .map_err(|error| format!("{:?} at {}", error.category, error.location))
    }

    pub(super) fn contribution_from_routes(
        routes: &[&RichCompiledRoute],
        public_origin: &str,
    ) -> Result<Value, String> {
        let mut ds_only_paths: HashSet<_> = routes
            .iter()
            .flat_map(|route| match &route.behavior {
                RichRouteBehaviour::CallAgent(inner)
                    if inner.route_mode == AgentRouteMode::DurableStreams =>
                {
                    let mut paths = vec![route.path.clone()];
                    let path_variables = route
                        .path
                        .iter()
                        .filter(|segment| !matches!(segment, PathSegment::Literal { .. }))
                        .count();
                    if matches!(
                        route.path.as_slice(),
                        [.., PathSegment::Literal { value }, PathSegment::Variable { .. }]
                            if value == "streams"
                    ) && path_variables > inner.base_path_variables as usize
                        && let Some(policy) = &inner.durable_streams
                    {
                        paths.extend(policy.slots.iter().map(|slot| {
                            let mut path = route.path[..route.path.len() - 1].to_vec();
                            path.push(PathSegment::Literal {
                                value: slot.public_name.clone(),
                            });
                            path
                        }));
                    }
                    paths
                }
                _ => Vec::new(),
            })
            .collect();
        for route in routes {
            match &route.behavior {
                RichRouteBehaviour::CallAgent(inner)
                    if inner.route_mode == AgentRouteMode::DurableStreams => {}
                RichRouteBehaviour::CorsPreflight(_) => {}
                _ => {
                    ds_only_paths.remove(&route.path);
                }
            }
        }
        let routes: Vec<_> = routes
            .iter()
            .filter(|route| match &route.behavior {
                RichRouteBehaviour::CallAgent(inner) => {
                    inner.route_mode == AgentRouteMode::Rest
                        || (matches!(
                            route.route_match.method(),
                            Some(golem_common::model::agent::HttpMethod::Put(_))
                        ) && route
                            .path
                            .iter()
                            .filter(|segment| !matches!(segment, PathSegment::Literal { .. }))
                            .count()
                            == inner.base_path_variables as usize)
                }
                RichRouteBehaviour::CorsPreflight(_) => !ds_only_paths.contains(&route.path),
                _ => true,
            })
            .copied()
            .collect();
        let document = build_document_schema(&routes).map_err(|e| e.to_string())?;
        let graph = &document.graph;

        let mut component_schemas: Map<String, Value> = Map::new();
        let mut security_schemes: Map<String, Value> = Map::new();
        let mut paths: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        let mut binding_counts = BTreeMap::new();
        for route in &routes {
            if let RichRouteBehaviour::CallAgent(inner) = &route.behavior {
                *binding_counts
                    .entry((&inner.agent_type.0, &inner.method_name))
                    .or_insert(0usize) += 1;
            }
        }

        // Emit synthesized DS paths first. Explicit literal routes take
        // precedence over the compiled DS slot wildcard in the HTTP router.
        let mut ordered: Vec<_> = routes.iter().zip(document.per_route.iter()).collect();
        ordered.sort_by_key(|(_, schema)| {
            !schema
                .call_agent
                .as_ref()
                .is_some_and(|call| call.stream_slots.is_some())
        });
        for (route, route_schema) in ordered {
            let Some(method) = route.route_match.method() else {
                continue;
            };
            let method: http::Method = method
                .clone()
                .try_into()
                .map_err(|e| format!("Invalid route method: {e}"))?;
            if route_schema
                .call_agent
                .as_ref()
                .is_some_and(|call| call.stream_slots.is_some())
            {
                durable_streams::emit(
                    route,
                    route_schema,
                    graph,
                    &mut component_schemas,
                    &mut security_schemes,
                    &mut paths,
                )?;
                continue;
            }

            let mut operation =
                build_operation(route, route_schema, graph, &mut component_schemas)?;
            if let RichRouteBehaviour::CallAgent(inner) = &route.behavior
                && binding_counts[&(&inner.agent_type.0, &inner.method_name)] > 1
            {
                operation.as_object_mut().unwrap().remove("operationId");
            }
            operation["security"] = build_security(&route.security, &mut security_schemes)?;

            let mut path = render_full_path(&route.path);
            if matches!(
                route.route_match,
                RouteMatch::Method {
                    trailing_slash: true,
                    ..
                }
            ) {
                path.push('/');
            }
            let canonical = paths
                .iter()
                .find(|(candidate, item)| {
                    item.contains_key("x-golem-route-mode")
                        && candidate
                            .split('/')
                            .map(template_segment)
                            .eq(path.split('/').map(template_segment))
                })
                .map(|(candidate, _)| candidate.clone())
                .unwrap_or_else(|| path.clone());
            if canonical != path
                && let Some(parameters) = operation["parameters"].as_array_mut()
            {
                for parameter in parameters {
                    if parameter["in"] == "path" {
                        for (from, to) in path.split('/').zip(canonical.split('/')) {
                            if let (Some(from), Some(to)) = (capture_name(from), capture_name(to))
                                && parameter["name"] == from
                            {
                                parameter["name"] = json!(to);
                                break;
                            }
                        }
                    }
                }
            }
            let path_item = paths.entry(canonical).or_default();
            if path_item.contains_key("x-golem-route-mode") {
                operation["x-golem-route-mode"] = json!("rest");
            }
            insert_operation(path_item, method.as_str(), operation)?;
        }

        let mut components = Map::new();
        if !component_schemas.is_empty() {
            components.insert("schemas".to_string(), Value::Object(component_schemas));
        }
        if !security_schemes.is_empty() {
            components.insert(
                "securitySchemes".to_string(),
                Value::Object(security_schemes),
            );
        }

        let paths_value: Map<String, Value> = paths
            .into_iter()
            .map(|(k, v)| (k, Value::Object(v)))
            .collect();

        Ok(json!({
            "openapi": "3.1.0",
            "info": {
                "title": "Managed api provided by Golem",
                "version": "1.0.0",
            },
            "servers": [
                { "url": public_origin },
            ],
            "paths": Value::Object(paths_value),
            "components": Value::Object(components),
        }))
    }
}

fn build_operation(
    route: &RichCompiledRoute,
    route_schema: &RouteSchema,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
) -> Result<Value, String> {
    let mut operation = Map::new();

    if let RichRouteBehaviour::CallAgent(inner) = &route.behavior {
        operation.insert(
            "operationId".to_string(),
            json!(format!("{}-{}", inner.agent_type.0, inner.method_name)),
        );
        if let Some(description) = &inner.method_description {
            operation.insert("description".to_string(), json!(description));
        }
    }

    let mut parameters: Vec<Value> = Vec::new();
    add_route_parameters(route, route_schema, graph, components, &mut parameters)?;

    if let Some(request_body) = build_request_body(
        &route_schema.request_body,
        graph,
        components,
        &mut parameters,
    )? {
        operation.insert("requestBody".to_string(), request_body);
    }

    if !parameters.is_empty() {
        operation.insert("parameters".to_string(), Value::Array(parameters));
    }

    let response_model = get_route_response_schema(route, route_schema, graph, components)?;
    operation.insert("responses".to_string(), build_responses(response_model));

    Ok(Value::Object(operation))
}

fn add_route_parameters(
    route: &RichCompiledRoute,
    route_schema: &RouteSchema,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
    parameters: &mut Vec<Value>,
) -> Result<(), String> {
    match &route.behavior {
        RichRouteBehaviour::CallAgent(_) => {
            if let Some(call_agent) = &route_schema.call_agent {
                for param in &call_agent.path_params {
                    let mut schema = render_input_schema(graph, &param.schema, components)?;
                    if param.is_catchall {
                        set_schema_description(
                            &mut schema,
                            "Parameter represents the remaining path, including slashes.",
                        );
                    }
                    parameters.push(path_parameter(&param.name, schema));
                }
                for param in &call_agent.query_params {
                    let schema = render_input_schema(graph, &param.schema, components)?;
                    parameters.push(query_parameter(&param.name, param.required, schema));
                }
                for param in &call_agent.header_params {
                    let schema = render_input_schema(graph, &param.schema, components)?;
                    parameters.push(header_parameter(&param.name, param.required, schema));
                }
            }
        }
        RichRouteBehaviour::WebhookCallback(_) => {
            // Webhook callbacks carry a single fixed string path parameter (the
            // promise id); it is not an agent parameter, so it is emitted here
            // directly rather than via the schema model.
            for segment in &route.path {
                match segment {
                    PathSegment::Variable { display_name }
                    | PathSegment::CatchAll { display_name } => {
                        parameters.push(path_parameter(display_name, string_schema()));
                    }
                    PathSegment::Literal { .. } => {}
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn build_request_body(
    body: &RequestBodyModel,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
    parameters: &mut Vec<Value>,
) -> Result<Option<Value>, String> {
    Ok(match body {
        RequestBodyModel::Unused => None,
        RequestBodyModel::Json(ty) => {
            let schema = render_input_schema(graph, ty, components)?;
            Some(request_body_value(
                "JSON body",
                vec![("application/json".to_string(), schema)],
            ))
        }
        RequestBodyModel::UnrestrictedBinary => Some(request_body_value(
            "Unrestricted binary body",
            vec![("*/*".to_string(), arbitrary_binary_schema())],
        )),
        RequestBodyModel::RestrictedBinary { mime_types } => {
            let content = mime_types
                .iter()
                .map(|mime| (mime.clone(), arbitrary_binary_schema()))
                .collect();
            Some(request_body_value("Restricted binary body", content))
        }
        RequestBodyModel::UnrestrictedText => {
            parameters.push(header_parameter("Content-Language", false, string_schema()));
            Some(request_body_value(
                "Unrestricted text body",
                vec![("text/plain".to_string(), string_schema())],
            ))
        }
        RequestBodyModel::RestrictedText { language_codes } => {
            parameters.push(header_parameter(
                "Content-Language",
                false,
                string_enum_schema(language_codes),
            ));
            Some(request_body_value(
                "Restricted text body",
                vec![("text/plain".to_string(), string_schema())],
            ))
        }
    })
}

fn request_body_value(description: &str, content: Vec<(String, Value)>) -> Value {
    let content_map: Map<String, Value> = content
        .into_iter()
        .map(|(content_type, schema)| (content_type, json!({ "schema": schema })))
        .collect();
    json!({
        "description": description,
        "content": Value::Object(content_map),
        "required": true,
    })
}

fn build_responses(model: RouteResponseOpenApiSchema) -> Value {
    let headers_value = build_response_headers(&model);

    let mut responses = Map::new();
    for (status_code, body) in model.body_and_status_codes {
        let mut response = Map::new();
        response.insert(
            "description".to_string(),
            json!(format!("Response {status_code}")),
        );

        match body {
            ResponseBodyOpenApiSchema::Known {
                schema,
                content_type,
            } => {
                let mut content = Map::new();
                content.insert(content_type, json!({ "schema": schema }));
                response.insert("content".to_string(), Value::Object(content));
            }
            ResponseBodyOpenApiSchema::Unknown => {
                let mut content = Map::new();
                content.insert(
                    "*/*".to_string(),
                    json!({ "schema": arbitrary_binary_schema() }),
                );
                response.insert("content".to_string(), Value::Object(content));
            }
            ResponseBodyOpenApiSchema::NoBody => {}
        }

        if let Some(headers) = &headers_value {
            response.insert("headers".to_string(), headers.clone());
        }

        responses.insert(status_code.to_string(), Value::Object(response));
    }

    Value::Object(responses)
}

fn build_response_headers(model: &RouteResponseOpenApiSchema) -> Option<Value> {
    if model.headers.is_empty() {
        return None;
    }
    let mut headers = Map::new();
    for (name, header) in &model.headers {
        headers.insert(
            name.clone(),
            json!({
                "description": format!("Response header: {name}"),
                "style": "simple",
                "required": header.required,
                "schema": header.schema.clone(),
            }),
        );
    }
    Some(Value::Object(headers))
}

pub(super) fn build_security(
    security: &RichRouteSecurity,
    schemes: &mut Map<String, Value>,
) -> Result<Value, String> {
    let (name, scopes, definition) = match security {
        RichRouteSecurity::None => return Ok(json!([])),
        RichRouteSecurity::Unavailable => return Err("Unavailable route security".into()),
        RichRouteSecurity::SessionFromHeader(inner) => (
            format!(
                "golem-session-header-{}",
                URL_SAFE_NO_PAD.encode(inner.header_name.to_ascii_lowercase())
            ),
            vec![],
            json!({"type":"apiKey", "in":"header", "name":inner.header_name.to_ascii_lowercase()}),
        ),
        RichRouteSecurity::SecurityScheme(inner) => {
            let details = &inner.security_scheme;
            let issuer_url = details
                .provider_type
                .issuer_url()
                .map_err(|_| "Invalid OpenID issuer")?;
            (
                details.name.0.clone(),
                details
                    .scopes
                    .iter()
                    .map(|scope| scope.to_string())
                    .collect::<Vec<_>>(),
                json!({
                    "type":"openIdConnect",
                    "openIdConnectUrl":format!("{}/.well-known/openid-configuration", issuer_url.url().as_str().trim_end_matches('/')),
                    "description":format!("OpenID Connect provider for {}", details.name),
                }),
            )
        }
    };
    if let Some(previous) = schemes.get(&name) {
        if previous != &definition {
            return Err("Conflicting route security definitions".into());
        }
    } else {
        schemes.insert(name.clone(), definition);
    }
    Ok(json!([{name:scopes}]))
}

fn path_parameter(name: &str, schema: Value) -> Value {
    json!({
        "in": "path",
        "name": name,
        "description": format!("Path parameter: {name}"),
        "required": true,
        "schema": schema,
        "explode": false,
        "style": "simple",
    })
}

fn query_parameter(name: &str, required: bool, schema: Value) -> Value {
    json!({
        "in": "query",
        "name": name,
        "description": format!("Query parameter: {name}"),
        "required": required,
        "schema": schema,
        "explode": false,
        "style": "form",
        "allowEmptyValue": false,
    })
}

fn header_parameter(name: &str, required: bool, schema: Value) -> Value {
    json!({
        "in": "header",
        "name": name,
        "description": format!("Header parameter: {name}"),
        "required": required,
        "schema": schema,
        "explode": false,
        "style": "simple",
    })
}

fn set_schema_description(schema: &mut Value, description: &str) {
    if let Value::Object(obj) = schema {
        obj.insert("description".to_string(), json!(description));
    }
}

pub(super) fn render_full_path(path_segments: &[PathSegment]) -> String {
    let suffix = path_segments
        .iter()
        .map(|ps| match ps {
            PathSegment::Literal { value } => {
                let mut result = String::new();
                for byte in value.bytes() {
                    if byte.is_ascii_alphanumeric() || b"-._~!$&'()+,;=:@".contains(&byte) {
                        result.push(byte as char);
                    } else {
                        use std::fmt::Write;
                        write!(result, "%{byte:02X}").unwrap();
                    }
                }
                result
            }
            PathSegment::Variable { display_name } => format!("{{{display_name}}}"),
            // Note: same rendering as Variable on purpose. The difference is
            // only communicated through the parameter type in OpenAPI.
            PathSegment::CatchAll { display_name } => format!("{{{display_name}}}"),
        })
        .collect::<Vec<String>>()
        .join("/");
    format!("/{suffix}")
}

fn capture_name(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

fn template_segment(segment: &str) -> std::borrow::Cow<'_, str> {
    if capture_name(segment).is_some() {
        "{}".into()
    } else {
        urlencoding::decode(segment).unwrap_or_else(|_| segment.into())
    }
}

pub(super) fn insert_operation(
    path_item: &mut Map<String, Value>,
    method: &str,
    operation: Value,
) -> Result<(), String> {
    let key = match method {
        "GET" | "get" => "get",
        "POST" | "post" => "post",
        "PUT" | "put" => "put",
        "DELETE" | "delete" => "delete",
        "PATCH" | "patch" => "patch",
        "OPTIONS" | "options" => "options",
        "HEAD" | "head" => "head",
        "TRACE" | "trace" => "trace",
        _ => return Ok(()),
    };
    if path_item.contains_key(key)
        && !(path_item
            .get("x-golem-route-mode")
            .is_some_and(|mode| mode == "durable-streams")
            && path_item[key]["x-golem-route-mode"] != "rest"
            && operation["x-golem-route-mode"] == "rest")
    {
        return Err("Duplicate generated operation".into());
    }
    path_item.insert(key.to_string(), operation);
    Ok(())
}
