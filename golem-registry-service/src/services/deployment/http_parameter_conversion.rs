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

//! Schema-native HTTP parameter binding for compiled agent routes.
//!
//! Consumes the agent's per-agent [`SchemaGraph`] plus the constructor /
//! method [`InputSchema`] (an ordered list of [`NamedField`]s) and produces
//! the persisted [`ConstructorParameter`] / [`MethodParameter`] bindings plus
//! the [`RequestBodySchema`] for a route.
//!
//! Auto-injected fields ([`FieldSource::AutoInjected`]) are never bound to
//! HTTP and never appear in the request body — the host injects them at
//! invocation time. They are still part of the method/constructor input record
//! (carried in the compiled [`CompiledInputSchema`]); the worker-service
//! runtime reconstructs the full positional record by iterating the
//! [`InputSchema`] and inserting auto-injected fields in declaration order.

use golem_common::model::agent::{
    HeaderVariable, HttpEndpointDetails, HttpMountDetails, PathSegment, QueryVariable,
};
use golem_common::schema::adapters::resolve_ref;
use golem_common::schema::{
    FieldSource, InputSchema, NamedField, NamedFieldType, SchemaGraph, SchemaType,
};
use golem_service_base::custom_api::{
    CompiledSchema, ConstructorParameter, MethodParameter, PathSegmentType, QueryOrHeaderType,
    RequestBodySchema,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::TypeEnum;
use std::collections::HashMap;

pub fn build_http_agent_constructor_parameters<E>(
    mount: &HttpMountDetails,
    graph: &SchemaGraph,
    schema: &InputSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<Vec<ConstructorParameter>, E> {
    let fields = schema.fields();

    let path_bindings = collect_path_bindings_from_segments(
        &mount.path_prefix,
        0..mount.path_prefix.len(),
        make_error,
    )?;

    let mut result = Vec::with_capacity(fields.len());

    for field in fields {
        // Auto-injected fields are supplied by the host; they must not bind to
        // HTTP path segments and they are not part of the caller-facing route.
        if matches!(field.source, FieldSource::AutoInjected(_)) {
            if path_bindings.contains_key(&field.name) {
                return Err(make_error(format!(
                    "Auto-injected field '{}' cannot be bound to a path variable",
                    field.name
                )));
            }
            continue;
        }

        let name = &field.name;

        let (path_index, segment) = path_bindings.get(name).ok_or_else(|| {
            make_error(format!(
                "Constructor parameter '{}' must bind to a path variable",
                name
            ))
        })?;

        let ty = schema_type_to_path_segment_type(graph, &field.schema, make_error)?;

        validate_path_segment_type(segment, &ty, make_error)?;

        result.push(ConstructorParameter::Path {
            path_segment_index: *path_index,
            parameter_type: ty,
        });
    }

    Ok(result)
}

pub fn build_http_agent_method_parameters<E>(
    mount: &HttpMountDetails,
    endpoint: &HttpEndpointDetails,
    graph: &SchemaGraph,
    schema: &InputSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<(RequestBodySchema, Vec<MethodParameter>), E> {
    let fields = schema.fields();

    let mut all_segments = Vec::new();
    all_segments.extend_from_slice(&mount.path_prefix);
    let bindable_start = all_segments.len();
    all_segments.extend_from_slice(&endpoint.path_suffix);

    // methods are only allowed to bind to the section of the path they define
    let path_bindings = collect_path_bindings_from_segments(
        &all_segments,
        bindable_start..all_segments.len(),
        make_error,
    )?;
    let query_bindings = collect_query_bindings(endpoint);
    let header_bindings = collect_header_bindings(endpoint);

    // Per-field binding slot. `None` for auto-injected fields (excluded from the
    // invocation record) and for fields that still need body classification.
    let mut per_field: Vec<Option<MethodParameter>> = vec![None; fields.len()];
    let mut consumed = vec![false; fields.len()];

    // First pass: classify path / query / header bindings (user-supplied fields
    // only). Auto-injected fields are supplied by the host out-of-band, so they
    // must not bind to HTTP and are excluded from the invocation record.
    for (idx, field) in fields.iter().enumerate() {
        let name = &field.name;

        if matches!(field.source, FieldSource::AutoInjected(_)) {
            if path_bindings.contains_key(name)
                || query_bindings.contains_key(name)
                || header_bindings.contains_key(name)
            {
                return Err(make_error(format!(
                    "Auto-injected field '{}' cannot be bound to a path, query or header variable",
                    name
                )));
            }
            // mark consumed so it is excluded from the body record too
            consumed[idx] = true;
            continue;
        }

        // 1. Path
        if let Some((path_index, segment_kind)) = path_bindings.get(name) {
            let ty = schema_type_to_path_segment_type(graph, &field.schema, make_error)?;
            validate_path_segment_type(segment_kind, &ty, make_error)?;

            per_field[idx] = Some(MethodParameter::Path {
                path_segment_index: *path_index,
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }

        // 2. Query
        if let Some(query_var) = query_bindings.get(name) {
            let ty = schema_type_to_query_or_header_type(graph, &field.schema, make_error)?;

            per_field[idx] = Some(MethodParameter::Query {
                query_parameter_name: query_var.query_param_name.clone(),
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }

        // 3. Header
        if let Some(header_var) = header_bindings.get(name) {
            let ty = schema_type_to_query_or_header_type(graph, &field.schema, make_error)?;

            per_field[idx] = Some(MethodParameter::Header {
                header_name: header_var.header_name.clone(),
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }
    }

    // Second pass: classify the remaining (body) fields into `per_field`.
    let body_schema = handle_body_parameters(graph, fields, &consumed, &mut per_field, make_error)?;

    // Final pass: emit method parameters in user-supplied input declaration
    // order. The worker-service runtime relies on this ordering to build the
    // positional `SchemaValue::Record` the executor expects (which validates
    // the record positionally against the user-supplied fields in declaration
    // order).
    let method_parameters: Vec<MethodParameter> = per_field.into_iter().flatten().collect();

    Ok((body_schema, method_parameters))
}

fn handle_body_parameters<E>(
    graph: &SchemaGraph,
    fields: &[NamedField],
    consumed: &[bool],
    per_field: &mut [Option<MethodParameter>],
    make_error: &impl Fn(String) -> E,
) -> Result<RequestBodySchema, E> {
    let leftovers: Vec<(usize, &NamedField)> = fields
        .iter()
        .enumerate()
        .filter(|(i, field)| matches!(field.source, FieldSource::UserSupplied) && !consumed[*i])
        .collect();

    // No body
    if leftovers.is_empty() {
        return Ok(RequestBodySchema::Unused);
    }

    // binary body
    if leftovers.len() == 1
        && let SchemaType::Binary { .. } =
            resolve_ref(graph, &leftovers[0].1.schema).map_err(|e| make_error(e.to_string()))?
    {
        per_field[leftovers[0].0] = Some(MethodParameter::UnstructuredBinaryBody);
        return Ok(RequestBodySchema::BinaryBody {
            expected: CompiledSchema {
                graph: SchemaGraph {
                    defs: graph.defs.clone(),
                    root: leftovers[0].1.schema.clone(),
                },
            },
        });
    }

    // text body
    if leftovers.len() == 1
        && let SchemaType::Text { .. } =
            resolve_ref(graph, &leftovers[0].1.schema).map_err(|e| make_error(e.to_string()))?
    {
        per_field[leftovers[0].0] = Some(MethodParameter::UnstructuredTextBody);
        return Ok(RequestBodySchema::TextBody {
            expected: CompiledSchema {
                graph: SchemaGraph {
                    defs: graph.defs.clone(),
                    root: leftovers[0].1.schema.clone(),
                },
            },
        });
    }

    // JSON object body: every leftover field must be a structural component-model
    // type (i.e. not a raw unstructured binary/text body). `field_index` indexes
    // into the JSON body record (built here in leftover/declaration order), not
    // into the input field list.
    let mut body_fields = Vec::new();
    for (input_index, field) in &leftovers {
        let resolved = resolve_ref(graph, &field.schema).map_err(|e| make_error(e.to_string()))?;
        if matches!(
            resolved,
            SchemaType::Binary { .. } | SchemaType::Text { .. }
        ) {
            return Err(make_error(
                "Invalid body parameters: expected either no body, \
                 all structural parameters (JSON object body), a single binary parameter, \
                 or a single text parameter"
                    .into(),
            ));
        }

        let field_index = body_fields.len();
        body_fields.push(NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        });

        per_field[*input_index] = Some(MethodParameter::JsonObjectBodyField {
            field_index: SafeIndex::try_from(field_index).map_err(make_error)?,
        });
    }

    Ok(RequestBodySchema::JsonBody {
        expected: CompiledSchema {
            graph: SchemaGraph {
                defs: graph.defs.clone(),
                root: SchemaType::record(body_fields),
            },
        },
    })
}

fn collect_path_bindings_from_segments<E>(
    all_segments: &[PathSegment],
    bindable_range: std::ops::Range<usize>,
    make_error: &impl Fn(String) -> E,
) -> Result<HashMap<String, (SafeIndex, PathSegment)>, E> {
    let mut map = HashMap::new();
    let mut var_index: u32 = 0;
    let mut found_remaining_path_variable = false;

    for (i, segment) in all_segments.iter().enumerate() {
        // remaining_path_variable must be last segment
        if found_remaining_path_variable {
            return Err(make_error(
                "RemainingPathVariable must only occur as the last segment".into(),
            ));
        }

        let is_variable = matches!(
            segment,
            PathSegment::PathVariable(_) | PathSegment::RemainingPathVariable(_)
        );

        if is_variable {
            let index = SafeIndex::from(var_index);

            // Only variables inside bindable_range are exposed
            if bindable_range.contains(&i) {
                let var = match segment {
                    PathSegment::PathVariable(v) | PathSegment::RemainingPathVariable(v) => v,
                    _ => unreachable!(),
                };

                if map
                    .insert(var.variable_name.clone(), (index, segment.clone()))
                    .is_some()
                {
                    return Err(make_error(format!(
                        "Duplicate path variable '{}'",
                        var.variable_name
                    )));
                }
            }

            var_index = var_index
                .checked_add(1)
                .ok_or_else(|| make_error("Path variable index overflow".into()))?;

            if matches!(segment, PathSegment::RemainingPathVariable(_)) {
                found_remaining_path_variable = true;
            }
        }
    }

    Ok(map)
}

fn collect_query_bindings(endpoint: &HttpEndpointDetails) -> HashMap<String, QueryVariable> {
    endpoint
        .query_vars
        .iter()
        .map(|v| (v.variable_name.clone(), v.clone()))
        .collect()
}

fn collect_header_bindings(endpoint: &HttpEndpointDetails) -> HashMap<String, HeaderVariable> {
    endpoint
        .header_vars
        .iter()
        .map(|v| (v.variable_name.clone(), v.clone()))
        .collect()
}

/// Classify a (possibly `Ref`) [`SchemaType`] as a scalar HTTP path-segment
/// type. Refs are resolved against `graph` first.
fn schema_type_to_path_segment_type<E>(
    graph: &SchemaGraph,
    schema: &SchemaType,
    make_error: &impl Fn(String) -> E,
) -> Result<PathSegmentType, E> {
    let resolved = resolve_ref(graph, schema).map_err(|e| make_error(e.to_string()))?;
    match resolved {
        SchemaType::String { .. } => Ok(PathSegmentType::Str),
        SchemaType::Char { .. } => Ok(PathSegmentType::Chr),
        SchemaType::F64 { .. } => Ok(PathSegmentType::F64),
        SchemaType::F32 { .. } => Ok(PathSegmentType::F32),
        SchemaType::U64 { .. } => Ok(PathSegmentType::U64),
        SchemaType::S64 { .. } => Ok(PathSegmentType::S64),
        SchemaType::U32 { .. } => Ok(PathSegmentType::U32),
        SchemaType::S32 { .. } => Ok(PathSegmentType::S32),
        SchemaType::U16 { .. } => Ok(PathSegmentType::U16),
        SchemaType::S16 { .. } => Ok(PathSegmentType::S16),
        SchemaType::U8 { .. } => Ok(PathSegmentType::U8),
        SchemaType::S8 { .. } => Ok(PathSegmentType::S8),
        SchemaType::Bool { .. } => Ok(PathSegmentType::Bool),
        SchemaType::Enum { cases, .. } => Ok(PathSegmentType::Enum(TypeEnum {
            owner: None,
            name: None,
            cases: cases.clone(),
        })),
        _ => Err(make_error(
            "Only primitive or enum types can be bound to path segments".into(),
        )),
    }
}

/// Classify a (possibly `Ref`) [`SchemaType`] as a query/header parameter
/// type: a scalar, or an `option`/`list` of a scalar. Refs are resolved
/// against `graph` first.
fn schema_type_to_query_or_header_type<E>(
    graph: &SchemaGraph,
    schema: &SchemaType,
    make_error: &impl Fn(String) -> E,
) -> Result<QueryOrHeaderType, E> {
    let resolved = resolve_ref(graph, schema).map_err(|e| make_error(e.to_string()))?;
    match resolved {
        SchemaType::Option { inner, .. } => Ok(QueryOrHeaderType::Option {
            name: None,
            owner: None,
            inner: Box::new(schema_type_to_path_segment_type(graph, inner, make_error)?),
        }),
        SchemaType::List { element, .. } => Ok(QueryOrHeaderType::List {
            name: None,
            owner: None,
            inner: Box::new(schema_type_to_path_segment_type(
                graph, element, make_error,
            )?),
        }),
        other => Ok(QueryOrHeaderType::Primitive(
            schema_type_to_path_segment_type(graph, other, make_error)?,
        )),
    }
}

fn validate_path_segment_type<E>(
    segment: &PathSegment,
    ty: &PathSegmentType,
    make_error: &impl Fn(String) -> E,
) -> Result<(), E> {
    if let PathSegment::RemainingPathVariable(_) = segment
        && !matches!(ty, PathSegmentType::Str)
    {
        return Err(make_error(
            "Remaining path variables must be of type string".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use golem_common::model::agent::{
        CorsOptions, HeaderVariable, HttpEndpointDetails, HttpMethod, HttpMountDetails,
        LiteralSegment, PathSegment, PathVariable, QueryVariable,
    };
    use golem_common::schema::{
        BinaryRestrictions, InputSchema, NamedField, SchemaGraph, SchemaType, TextRestrictions,
    };
    use test_r::test;

    use crate::services::deployment::http_parameter_conversion::{
        build_http_agent_constructor_parameters, build_http_agent_method_parameters,
    };
    use assert2::{assert, let_assert};
    use golem_common::model::Empty;
    use golem_service_base::custom_api::{
        ConstructorParameter, MethodParameter, PathSegmentType, RequestBodySchema,
    };
    use golem_service_base::model::SafeIndex;

    fn empty_graph() -> SchemaGraph {
        SchemaGraph::empty()
    }

    fn input(fields: Vec<NamedField>) -> InputSchema {
        InputSchema::Parameters(fields)
    }

    fn str_field(name: &str) -> NamedField {
        NamedField::user_supplied(name, SchemaType::string())
    }

    #[test]
    fn constructor_binds_all_parameters_from_mount_path() {
        let mount = HttpMountDetails {
            path_prefix: vec![
                PathSegment::Literal(LiteralSegment {
                    value: "agents".into(),
                }),
                PathSegment::PathVariable(PathVariable {
                    variable_name: "agent_id".into(),
                }),
            ],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let schema = input(vec![str_field("agent_id")]);

        let params =
            build_http_agent_constructor_parameters(&mount, &empty_graph(), &schema, &|msg| msg)
                .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(
            params[0],
            ConstructorParameter::Path {
                path_segment_index,
                parameter_type: PathSegmentType::Str
            } if path_segment_index == SafeIndex::from(0)
        ));
    }

    #[test]
    fn constructor_fails_if_parameter_not_in_path() {
        let mount = HttpMountDetails {
            path_prefix: vec![PathSegment::Literal(LiteralSegment {
                value: "agents".into(),
            })],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let schema = input(vec![str_field("missing")]);

        let err =
            build_http_agent_constructor_parameters(&mount, &empty_graph(), &schema, &|msg| msg)
                .unwrap_err();

        assert!(err.contains("must bind to a path variable"));
    }

    #[test]
    fn constructor_rejects_non_string_remaining_path_variable() {
        let mount = HttpMountDetails {
            path_prefix: vec![PathSegment::RemainingPathVariable(PathVariable {
                variable_name: "rest".into(),
            })],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let schema = input(vec![NamedField::user_supplied("rest", SchemaType::u32())]);

        let err =
            build_http_agent_constructor_parameters(&mount, &empty_graph(), &schema, &|msg| msg)
                .unwrap_err();

        assert!(err.contains("Remaining path variables must be of type string"));
    }

    #[test]
    fn method_parameters_only_bind_to_endpoint_suffix() {
        let mount = HttpMountDetails {
            path_prefix: vec![PathSegment::PathVariable(PathVariable {
                variable_name: "agent_id".into(),
            })],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let endpoint = HttpEndpointDetails {
            http_method: HttpMethod::Get(Empty {}),
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            path_suffix: vec![PathSegment::PathVariable(PathVariable {
                variable_name: "task_id".into(),
            })],
            query_vars: vec![],
            header_vars: vec![],
        };

        let schema = input(vec![str_field("task_id")]);

        let (_body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(
            params[0],
            MethodParameter::Path {
                path_segment_index,
                ..
            } if path_segment_index == SafeIndex::from(1)
        ));
    }

    #[test]
    fn method_infers_json_body_from_component_model_parameters() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![
            str_field("a"),
            NamedField::user_supplied("b", SchemaType::u32()),
        ]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert!(let RequestBodySchema::JsonBody { .. } = body);
        assert_eq!(params.len(), 2);

        assert!(
            params
                .iter()
                .all(|p| matches!(p, MethodParameter::JsonObjectBodyField { .. }))
        );
    }

    #[test]
    fn method_accepts_unstructured_binary_body_unrestricted() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![NamedField::user_supplied(
            "return-type",
            SchemaType::binary(BinaryRestrictions::default()),
        )]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert!(matches!(body, RequestBodySchema::BinaryBody { .. }));
        assert_eq!(params, vec![MethodParameter::UnstructuredBinaryBody]);
    }

    #[test]
    fn method_accepts_unstructured_binary_body_restricted() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![NamedField::user_supplied(
            "return-type",
            SchemaType::binary(BinaryRestrictions {
                mime_types: Some(vec!["application/octet-stream".into()]),
                min_bytes: None,
                max_bytes: None,
            }),
        )]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        let_assert!(RequestBodySchema::BinaryBody { expected } = body);
        let_assert!(SchemaType::Binary { restrictions, .. } = expected.graph.root);
        assert!(restrictions.mime_types == Some(vec!["application/octet-stream".to_string()]));

        assert_eq!(params, vec![MethodParameter::UnstructuredBinaryBody]);
    }

    #[test]
    fn method_rejects_mixed_body_parameters() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![
            str_field("a"),
            NamedField::user_supplied("b", SchemaType::binary(BinaryRestrictions::default())),
        ]);

        let err = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap_err();

        assert!(err.contains("Invalid body parameters"));
    }

    #[test]
    fn constructor_binds_snake_case_parameter_from_path() {
        let mount = HttpMountDetails {
            path_prefix: vec![
                PathSegment::Literal(LiteralSegment {
                    value: "agents".into(),
                }),
                PathSegment::PathVariable(PathVariable {
                    variable_name: "user_name".into(),
                }),
            ],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let schema = input(vec![str_field("user_name")]);

        let params =
            build_http_agent_constructor_parameters(&mount, &empty_graph(), &schema, &|msg| msg)
                .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(
            params[0],
            ConstructorParameter::Path {
                path_segment_index,
                parameter_type: PathSegmentType::Str
            } if path_segment_index == SafeIndex::from(0)
        ));
    }

    #[test]
    fn constructor_binds_camel_case_parameter_from_path() {
        let mount = HttpMountDetails {
            path_prefix: vec![
                PathSegment::Literal(LiteralSegment {
                    value: "agents".into(),
                }),
                PathSegment::PathVariable(PathVariable {
                    variable_name: "userName".into(),
                }),
            ],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        };

        let schema = input(vec![str_field("userName")]);

        let params =
            build_http_agent_constructor_parameters(&mount, &empty_graph(), &schema, &|msg| msg)
                .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(
            params[0],
            ConstructorParameter::Path {
                path_segment_index,
                parameter_type: PathSegmentType::Str
            } if path_segment_index == SafeIndex::from(0)
        ));
    }

    #[test]
    fn method_binds_snake_case_query_variable() {
        let mount = empty_mount();

        let endpoint = HttpEndpointDetails {
            http_method: HttpMethod::Get(Empty {}),
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            path_suffix: vec![],
            query_vars: vec![QueryVariable {
                query_param_name: "page_size".into(),
                variable_name: "page_size".into(),
            }],
            header_vars: vec![],
        };

        let schema = input(vec![str_field("page_size")]);

        let (_body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(params[0], MethodParameter::Query { .. }));
    }

    #[test]
    fn method_binds_camel_case_query_variable() {
        let mount = empty_mount();

        let endpoint = HttpEndpointDetails {
            http_method: HttpMethod::Get(Empty {}),
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            path_suffix: vec![],
            query_vars: vec![QueryVariable {
                query_param_name: "pageSize".into(),
                variable_name: "pageSize".into(),
            }],
            header_vars: vec![],
        };

        let schema = input(vec![str_field("pageSize")]);

        let (_body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(params[0], MethodParameter::Query { .. }));
    }

    #[test]
    fn method_binds_snake_case_header_variable() {
        let mount = empty_mount();

        let endpoint = HttpEndpointDetails {
            http_method: HttpMethod::Get(Empty {}),
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![HeaderVariable {
                header_name: "x-api-key".into(),
                variable_name: "x_api_key".into(),
            }],
        };

        let schema = input(vec![str_field("x_api_key")]);

        let (_body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert_eq!(params.len(), 1);
        assert!(matches!(params[0], MethodParameter::Header { .. }));
    }

    #[test]
    fn method_json_body_preserves_snake_case_field_names() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![str_field("first_name"), str_field("last_name")]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        let_assert!(RequestBodySchema::JsonBody { expected } = body);
        let_assert!(SchemaType::Record { fields, .. } = expected.graph.root);
        assert!(fields[0].name == "first_name");
        assert!(fields[1].name == "last_name");

        assert_eq!(params.len(), 2);
    }

    #[test]
    fn method_json_body_preserves_camel_case_field_names() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![str_field("firstName"), str_field("lastName")]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        let_assert!(RequestBodySchema::JsonBody { expected } = body);
        let_assert!(SchemaType::Record { fields, .. } = expected.graph.root);
        assert!(fields[0].name == "firstName");
        assert!(fields[1].name == "lastName");

        assert_eq!(params.len(), 2);
    }

    fn empty_mount() -> HttpMountDetails {
        HttpMountDetails {
            path_prefix: vec![],
            auth_details: None,
            phantom_agent: false,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            webhook_suffix: vec![],
        }
    }

    fn empty_get_endpoint() -> HttpEndpointDetails {
        HttpEndpointDetails {
            http_method: HttpMethod::Get(Empty {}),
            auth_details: None,
            cors_options: CorsOptions {
                allowed_patterns: Vec::new(),
            },
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![],
        }
    }

    #[test]
    fn method_accepts_unstructured_text_body_unrestricted() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![NamedField::user_supplied(
            "body",
            SchemaType::text(TextRestrictions::default()),
        )]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        assert!(matches!(body, RequestBodySchema::TextBody { .. }));
        assert_eq!(params, vec![MethodParameter::UnstructuredTextBody]);
    }

    #[test]
    fn method_accepts_unstructured_text_body_restricted() {
        let mount = empty_mount();
        let endpoint = empty_get_endpoint();

        let schema = input(vec![NamedField::user_supplied(
            "body",
            SchemaType::text(TextRestrictions {
                languages: Some(vec!["en".into(), "de".into()]),
                min_length: None,
                max_length: None,
                regex: None,
            }),
        )]);

        let (body, params) = build_http_agent_method_parameters(
            &mount,
            &endpoint,
            &empty_graph(),
            &schema,
            &|msg| msg,
        )
        .unwrap();

        let_assert!(RequestBodySchema::TextBody { expected } = body);
        let_assert!(SchemaType::Text { restrictions, .. } = expected.graph.root);
        assert!(restrictions.languages == Some(vec!["en".to_string(), "de".to_string()]));

        assert_eq!(params, vec![MethodParameter::UnstructuredTextBody]);
    }
}
