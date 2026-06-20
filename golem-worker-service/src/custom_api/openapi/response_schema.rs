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

//! HTTP response protocol policy for the OpenAPI emitter.
//!
//! The status-code / content-type policy lives here (not in `SchemaType`):
//! `unit` → 204, `option<T>` → 200 + 404, `result<ok, err>` → 200 / 500,
//! `Text` → `text/plain` + `Content-Language`, `Binary` → selected media type,
//! everything else → `application/json`. Schema bodies are rendered from the
//! schema model via [`render_schema`]; CORS / webhook / OpenAPI-spec / OIDC
//! routes carry no agent schema and produce fixed responses/headers.

use super::route_schema::{ResponseModel, RouteSchema};
use super::schema_mapping::{
    arbitrary_binary_schema, render_schema, string_enum_schema, string_schema,
};
use crate::custom_api::{RichCompiledRoute, RichRouteBehaviour};
use golem_common::base_model::agent::HttpMethod;
use golem_common::schema::unstructured::{UnstructuredKind, unstructured_kind};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};
use golem_service_base::custom_api::OpenApiSpecFormat;
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::HashSet;

pub enum ResponseBodyOpenApiSchema {
    /// Opaque body rendered as `*/*` arbitrary binary.
    Unknown,
    Known {
        schema: Value,
        content_type: String,
    },
    NoBody,
}

pub struct ResponseHeaderSchema {
    pub schema: Value,
    pub required: bool,
}

pub struct RouteResponseOpenApiSchema {
    pub body_and_status_codes: IndexMap<u16, ResponseBodyOpenApiSchema>,
    pub headers: IndexMap<String, ResponseHeaderSchema>,
}

/// Compute the OpenAPI responses (and response headers) for a route.
///
/// `route_schema` carries the schema-model view of the route (produced by the
/// boundary adapter); `graph` is the document-wide schema graph and
/// `components` accumulates any `components/schemas` referenced by rendered
/// response bodies.
pub fn get_route_response_schema(
    route: &RichCompiledRoute,
    route_schema: &RouteSchema,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
) -> Result<RouteResponseOpenApiSchema, String> {
    let mut responses = IndexMap::new();
    let mut headers = IndexMap::new();

    match &route.behavior {
        RichRouteBehaviour::CallAgent(_) => {
            let response = route_schema.call_agent.as_ref().map(|ca| &ca.response);
            match response {
                Some(ResponseModel::Unit) => {
                    responses.insert(204, ResponseBodyOpenApiSchema::NoBody);
                }
                Some(ResponseModel::Single(ty)) => {
                    classify_single_response(ty, graph, components, &mut responses, &mut headers)?;
                }
                // `Unknown` (multimodal / unexpected multi-element tuple) or a
                // missing CallAgent model both map to an opaque 200 body, as in
                // the legacy emitter.
                Some(ResponseModel::Unknown) | None => {
                    responses.insert(200, ResponseBodyOpenApiSchema::Unknown);
                }
            }
        }
        RichRouteBehaviour::CorsPreflight(cors_preflight_behaviour) => {
            responses.insert(204, ResponseBodyOpenApiSchema::NoBody);

            for name in [
                "Access-Control-Allow-Origin",
                "Access-Control-Allow-Headers",
                "Access-Control-Allow-Credentials",
            ] {
                headers.insert(
                    name.to_string(),
                    ResponseHeaderSchema {
                        schema: string_schema(),
                        required: true,
                    },
                );
            }

            let allowed_methods: Vec<String> = cors_preflight_behaviour
                .method_policies
                .iter()
                .map(|policy| http_method_name(&policy.method))
                .collect();

            headers.insert(
                "Access-Control-Allow-Methods".to_string(),
                ResponseHeaderSchema {
                    schema: string_enum_schema(&allowed_methods),
                    required: true,
                },
            );
        }
        RichRouteBehaviour::WebhookCallback(_) => {
            responses.insert(204, ResponseBodyOpenApiSchema::NoBody);
            responses.insert(404, ResponseBodyOpenApiSchema::NoBody);
        }
        RichRouteBehaviour::OpenApiSpec(behaviour) => {
            let content_type = match behaviour.format {
                OpenApiSpecFormat::Json => "application/json".to_string(),
                OpenApiSpecFormat::Yaml => "application/yaml".to_string(),
            };
            responses.insert(
                200,
                ResponseBodyOpenApiSchema::Known {
                    schema: serde_json::json!({
                        "type": "object",
                        "additionalProperties": true,
                    }),
                    content_type,
                },
            );
        }
        RichRouteBehaviour::OidcCallback(_) => {
            // Preserves the legacy emitter behaviour: only response headers are
            // described and no status code is registered, so the rendered
            // operation ends up with an empty `responses` object (the headers
            // are attached per status code and there are none). This is an
            // existing quirk kept intact by the schema-model migration.
            headers.insert(
                "Set-Cookie".to_string(),
                ResponseHeaderSchema {
                    schema: string_schema(),
                    required: true,
                },
            );
            headers.insert(
                "Location".to_string(),
                ResponseHeaderSchema {
                    schema: string_schema(),
                    required: true,
                },
            );
        }
    }

    Ok(RouteResponseOpenApiSchema {
        body_and_status_codes: responses,
        headers,
    })
}

