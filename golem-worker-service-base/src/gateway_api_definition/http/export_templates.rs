use serde_json::Value;
use golem_wasm_ast::analysis::*;
use super::rib_converter::RibConverter;
use poem_openapi::registry::Registry;

/// Returns a template OpenAPI spec for a storage-like API service
pub fn get_storage_api_template() -> Value {
    // Define bucket type using RIB types
    let bucket_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "name".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
            NameTypePair {
                name: "created".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
            NameTypePair {
                name: "location".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
            NameTypePair {
                name: "storage_class".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    });

    let bucket_list_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "items".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(bucket_type.clone()),
                }),
            },
            NameTypePair {
                name: "next_page_token".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    });

    // Convert RIB types to OpenAPI using RibConverter
    let converter = RibConverter;
    let bucket_schema = converter.convert_type(&bucket_type)
        .expect("Failed to convert bucket type");
    let bucket_list_schema = converter.convert_type(&bucket_list_type)
        .expect("Failed to convert bucket list type");

    // Build the OpenAPI spec
    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Storage API Template",
            "description": "Template API for storage service implementation using RIB types",
            "version": "1.0.0"
        },
        "paths": {
            "/api/v1/buckets": {
                "get": {
                    "tags": ["Buckets"],
                    "summary": "List buckets in a project",
                    "parameters": [
                        {
                            "name": "project",
                            "in": "query",
                            "description": "Project ID",
                            "required": true,
                            "schema": {
                                "type": "string"
                            },
                            "example": "my-project-123"
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "List of buckets",
                            "content": {
                                "application/json": {
                                    "schema": bucket_list_schema,
                                    "example": {
                                        "items": [
                                            {
                                                "name": "my-files",
                                                "id": "bucket-1",
                                                "created": "2024-01-02T12:00:00Z",
                                                "location": "us-east-1",
                                                "storage_class": "STANDARD"
                                            }
                                        ],
                                        "next_page_token": "next-page-token-abc"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Bucket": bucket_schema,
                "BucketList": bucket_list_schema
            }
        }
    })
}

/// Returns a template OpenAPI spec for a complex data handling API
pub fn get_complex_api_template() -> Value {
    // Define status type using RIB types
    let status_type = AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "Active".to_string(),
                typ: None,
            },
            NameOptionTypePair {
                name: "Inactive".to_string(),
                typ: Some(AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "reason".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                    ],
                })),
            },
        ],
    });

    let complex_request_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "name".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "flags".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Bool(TypeBool)),
                }),
            },
            NameTypePair {
                name: "status".to_string(),
                typ: status_type.clone(),
            },
        ],
    });

    let api_response_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "success".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
            NameTypePair {
                name: "received".to_string(),
                typ: complex_request_type.clone(),
            },
        ],
    });

    // Convert RIB types to OpenAPI using RibConverter
    let converter = RibConverter;
    let mut registry = Registry::new();
    
    let status_schema = converter.convert_type(&status_type, &mut registry)
        .expect("Failed to convert status type");
    let request_schema = converter.convert_type(&complex_request_type, &mut registry)
        .expect("Failed to convert request type");
    let response_schema = converter.convert_type(&api_response_type, &mut registry)
        .expect("Failed to convert response type");

    // Build the OpenAPI spec
    serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Complex Data API Template",
            "description": "Template API for handling complex data structures using RIB types",
            "version": "1.0.0"
        },
        "paths": {
            "/api/v1/complex": {
                "post": {
                    "operationId": "handle_complex_request",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": request_schema,
                                "example": {
                                    "id": 42,
                                    "name": "Example Request",
                                    "flags": [true, false, true],
                                    "status": {
                                        "discriminator": "Active"
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "Success response",
                            "content": {
                                "application/json": {
                                    "schema": response_schema,
                                    "example": {
                                        "success": true,
                                        "received": {
                                            "id": 42,
                                            "name": "Example Request",
                                            "flags": [true, false, true],
                                            "status": {
                                                "discriminator": "Active"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Status": status_schema,
                "ComplexRequest": request_schema,
                "ApiResponse": response_schema
            }
        }
    })
}

/// Returns a list of available API templates with their descriptions
pub fn get_available_templates() -> Vec<(&'static str, &'static str)> {
    vec![
        ("storage", "A storage service API template similar to cloud storage services, built with RIB types"),
        ("complex", "A template demonstrating complex data structures and request handling using RIB types")
    ]
} 