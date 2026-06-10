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

//! Boundary adapter: legacy compiled-route schema → new schema model.
//!
//! The OpenAPI emitter is generated only from the persisted/compiled
//! [`RichCompiledRoute`]s available at route-resolution time. Those still
//! carry the legacy `DataSchema` / `AnalysedType` / reduced-parameter types
//! (they flip to the new schema model in Wave 3). This module is the **only**
//! part of the emitter that touches those legacy types: it lowers every
//! schema-bearing field of every route into the new model
//! (`SchemaType` / `SchemaGraph` plus small HTTP-shaped descriptors), so the
//! rest of the emitter renders directly from `SchemaType` with no legacy
//! references.
//!
//! All named composite types reached from any route's request body or
//! response are lowered through a single shared [`SchemaGraphBuilder`], so the
//! same named type used by multiple methods of an agent is deduplicated to one
//! `SchemaTypeDef` (and two distinct types sharing a name but with different
//! bodies are disambiguated). The resulting document-wide [`SchemaGraph`]
//! backs `components/schemas` emission.

use super::call_agent;
use golem_common::base_model::agent::{
    BinaryDescriptor, ComponentModelElementSchema, DataSchema, ElementSchema, NamedElementSchemas,
    TextDescriptor,
};
use golem_common::schema::adapters::{
    SchemaAdapterError, SchemaGraphBuilder, analysed_type_to_schema_type_inline,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::{BinaryRestrictions, SchemaType, TextRestrictions};
use golem_service_base::custom_api::{
    CallAgentBehaviour, PathSegment, QueryOrHeaderType, RequestBodySchema,
};
use golem_wasm::analysis::AnalysedType;

/// Schema-model view of an entire set of compiled routes, ready for the
/// OpenAPI emitter.
///
/// `graph` is the document-wide [`SchemaGraph`] holding every named composite
/// referenced by any route's request body or response. `per_route` is aligned
/// 1:1 with the input route slice.
pub struct DocumentSchema {
    pub graph: SchemaGraph,
    pub per_route: Vec<RouteSchema>,
}

/// Schema-model view of a single route.
pub struct RouteSchema {
    /// Request body schema model (applies to every route kind).
    pub request_body: RequestBodyModel,
    /// CallAgent-specific schema model; `None` for non-CallAgent routes
    /// (those carry no agent input/output schema).
    pub call_agent: Option<CallAgentRouteSchema>,
}

/// Schema-model view of a `CallAgent` route's parameters and response.
pub struct CallAgentRouteSchema {
    pub path_params: Vec<PathParamSchema>,
    pub query_params: Vec<NamedParamSchema>,
    pub header_params: Vec<NamedParamSchema>,
    pub response: ResponseModel,
}

/// A path parameter, with its inline scalar/enum schema.
pub struct PathParamSchema {
    pub name: String,
    pub is_catchall: bool,
    pub schema: SchemaType,
}

/// A query or header parameter, with its inline schema and HTTP-required flag
/// (`option<…>` parameters are not required).
pub struct NamedParamSchema {
    pub name: String,
    pub schema: SchemaType,
    pub required: bool,
}

/// Request body schema model. Mirrors [`RequestBodySchema`] but with the
/// JSON body lowered to a `SchemaType` (a `Ref` into the document graph when
/// the body is a named composite).
pub enum RequestBodyModel {
    Unused,
    Json(Box<SchemaType>),
    UnrestrictedBinary,
    RestrictedBinary { mime_types: Vec<String> },
    UnrestrictedText,
    RestrictedText { language_codes: Vec<String> },
}

/// Response schema model. The HTTP response policy (status-code split for
/// `option` / `result`, text/binary content types, etc.) is applied by the
/// emitter against the document graph; this only classifies the legacy
/// `DataSchema` into the cases the policy distinguishes.
pub enum ResponseModel {
    /// Empty output (0-element tuple) → `204 No Content`.
    Unit,
    /// A single typed output. The emitter resolves the `SchemaType` against
    /// the graph and applies the response policy (`option` → 200/404,
    /// `result` → 200/500, `text`/`binary` content types, otherwise JSON).
    Single(Box<SchemaType>),
    /// Opaque output (multimodal, or an unexpected multi-element tuple) →
    /// `200` with an arbitrary binary body, matching legacy behaviour.
    Unknown,
}

/// Lower every schema-bearing field of every route into the new schema model.
pub fn build_document_schema(
    routes: &[super::super::RichCompiledRoute],
) -> Result<DocumentSchema, SchemaAdapterError> {
    let mut builder = SchemaGraphBuilder::new();
    let mut per_route = Vec::with_capacity(routes.len());

    for route in routes {
        let request_body = lower_request_body(&mut builder, &route.body)?;
        let call_agent = match &route.behavior {
            super::super::RichRouteBehaviour::CallAgent(inner) => {
                Some(lower_call_agent(&mut builder, &route.path, inner)?)
            }
            _ => None,
        };
        per_route.push(RouteSchema {
            request_body,
            call_agent,
        });
    }

    // The graph carries a registry of named defs reachable from every route;
    // its `root` is unused (the emitter renders each route's individual roots
    // against this graph), so a placeholder is used here.
    let graph = builder.into_graph_with_root(SchemaType::bool());
    Ok(DocumentSchema { graph, per_route })
}

fn lower_request_body(
    builder: &mut SchemaGraphBuilder,
    body: &RequestBodySchema,
) -> Result<RequestBodyModel, SchemaAdapterError> {
    Ok(match body {
        RequestBodySchema::Unused => RequestBodyModel::Unused,
        RequestBodySchema::JsonBody { expected_type } => {
            RequestBodyModel::Json(Box::new(builder.lower(expected_type)?))
        }
        RequestBodySchema::UnrestrictedBinary => RequestBodyModel::UnrestrictedBinary,
        RequestBodySchema::RestrictedBinary { allowed_mime_types } => {
            RequestBodyModel::RestrictedBinary {
                mime_types: allowed_mime_types.clone(),
            }
        }
        RequestBodySchema::UnrestrictedText => RequestBodyModel::UnrestrictedText,
        RequestBodySchema::RestrictedText {
            allowed_language_codes,
        } => RequestBodyModel::RestrictedText {
            language_codes: allowed_language_codes.clone(),
        },
    })
}

fn lower_call_agent(
    builder: &mut SchemaGraphBuilder,
    path: &[PathSegment],
    inner: &CallAgentBehaviour,
) -> Result<CallAgentRouteSchema, SchemaAdapterError> {
    let path_params = call_agent::get_path_variables_and_types(
        path,
        &inner.constructor_parameters,
        &inner.method_parameters,
    )
    .into_iter()
    .map(|(name, is_catchall, pst)| {
        Ok(PathParamSchema {
            name: name.to_string(),
            is_catchall,
            schema: analysed_type_to_schema_type_inline(&AnalysedType::from(pst))?,
        })
    })
    .collect::<Result<Vec<_>, SchemaAdapterError>>()?;

    let query_params = call_agent::get_query_variable_and_types(&inner.method_parameters)
        .into_iter()
        .map(|(name, qoht)| lower_named_param(name, qoht))
        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;

    let header_params = call_agent::get_header_variable_and_types(&inner.method_parameters)
        .into_iter()
        .map(|(name, qoht)| lower_named_param(name, qoht))
        .collect::<Result<Vec<_>, SchemaAdapterError>>()?;

    let response = lower_response(builder, &inner.expected_agent_response)?;

    Ok(CallAgentRouteSchema {
        path_params,
        query_params,
        header_params,
        response,
    })
}

fn lower_named_param(
    name: &str,
    query_or_header_type: &QueryOrHeaderType,
) -> Result<NamedParamSchema, SchemaAdapterError> {
    // `option<…>` query/header parameters are optional; everything else is
    // required. This matches the legacy "required = !nullable" rule.
    let required = !matches!(query_or_header_type, QueryOrHeaderType::Option { .. });
    let schema =
        analysed_type_to_schema_type_inline(&AnalysedType::from(query_or_header_type.clone()))?;
    Ok(NamedParamSchema {
        name: name.to_string(),
        schema,
        required,
    })
}

fn lower_response(
    builder: &mut SchemaGraphBuilder,
    expected: &DataSchema,
) -> Result<ResponseModel, SchemaAdapterError> {
    match expected {
        DataSchema::Tuple(NamedElementSchemas { elements }) => match elements.as_slice() {
            [] => Ok(ResponseModel::Unit),
            [single] => lower_response_element(builder, &single.schema),
            // Agent methods only ever return 0 or 1 element at runtime; a
            // multi-element tuple is treated as opaque, as in the legacy
            // emitter.
            _ => Ok(ResponseModel::Unknown),
        },
        // Multimodal responses are rendered as opaque by the legacy emitter.
        DataSchema::Multimodal(_) => Ok(ResponseModel::Unknown),
    }
}

fn lower_response_element(
    builder: &mut SchemaGraphBuilder,
    element: &ElementSchema,
) -> Result<ResponseModel, SchemaAdapterError> {
    Ok(match element {
        ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) => {
            ResponseModel::Single(Box::new(builder.lower(element_type)?))
        }
        ElementSchema::UnstructuredText(TextDescriptor { restrictions }) => {
            let languages = restrictions
                .as_ref()
                .map(|r| r.iter().map(|t| t.language_code.clone()).collect());
            ResponseModel::Single(Box::new(SchemaType::text(TextRestrictions {
                languages,
                ..Default::default()
            })))
        }
        ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions }) => {
            let mime_types = restrictions
                .as_ref()
                .map(|r| r.iter().map(|t| t.mime_type.clone()).collect());
            ResponseModel::Single(Box::new(SchemaType::binary(BinaryRestrictions {
                mime_types,
                ..Default::default()
            })))
        }
    })
}