/// Apply the response protocol policy to a single typed output.
fn classify_single_response(
    ty: &SchemaType,
    graph: &SchemaGraph,
    components: &mut Map<String, Value>,
    responses: &mut IndexMap<u16, ResponseBodyOpenApiSchema>,
    headers: &mut IndexMap<String, ResponseHeaderSchema>,
) -> Result<(), String> {
    // A canonical unstructured text/binary output (`variant { inline, url }`)
    // describes its `inline` 200 body exactly like a bare `Text` / `Binary`
    // rich scalar; the `url` alternative is a 307 redirect applied at runtime
    // (see `response_mapping.rs`) and is not described here.
    match unstructured_kind(graph, ty).map_err(|err| err.to_string())? {
        Some(UnstructuredKind::Binary(restrictions)) => {
            insert_binary_response(restrictions, responses);
            return Ok(());
        }
        Some(UnstructuredKind::Text(restrictions)) => {
            insert_text_response(restrictions, responses, headers);
            return Ok(());
        }
        None => {}
    }

    match resolve_top_ref(graph, ty) {
        SchemaType::Option { inner, .. } => {
            let schema = render_schema(graph, inner, components)?;
            responses.insert(
                200,
                ResponseBodyOpenApiSchema::Known {
                    schema,
                    content_type: "application/json".to_string(),
                },
            );
            responses.insert(404, ResponseBodyOpenApiSchema::NoBody);
        }
        SchemaType::Result { spec, .. } => {
            match &spec.ok {
                Some(ok) => {
                    let schema = render_schema(graph, ok, components)?;
                    responses.insert(
                        200,
                        ResponseBodyOpenApiSchema::Known {
                            schema,
                            content_type: "application/json".to_string(),
                        },
                    );
                }
                None => {
                    // `result<(), E>` with a unit ok: success carries no
                    // payload, so 204 No Content must have no body.
                    responses.insert(204, ResponseBodyOpenApiSchema::NoBody);
                }
            }
            match &spec.err {
                Some(err) => {
                    let schema = render_schema(graph, err, components)?;
                    responses.insert(
                        500,
                        ResponseBodyOpenApiSchema::Known {
                            schema,
                            content_type: "application/json".to_string(),
                        },
                    );
                }
                None => {
                    // `result<T, ()>` with a unit err: the error carries no
                    // payload, so the 500 response has no body.
                    responses.insert(500, ResponseBodyOpenApiSchema::NoBody);
                }
            }
        }
        SchemaType::Binary { restrictions, .. } => {
            insert_binary_response(restrictions, responses);
        }
        SchemaType::Text { restrictions, .. } => {
            insert_text_response(restrictions, responses, headers);
        }
        _ => {
            // Render the original `ty` (not the ref-resolved body) so a named
            // type keeps its `$ref` and is emitted once into components.
            let schema = render_schema(graph, ty, components)?;
            responses.insert(
                200,
                ResponseBodyOpenApiSchema::Known {
                    schema,
                    content_type: "application/json".to_string(),
                },
            );
        }
    }
    Ok(())
}

/// Describe a `binary` 200 response: the first allowed MIME type (or
/// `application/octet-stream` when unrestricted) carrying an arbitrary binary
/// body. Shared by bare `Binary` outputs and the inline case of the canonical
/// unstructured-binary wrapper.
fn insert_binary_response(
    restrictions: &BinaryRestrictions,
    responses: &mut IndexMap<u16, ResponseBodyOpenApiSchema>,
) {
    let content_type = restrictions
        .mime_types
        .as_ref()
        .and_then(|types| types.first())
        .cloned()
        .unwrap_or_else(|| "application/octet-stream".to_string());
    responses.insert(
        200,
        ResponseBodyOpenApiSchema::Known {
            schema: arbitrary_binary_schema(),
            content_type,
        },
    );
}

/// Describe a `text` 200 response: a `text/plain` body plus an optional
/// `Content-Language` header enumerating the allowed languages. Shared by bare
/// `Text` outputs and the inline case of the canonical unstructured-text
/// wrapper.
fn insert_text_response(
    restrictions: &TextRestrictions,
    responses: &mut IndexMap<u16, ResponseBodyOpenApiSchema>,
    headers: &mut IndexMap<String, ResponseHeaderSchema>,
) {
    responses.insert(
        200,
        ResponseBodyOpenApiSchema::Known {
            schema: string_schema(),
            content_type: "text/plain".to_string(),
        },
    );
    let languages = restrictions.languages.clone().unwrap_or_default();
    headers.insert(
        "Content-Language".to_string(),
        ResponseHeaderSchema {
            schema: string_enum_schema(&languages),
            required: false,
        },
    );
}

/// Follow a chain of `Ref`s against `graph` and return the first non-`Ref`
/// type (or the last `Ref` if it dangles / cycles). Top-level `option` /
/// `result` / `text` / `binary` outputs are anonymous in practice, but
/// resolving keeps the classifier robust to named aliases.
fn resolve_top_ref<'a>(graph: &'a SchemaGraph, ty: &'a SchemaType) -> &'a SchemaType {
    let mut current = ty;
    let mut visited: HashSet<_> = HashSet::new();
    while let SchemaType::Ref { id, .. } = current {
        if !visited.insert(id.clone()) {
            break;
        }
        match graph.lookup(id) {
            Some(def) => current = &def.body,
            None => break,
        }
    }
    current
}

fn http_method_name(method: &HttpMethod) -> String {
    match method {
        HttpMethod::Get(_) => "GET".to_string(),
        HttpMethod::Head(_) => "HEAD".to_string(),
        HttpMethod::Post(_) => "POST".to_string(),
        HttpMethod::Put(_) => "PUT".to_string(),
        HttpMethod::Delete(_) => "DELETE".to_string(),
        HttpMethod::Connect(_) => "CONNECT".to_string(),
        HttpMethod::Options(_) => "OPTIONS".to_string(),
        HttpMethod::Trace(_) => "TRACE".to_string(),
        HttpMethod::Patch(_) => "PATCH".to_string(),
        HttpMethod::Custom(custom) => custom.value.to_uppercase(),
    }
}
