// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use crate::custom_api::RichRouteBehaviour;
use crate::custom_api::openapi::core::HttpApiRoute;
use crate::custom_api::openapi::schema_mapping::create_schema_from_analysed_type;
use golem_common::base_model::agent::{ElementSchema, HttpMethod};
use golem_common::model::agent::DataSchema;
use golem_wasm::analysis::AnalysedType;
use indexmap::IndexMap;
use openapiv3::{
    AdditionalProperties, ObjectType, Schema, SchemaData, SchemaKind, StringFormat, StringType,
    Type, VariantOrUnknownOrEmpty,
};

pub enum ResponseBodyOpenApiSchema {
    Unknown,
    Known {
        schema: Box<Schema>,
        content_type: String,
    },
    NoBody,
}

pub struct AgentResponseOpenApiSchema {
    pub body_and_status_codes: IndexMap<u16, ResponseBodyOpenApiSchema>,
    pub headers: IndexMap<String, Schema>,
}

pub fn get_agent_response_schema<T: HttpApiRoute + ?Sized>(
    route: &T,
) -> AgentResponseOpenApiSchema {
    let mut responses = IndexMap::new();
    let mut headers = IndexMap::new();

    match route.binding() {
        RichRouteBehaviour::CallAgent(call_agent_behaviour) => {
            match &call_agent_behaviour.expected_agent_response {
                DataSchema::Tuple(named_elements) => match named_elements.elements.len() {
                    0 => {
                        responses.insert(204, ResponseBodyOpenApiSchema::NoBody);
                    }
                    1 => {
                        let element_schema = &named_elements.elements[0].schema;

                        match element_schema {
                            // based on response_mapping logic in the actual request_handler
                            ElementSchema::ComponentModel(typ) => {
                                if let AnalysedType::Option(opt) = &typ.element_type {
                                    let inner_schema = create_schema_from_analysed_type(&opt.inner);
                                    responses.insert(
                                        200,
                                        ResponseBodyOpenApiSchema::Known {
                                            schema: Box::new(inner_schema),
                                            content_type: "application/json".to_string(),
                                        },
                                    );
                                    responses.insert(404, ResponseBodyOpenApiSchema::NoBody);
                                } else if let AnalysedType::Result(res) = &typ.element_type {
                                    if let Some(ok_type) = &res.ok {
                                        let ok_schema = create_schema_from_analysed_type(ok_type);
                                        responses.insert(
                                            200,
                                            ResponseBodyOpenApiSchema::Known {
                                                schema: Box::new(ok_schema),
                                                content_type: "application/json".to_string(),
                                            },
                                        );
                                    } else {
                                        responses.insert(200, ResponseBodyOpenApiSchema::Unknown);
                                    }

                                    if let Some(err_type) = &res.err {
                                        let err_schema = create_schema_from_analysed_type(err_type);
                                        responses.insert(
                                            500,
                                            ResponseBodyOpenApiSchema::Known {
                                                schema: Box::new(err_schema),
                                                content_type: "application/json".to_string(),
                                            },
                                        );
                                        responses.insert(500, ResponseBodyOpenApiSchema::NoBody);
                                    } else {
                                        responses.insert(500, ResponseBodyOpenApiSchema::Unknown);
                                    }
                                } else {
                                    let schema =
                                        create_schema_from_analysed_type(&typ.element_type);
                                    responses.insert(
                                        200,
                                        ResponseBodyOpenApiSchema::Known {
                                            schema: Box::new(schema),
                                            content_type: "application/json".to_string(),
                                        },
                                    );
                                }
                            }
                            ElementSchema::UnstructuredBinary(binary_descriptor) => {
                                let content_type = binary_descriptor
                                    .restrictions
                                    .as_ref()
                                    .and_then(|types| types.first())
                                    .map(|bt| bt.mime_type.clone())
                                    .unwrap_or_else(|| "application/octet-stream".to_string());

                                let schema = Schema {
                                    schema_data: SchemaData::default(),
                                    schema_kind: SchemaKind::Type(Type::String(StringType {
                                        format: VariantOrUnknownOrEmpty::Item(StringFormat::Binary),
                                        pattern: None,
                                        enumeration: Vec::new(),
                                        min_length: None,
                                        max_length: None,
                                    })),
                                };
                                responses.insert(
                                    200,
                                    ResponseBodyOpenApiSchema::Known {
                                        schema: Box::new(schema),
                                        content_type,
                                    },
                                );
                            }
                            _ => {
                                responses.insert(200, ResponseBodyOpenApiSchema::Unknown);
                            }
                        }
                    }
                    _ => {
                        responses.insert(200, ResponseBodyOpenApiSchema::Unknown);
                    }
                },
                DataSchema::Multimodal(_) => {
                    responses.insert(200, ResponseBodyOpenApiSchema::Unknown);
                }
            }
        }
        RichRouteBehaviour::CorsPreflight(cors_preflight_behaviour) => {
            responses.insert(204, ResponseBodyOpenApiSchema::NoBody);

            let string_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            };

            headers.insert(
                "Access-Control-Allow-Origin".to_string(),
                string_schema.clone(),
            );

            headers.insert(
                "Access-Control-Allow-Headers".to_string(),
                string_schema.clone(),
            );

            let allowed_methods_enum: Vec<Option<String>> = cors_preflight_behaviour
                .allowed_methods
                .iter()
                .map(|m| {
                    Some(match m {
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
                    })
                })
                .collect();

            headers.insert(
                "Access-Control-Allow-Methods".to_string(),
                Schema {
                    schema_data: Default::default(),
                    schema_kind: SchemaKind::Type(Type::String(StringType {
                        format: VariantOrUnknownOrEmpty::Empty,
                        pattern: None,
                        enumeration: allowed_methods_enum,
                        min_length: None,
                        max_length: None,
                    })),
                },
            );
        }
        RichRouteBehaviour::WebhookCallback(_) => {
            // based on runtime behaviour of webhook
            responses.insert(204, ResponseBodyOpenApiSchema::NoBody);
            responses.insert(404, ResponseBodyOpenApiSchema::NoBody);
        }
        RichRouteBehaviour::OpenApiSpec(_) => {
            let schema = openapiv3::Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                    properties: Default::default(),
                    required: Default::default(),
                    additional_properties: Some(AdditionalProperties::Any(true)),
                    min_properties: None,
                    max_properties: None,
                })),
            };

            responses.insert(
                200,
                ResponseBodyOpenApiSchema::Known {
                    schema: Box::new(schema),
                    content_type: "application/json".to_string(),
                },
            );
        }
        RichRouteBehaviour::OidcCallback(_) => {
            let string_schema = Schema {
                schema_data: Default::default(),
                schema_kind: SchemaKind::Type(Type::String(StringType {
                    format: VariantOrUnknownOrEmpty::Empty,
                    pattern: None,
                    enumeration: vec![],
                    min_length: None,
                    max_length: None,
                })),
            };

            headers.insert("Set-Cookie".to_string(), string_schema.clone());
            headers.insert("Location".to_string(), string_schema);
        }
    }

    AgentResponseOpenApiSchema {
        body_and_status_codes: responses,
        headers,
    }
}
