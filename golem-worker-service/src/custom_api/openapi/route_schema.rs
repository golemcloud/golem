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
use golem_common::schema::OutputSchema;
use golem_common::schema::adapters::{
    SchemaAdapterError, analysed_type_to_schema_type_inline, binary_body_restrictions,
    is_multimodal_schema_type, text_body_restrictions,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::merge_agent_graphs;
use golem_common::schema::schema_type::SchemaType;
use golem_service_base::custom_api::{
    CallAgentBehaviour, CompiledOutputSchema, PathSegment, QueryOrHeaderType, RequestBodySchema,
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
    let mut per_route = Vec::with_capacity(routes.len());
    // Every per-route compiled schema already carries the full per-agent
    // `SchemaGraph` defs; merging them deduplicates by `TypeId` into one
    // document-wide graph that backs `components/schemas`.
    let mut graphs: Vec<SchemaGraph> = Vec::new();

    for route in routes {
        let request_body = lower_request_body(&route.body, &mut graphs)?;
        let call_agent = match &route.behavior {
            super::super::RichRouteBehaviour::CallAgent(inner) => {
                Some(lower_call_agent(&route.path, inner, &mut graphs)?)
            }
            _ => None,
        };
        per_route.push(RouteSchema {
            request_body,
            call_agent,
        });
    }

    // The merged graph carries the union of every route's named defs; its
    // `root` is a sentinel (the emitter resolves each route's own roots —
    // stored in the per-route models — against this graph's `defs`).
    let graph = merge_agent_graphs(graphs).map_err(|err| match err {
        golem_common::schema::MergeError::ConflictingDefinitions { id, .. } => {
            SchemaAdapterError::DuplicateTypeIdConflict(id)
        }
    })?;
    Ok(DocumentSchema { graph, per_route })
}

/// Lower a request body schema, contributing any named composites it
/// references to `graphs` for the document-wide merge.
fn lower_request_body(
    body: &RequestBodySchema,
    graphs: &mut Vec<SchemaGraph>,
) -> Result<RequestBodyModel, SchemaAdapterError> {
    Ok(match body {
        RequestBodySchema::Unused => RequestBodyModel::Unused,
        RequestBodySchema::JsonBody { expected } => {
            // The JSON body root may be (or reference) named composites; keep
            // its graph so those refs resolve against the merged document graph.
            graphs.push(expected.graph.clone());
            RequestBodyModel::Json(Box::new(expected.graph.root.clone()))
        }
        RequestBodySchema::BinaryBody { expected } => {
            // The body root is either a canonical unstructured-binary
            // `variant { inline, url }` wrapper or a bare `Binary` rich scalar;
            // both carry the MIME restrictions on their (inline) binary type. An
            // empty allow-list is treated as unrestricted (matching the runtime
            // request decoder).
            let restrictions = binary_body_restrictions(&expected.graph, &expected.graph.root)?;
            match &restrictions.mime_types {
                Some(mime_types) if !mime_types.is_empty() => RequestBodyModel::RestrictedBinary {
                    mime_types: mime_types.clone(),
                },
                _ => RequestBodyModel::UnrestrictedBinary,
            }
        }
        RequestBodySchema::TextBody { expected } => {
            // The body root is either a canonical unstructured-text
            // `variant { inline, url }` wrapper or a bare `Text` rich scalar;
            // both carry the language restrictions on their (inline) text type.
            // An empty allow-list is treated as unrestricted.
            let restrictions = text_body_restrictions(&expected.graph, &expected.graph.root)?;
            match &restrictions.languages {
                Some(language_codes) if !language_codes.is_empty() => {
                    RequestBodyModel::RestrictedText {
                        language_codes: language_codes.clone(),
                    }
                }
                _ => RequestBodyModel::UnrestrictedText,
            }
        }
    })
}

fn lower_call_agent(
    path: &[PathSegment],
    inner: &CallAgentBehaviour,
    graphs: &mut Vec<SchemaGraph>,
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

    let response = lower_response(&inner.expected_agent_response, graphs)?;

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

/// Lower a method's compiled output schema into the emitter response model,
/// contributing its named composites to `graphs`.
fn lower_response(
    expected: &CompiledOutputSchema,
    graphs: &mut Vec<SchemaGraph>,
) -> Result<ResponseModel, SchemaAdapterError> {
    match &expected.output_schema {
        OutputSchema::Unit => Ok(ResponseModel::Unit),
        OutputSchema::Single(ty) => {
            // Multimodal responses are rendered opaque by the emitter, matching
            // legacy behaviour.
            if is_multimodal_schema_type(&expected.graph, ty)? {
                Ok(ResponseModel::Unknown)
            } else {
                graphs.push(expected.graph.clone());
                Ok(ResponseModel::Single(Box::new((**ty).clone())))
            }
        }
    }
}
