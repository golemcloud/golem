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

use golem_common::model::agent::wit_naming::ToWitNaming;
use golem_common::model::agent::{
    ComponentModelElementSchema, DataSchema, ElementSchema, HeaderVariable, HttpEndpointDetails,
    HttpMountDetails, NamedElementSchema, NamedElementSchemas, PathSegment, QueryVariable,
};
use golem_service_base::custom_api::{
    ConstructorParameter, MethodParameter, PathSegmentType, QueryOrHeaderType, RequestBodySchema,
};
use golem_service_base::model::SafeIndex;
use golem_wasm::analysis::{AnalysedType, NameTypePair, TypeRecord};
use std::collections::HashMap;

pub fn build_http_agent_constructor_parameters<E>(
    mount: &HttpMountDetails,
    schema: &DataSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<Vec<ConstructorParameter>, E> {
    let elements = match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => elements,
        _ => {
            return Err(make_error(
                "Only Tuple dataschemas are supported for http-bindable constructors".into(),
            ));
        }
    };

    let path_bindings = collect_path_bindings_from_segments(
        &mount.path_prefix,
        0..mount.path_prefix.len(),
        make_error,
    )?;

    let mut result = Vec::with_capacity(elements.len());

    for element in elements {
        let name = &element.name;

        let (path_index, segment) = path_bindings.get(name).ok_or_else(|| {
            make_error(format!(
                "Constructor parameter '{}' must bind to a path variable",
                name
            ))
        })?;

        let ty = element_schema_to_path_segment_type(&element.schema, &make_error)?;

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
    schema: &DataSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<(RequestBodySchema, Vec<MethodParameter>), E> {
    let elements = match schema {
        DataSchema::Tuple(NamedElementSchemas { elements }) => elements,
        _ => {
            return Err(make_error(
                "Only Tuple dataschemas are supported for http-bindable agents".into(),
            ));
        }
    };

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

    let mut consumed = vec![false; elements.len()];
    let mut method_parameters = Vec::with_capacity(elements.len());

    // First pass: path / query / header bindings
    for (idx, element) in elements.iter().enumerate() {
        let name = &element.name;

        // 1. Path
        if let Some((path_index, segment_kind)) = path_bindings.get(name) {
            let ty = element_schema_to_path_segment_type(&element.schema, make_error)?;
            validate_path_segment_type(segment_kind, &ty, make_error)?;

            method_parameters.push(MethodParameter::Path {
                path_segment_index: *path_index,
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }

        // 2. Query
        if let Some(query_var) = query_bindings.get(name) {
            let ty = element_schema_to_query_or_header_type(&element.schema, make_error)?;

            method_parameters.push(MethodParameter::Query {
                query_parameter_name: query_var.query_param_name.clone(),
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }

        // 3. Header
        if let Some(header_var) = header_bindings.get(name) {
            let ty = element_schema_to_query_or_header_type(&element.schema, make_error)?;

            method_parameters.push(MethodParameter::Header {
                header_name: header_var.header_name.clone(),
                parameter_type: ty,
            });
            consumed[idx] = true;
            continue;
        }
    }

    // Second pass: body handling
    let body_schema =
        handle_body_parameters(elements, &consumed, &mut method_parameters, make_error)?;

    Ok((body_schema, method_parameters))
}

fn handle_body_parameters<E>(
    elements: &[NamedElementSchema],
    consumed: &[bool],
    out: &mut Vec<MethodParameter>,
    make_error: &impl Fn(String) -> E,
) -> Result<RequestBodySchema, E> {
    let leftovers: Vec<(usize, &NamedElementSchema)> = elements
        .iter()
        .enumerate()
        .filter(|(i, _)| !consumed[*i])
        .collect();

    // No body
    if leftovers.is_empty() {
        return Ok(RequestBodySchema::Unused);
    }

    // JSON object body
    if leftovers
        .iter()
        .all(|(_, e)| matches!(e.schema, ElementSchema::ComponentModel(_)))
    {
        let mut body_fields = Vec::new();
        for (_, named_schema) in leftovers {
            let ElementSchema::ComponentModel(ComponentModelElementSchema { element_type }) =
                &named_schema.schema
            else {
                unreachable!();
            };

            let field_index = body_fields.len();
            body_fields.push(NameTypePair {
                name: named_schema.name.to_wit_naming(),
                typ: element_type.clone(),
            });

            out.push(MethodParameter::JsonObjectBodyField {
                field_index: SafeIndex::try_from(field_index).map_err(make_error)?,
            });
        }

        // synthethic record made out of all leftover parameters
        let body_type = AnalysedType::Record(TypeRecord {
            owner: None,
            name: None,
            fields: body_fields,
        });

        return Ok(RequestBodySchema::JsonBody {
            expected_type: body_type,
        });
    }

    // binary body
    if leftovers.len() == 1
        && let ElementSchema::UnstructuredBinary(descriptor) = &leftovers[0].1.schema
    {
        out.push(MethodParameter::UnstructuredBinaryBody);
        if let Some(binary_types) = &descriptor.restrictions {
            let allowed_mime_types = binary_types.iter().map(|bd| bd.mime_type.clone()).collect();
            return Ok(RequestBodySchema::RestrictedBinary { allowed_mime_types });
        } else {
            return Ok(RequestBodySchema::UnrestrictedBinary);
        }
    }

    Err(make_error(
        "Invalid body parameters: expected either no body, \
         all ComponentModel parameters, or a single UnstructuredBinary parameter"
            .into(),
    ))
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

fn element_schema_to_path_segment_type<E>(
    schema: &ElementSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<PathSegmentType, E> {
    let cm = match schema {
        ElementSchema::ComponentModel(cm) => cm,
        _ => {
            return Err(make_error(
                "Only component model types can be bound to path segments".into(),
            ));
        }
    };

    let ty = PathSegmentType::try_from(cm.element_type.clone()).map_err(make_error)?;
    Ok(ty)
}

fn element_schema_to_query_or_header_type<E>(
    schema: &ElementSchema,
    make_error: &impl Fn(String) -> E,
) -> Result<QueryOrHeaderType, E> {
    let cm = match schema {
        ElementSchema::ComponentModel(cm) => cm,
        _ => {
            return Err(make_error(
                "Only component model types can be bound to query or header parameters".into(),
            ));
        }
    };

    let ty = QueryOrHeaderType::try_from(cm.element_type.clone()).map_err(make_error)?;
    Ok(ty)
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
        BinaryDescriptor, BinaryType, ComponentModelElementSchema, CorsOptions, DataSchema,
        ElementSchema, HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment,
        NamedElementSchema, NamedElementSchemas, PathSegment, PathVariable,
    };
    use golem_wasm::analysis::analysed_type;
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

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "agent_id".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: analysed_type::str(),
                }),
            }],
        });

        let params = build_http_agent_constructor_parameters(&mount, &schema, &|msg| msg).unwrap();

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

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "missing".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: analysed_type::str(),
                }),
            }],
        });

        let err = build_http_agent_constructor_parameters(&mount, &schema, &|msg| msg).unwrap_err();

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

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "rest".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: analysed_type::u32(),
                }),
            }],
        });

        let err = build_http_agent_constructor_parameters(&mount, &schema, &|msg| msg).unwrap_err();

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

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "task_id".into(),
                schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: analysed_type::str(),
                }),
            }],
        });

        let (_body, params) =
            build_http_agent_method_parameters(&mount, &endpoint, &schema, &|msg| msg).unwrap();

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
        let mount = HttpMountDetails {
            path_prefix: vec![],
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
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![],
        };

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "a".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: analysed_type::str(),
                    }),
                },
                NamedElementSchema {
                    name: "b".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: analysed_type::u32(),
                    }),
                },
            ],
        });

        let (body, params) =
            build_http_agent_method_parameters(&mount, &endpoint, &schema, &|msg| msg).unwrap();

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
        let mount = HttpMountDetails {
            path_prefix: vec![],
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
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![],
        };

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "return-type".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor { restrictions: None }),
            }],
        });

        let (body, params) =
            build_http_agent_method_parameters(&mount, &endpoint, &schema, &|msg| msg).unwrap();

        assert!(matches!(body, RequestBodySchema::UnrestrictedBinary));
        assert_eq!(params, vec![MethodParameter::UnstructuredBinaryBody]);
    }

    #[test]
    fn method_accepts_unstructured_binary_body_restricted() {
        let mount = HttpMountDetails {
            path_prefix: vec![],
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
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![],
        };

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![NamedElementSchema {
                name: "return-type".into(),
                schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                    restrictions: Some(vec![BinaryType {
                        mime_type: "application/octet-stream".into(),
                    }]),
                }),
            }],
        });

        let (body, params) =
            build_http_agent_method_parameters(&mount, &endpoint, &schema, &|msg| msg).unwrap();

        {
            let_assert!(RequestBodySchema::RestrictedBinary { allowed_mime_types } = body);
            assert!(allowed_mime_types == vec!["application/octet-stream"]);
        }

        assert_eq!(params, vec![MethodParameter::UnstructuredBinaryBody]);
    }

    #[test]
    fn method_rejects_mixed_body_parameters() {
        let mount = HttpMountDetails {
            path_prefix: vec![],
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
            path_suffix: vec![],
            query_vars: vec![],
            header_vars: vec![],
        };

        let schema = DataSchema::Tuple(NamedElementSchemas {
            elements: vec![
                NamedElementSchema {
                    name: "a".into(),
                    schema: ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: analysed_type::str(),
                    }),
                },
                NamedElementSchema {
                    name: "b".into(),
                    schema: ElementSchema::UnstructuredBinary(BinaryDescriptor {
                        restrictions: None,
                    }),
                },
            ],
        });

        let err =
            build_http_agent_method_parameters(&mount, &endpoint, &schema, &|msg| msg).unwrap_err();

        assert!(err.contains("Invalid body parameters"));
    }
}
