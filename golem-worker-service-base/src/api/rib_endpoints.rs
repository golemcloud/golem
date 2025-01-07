use poem::{
    Route,
    EndpointExt,
    Endpoint,
    IntoEndpoint,
};
use serde_json::Value;
use poem_openapi::{
    OpenApi,
    payload::Json,
    Object,
    param::{Path, Query},
    Tags,
    OpenApiService,
};
use poem_openapi::registry::Registry;
use serde::{Serialize, Deserialize};
use golem_wasm_ast::analysis::{
    AnalysedType, TypeRecord, NameTypePair, TypeBool, TypeU32, TypeU64,
    TypeF64, TypeStr, TypeList, TypeOption,
};
use crate::gateway_api_definition::http::rib_converter::RibConverter;
use super::routes::create_cors_middleware;

#[derive(Object)]
struct HealthResponse {
    status: String,
    data: Value,
}

#[derive(Object)]
struct VersionResponse {
    status: String,
    data: RibVersionData,
}

#[derive(Object)]
struct RibVersionData {
    version_str: String,
}

#[derive(Object)]
struct PrimitiveTypesResponse {
    status: String,
    data: Value,
}

#[derive(Object)]
struct UserProfileResponse {
    status: String,
    data: Value,
}

#[derive(Object, Serialize, Deserialize)]
struct UserSettingsRequest {
    theme: String,
    notifications_enabled: bool,
}

#[derive(Object)]
struct UserSettingsResponse {
    status: String,
    data: Value,
}

#[derive(Object, Serialize, Deserialize)]
struct ContentRequest {
    title: String,
    body: String,
}

#[derive(Object)]
struct ContentResponse {
    status: String,
    data: Value,
}

#[derive(Object, Serialize, Deserialize)]
struct SearchRequest {
    query: String,
    filters: Option<SearchFilters>,
}

#[derive(Object, Serialize, Deserialize)]
struct SearchFilters {
    #[oai(rename = "type")]
    filter_type: Option<String>,
    date_range: Option<DateRange>,
}

#[derive(Object, Serialize, Deserialize)]
struct DateRange {
    start: String,
    end: String,
}

#[derive(Object)]
struct SearchResponse {
    status: String,
    data: SearchResponseData,
}

#[derive(Object)]
struct SearchResponseData {
    matches: Vec<Value>,
    total_count: u32,
    execution_time_ms: u32,
}

#[derive(Object, Serialize, Deserialize)]
struct BatchRequest {
    items: Vec<BatchItem>,
}

#[derive(Object, Serialize, Deserialize)]
struct BatchItem {
    id: u32,
    action: String,
}

#[derive(Object)]
struct BatchResponse {
    status: String,
    data: BatchResponseData,
}

#[derive(Object)]
struct BatchResponseData {
    successful: Vec<Value>,
    failed: Vec<Value>,
}

#[derive(Object)]
struct BatchStatusResponse {
    status: String,
    data: BatchStatusData,
}

#[derive(Object)]
struct BatchStatusData {
    status: String,
    progress: u32,
    successful: u32,
    failed: u32,
}

#[derive(Object, Serialize, Deserialize)]
struct TransformRequest {
    input: String,
    transformations: Vec<Transformation>,
}

#[derive(Object, Serialize, Deserialize)]
struct Transformation {
    #[oai(rename = "type")]
    transform_type: String,
}

#[derive(Object)]
struct TransformResponse {
    status: String,
    data: TransformResponseData,
}

#[derive(Object)]
struct TransformResponseData {
    success: bool,
    output: Vec<Value>,
    metrics: TransformMetrics,
}

#[derive(Object)]
struct TransformMetrics {
    input_size: u32,
    output_size: u32,
    duration_ms: u32,
}

#[derive(Object, Serialize, Deserialize)]
struct TreeRequest {
    root: TreeNode,
}

#[derive(Object, Serialize, Deserialize)]
struct TreeNode {
    value: String,
    children: Vec<TreeNode>,
}

#[derive(Object)]
struct TreeResponse {
    status: String,
    data: TreeResponseData,
}

#[derive(Object)]
struct TreeResponseData {
    id: u32,
    node: TreeNode,
    metadata: TreeMetadata,
}

#[derive(Object)]
struct TreeMetadata {
    created_at: u64,
    modified_at: u64,
    tags: Vec<String>,
}

#[derive(Tags)]
enum ApiTags {
    #[oai(rename = "RIB API")]
    /// Runtime Interface Builder (RIB) API provides endpoints for managing and converting runtime interfaces,
    /// supporting complex type operations, batch processing, and tree-based data structures.
    RIB,
}

/// RIB API implementation
#[derive(Debug, Clone)]
pub struct RibApi;

impl RibApi {
    pub fn new() -> Self {
        RibApi
    }
}

#[derive(Object, Serialize, Deserialize)]
struct ComplexNestedTypes {
    optional_numbers: Vec<Option<i32>>,
    feature_flags: u32,
    nested_data: NestedData,
}

#[derive(Object, Serialize, Deserialize)]
struct NestedData {
    name: String,
    values: Vec<StringValue>,
    metadata: Option<String>,
}

#[derive(Object, Serialize, Deserialize)]
struct StringValue {
    string_val: String,
}

#[derive(Object)]
struct ComplexNestedTypesResponse {
    status: String,
    data: Value,
}

#[OpenApi]
impl RibApi {
    /// Get health status
    #[oai(path = "/healthcheck", method = "get", tag = "ApiTags::RIB")]
    async fn healthcheck(&self) -> Json<HealthResponse> {
        Json(HealthResponse {
            status: "success".to_string(),
            data: serde_json::json!({}),
        })
    }

    /// Get version information
    #[oai(
        path = "/version",
        method = "get",
        tag = "ApiTags::RIB"
    )]
    async fn version(&self) -> Json<VersionResponse> {
        Json(VersionResponse {
            status: "success".to_string(),
            data: RibVersionData {
                version_str: env!("CARGO_PKG_VERSION").to_string(),
            },
        })
    }

    /// Get primitive types schema
    #[oai(
        path = "/primitives",
        method = "get",
        tag = "ApiTags::RIB"
    )]
    async fn get_primitive_types(&self) -> Json<PrimitiveTypesResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        let record_type = AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "bool_val".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
                NameTypePair {
                    name: "u32_val".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "f64_val".to_string(),
                    typ: AnalysedType::F64(TypeF64),
                },
                NameTypePair {
                    name: "string_val".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        });
        
        let schema = match converter.convert_type(&record_type, &mut registry) {
            Ok(schema) => schema,
            Err(e) => {
                return Json(PrimitiveTypesResponse {
                    status: "error".to_string(),
                    data: serde_json::json!({
                        "error": format!("Failed to convert type: {}", e)
                    }),
                });
            }
        };
            
        Json(PrimitiveTypesResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "schema": schema,
                "example": {
                    "bool_val": true,
                    "u32_val": 42,
                    "f64_val": 3.14,
                    "string_val": "Hello RIB!"
                }
            }),
        })
    }

    /// Create primitive types
    #[oai(
        path = "/primitives",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn create_primitive_types(&self, body: Json<Value>) -> Json<PrimitiveTypesResponse> {
        Json(PrimitiveTypesResponse {
            status: "success".to_string(),
            data: body.0,
        })
    }

    /// Get user profile
    #[oai(
        path = "/users/:id/profile",
        method = "get",
        tag = "ApiTags::RIB"
    )]
    async fn get_user_profile(
        &self,
        #[oai(name = "id")] id: Path<u32>
    ) -> Json<UserProfileResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Create settings type
        let settings_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "theme".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "notifications_enabled".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
            ],
        };

        // Create permissions type
        let permissions_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "can_read".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
                NameTypePair {
                    name: "can_write".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
            ],
        };

        // Create profile type
        let profile_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "settings".to_string(),
                    typ: AnalysedType::Record(settings_type),
                },
                NameTypePair {
                    name: "permissions".to_string(),
                    typ: AnalysedType::Record(permissions_type),
                },
            ],
        };

        let schema = match converter.convert_type(&AnalysedType::Record(profile_type), &mut registry) {
            Ok(schema) => schema,
            Err(e) => {
                return Json(UserProfileResponse {
                    status: "error".to_string(),
                    data: serde_json::json!({
                        "error": format!("Failed to convert type: {}", e)
                    }),
                });
            }
        };

        let profile = serde_json::json!({
            "id": *id,
            "settings": {
                "theme": "light",
                "notifications_enabled": true
            },
            "permissions": {
                "can_read": true,
                "can_write": true
            }
        });

        Json(UserProfileResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "schema": schema,
                "profile": profile
            }),
        })
    }

    /// Update user settings
    #[oai(
        path = "/users/:id/settings",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn update_user_settings(
        &self,
        #[oai(name = "id")] id: Path<u32>,
        body: Json<UserSettingsRequest>
    ) -> Json<UserSettingsResponse> {
        Json(UserSettingsResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "id": *id,
                "settings": body.0
            }),
        })
    }

    /// Get user permissions
    #[oai(
        path = "/users/:id/permissions", 
        method = "get", 
        tag = "ApiTags::RIB"
    )]
    async fn get_user_permissions(&self, #[oai(name = "id")] _id: Path<u32>) -> Json<Value> {
        Json(serde_json::json!({
            "status": "success",
            "data": {
                "permissions": {
                    "can_read": true,
                    "can_write": true
                }
            }
        }))
    }

    /// Create content
    #[oai(path = "/content", method = "post", tag = "ApiTags::RIB")]
    async fn create_content(&self, body: Json<ContentRequest>) -> Json<ContentResponse> {
        Json(ContentResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "content": body.0
            }),
        })
    }

    /// Get content by ID
    #[oai(
        path = "/content/:id", 
        method = "get", 
        tag = "ApiTags::RIB"
    )]
    async fn get_content(&self, #[oai(name = "id")] id: Path<u32>) -> Json<ContentResponse> {
        Json(ContentResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "content": {
                    "id": *id,
                    "title": "Sample Content",
                    "body": "This is sample content"
                }
            }),
        })
    }

    /// Search content
    #[oai(
        path = "/search",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn perform_search(&self, body: Json<SearchRequest>) -> Json<SearchResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Convert search request type
        let search_request_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "query".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "filters".to_string(),
                    typ: AnalysedType::Option(TypeOption { inner: Box::new(AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "type".to_string(),
                                typ: AnalysedType::Option(TypeOption { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                            },
                            NameTypePair {
                                name: "date_range".to_string(),
                                typ: AnalysedType::Option(TypeOption { inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair {
                                            name: "start".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        },
                                        NameTypePair {
                                            name: "end".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        },
                                    ],
                                })) }),
                            },
                        ],
                    })) }),
                },
            ],
        };

        let request_schema = converter.convert_type(&AnalysedType::Record(search_request_type), &mut registry);
        
        // Convert search response type
        let search_response_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "matches".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
                NameTypePair {
                    name: "total_count".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "execution_time_ms".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        };

        let response_schema = converter.convert_type(&AnalysedType::Record(search_response_type), &mut registry);
        
        Json(SearchResponse {
            status: "success".to_string(),
            data: SearchResponseData {
                matches: vec![serde_json::json!({
                    "request_schema": request_schema,
                    "response_schema": response_schema,
                    "query": body.0.query,
                    "filters": body.0.filters,
                })],
                total_count: 1,
                execution_time_ms: 0,
            },
        })
    }

    /// Validate search query
    #[oai(
        path = "/search/validate",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn validate_search(&self, _body: Json<Value>) -> Json<Value> {
        Json(serde_json::json!({
            "status": "success",
            "data": {
                "valid": true
            }
        }))
    }

    /// Process batch operation
    #[oai(
        path = "/batch/process",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn batch_process(&self, body: Json<BatchRequest>) -> Json<BatchResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Convert batch request type
        let batch_item_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "action".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        };

        let batch_request_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "items".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Record(batch_item_type)) }),
                },
            ],
        };

        let request_schema = converter.convert_type(&AnalysedType::Record(batch_request_type), &mut registry);

        // Convert batch response type
        let batch_response_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "successful".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
                NameTypePair {
                    name: "failed".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
            ],
        };

        let response_schema = converter.convert_type(&AnalysedType::Record(batch_response_type), &mut registry);
        
        Json(BatchResponse {
            status: "success".to_string(),
            data: BatchResponseData {
                successful: vec![serde_json::json!({
                    "request_schema": request_schema,
                    "response_schema": response_schema,
                    "items": body.0.items,
                })],
                failed: vec![],
            },
        })
    }

    /// Validate batch operation
    #[oai(
        path = "/batch/validate",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn batch_validate(&self, _body: Json<Value>) -> Json<Value> {
        Json(serde_json::json!({
            "status": "success",
            "data": {
                "valid": true
            }
        }))
    }

    /// Get batch operation status
    #[oai(
        path = "/batch/:id/status",
        method = "get",
        tag = "ApiTags::RIB"
    )]
    async fn get_batch_status(
        &self,
        #[oai(name = "id")] _id: Path<u32>
    ) -> Json<BatchStatusResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Convert batch status type
        let batch_status_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "status".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "progress".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "successful".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "failed".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        };

        let _status_schema = converter.convert_type(&AnalysedType::Record(batch_status_type), &mut registry);
        
        Json(BatchStatusResponse {
            status: "success".to_string(),
            data: BatchStatusData {
                status: "in_progress".to_string(),
                progress: 50,
                successful: 5,
                failed: 1,
            },
        })
    }

    /// Apply transformation
    #[oai(
        path = "/transform",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn apply_transformation(&self, body: Json<TransformRequest>) -> Json<TransformResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Convert transform request type
        let transformation_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "type".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        };

        let transform_request_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "input".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "transformations".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Record(transformation_type)) }),
                },
            ],
        };

        let request_schema = converter.convert_type(&AnalysedType::Record(transform_request_type), &mut registry);

        // Convert transform response type
        let transform_metrics_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "input_size".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "output_size".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "duration_ms".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
            ],
        };

        let transform_response_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "success".to_string(),
                    typ: AnalysedType::Bool(TypeBool),
                },
                NameTypePair {
                    name: "output".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
                NameTypePair {
                    name: "metrics".to_string(),
                    typ: AnalysedType::Record(transform_metrics_type),
                },
            ],
        };

        let response_schema = converter.convert_type(&AnalysedType::Record(transform_response_type), &mut registry);
        
        Json(TransformResponse {
            status: "success".to_string(),
            data: TransformResponseData {
                success: true,
                output: vec![serde_json::json!({
                    "request_schema": request_schema,
                    "response_schema": response_schema,
                    "input": body.0.input,
                    "transformations": body.0.transformations,
                })],
                metrics: TransformMetrics {
                    input_size: body.0.input.len() as u32,
                    output_size: 0,
                    duration_ms: 0,
                },
            },
        })
    }

    /// Chain transformations
    #[oai(
        path = "/transform/chain",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn chain_transformations(&self, _body: Json<Value>) -> Json<Value> {
        Json(serde_json::json!({
            "status": "success",
            "data": {
                "success": true,
                "output": [],
                "metrics": {
                    "input_size": 0,
                    "output_size": 0,
                    "duration_ms": 0
                }
            }
        }))
    }

    /// Create tree
    #[oai(
        path = "/tree",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn create_tree(&self, body: Json<TreeRequest>) -> Json<TreeResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Convert tree node type
        let tree_node_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "children".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "value".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                            NameTypePair {
                                name: "children".to_string(),
                                typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                            },
                        ],
                    })) }),
                },
            ],
        };

        let tree_request_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "root".to_string(),
                    typ: AnalysedType::Record(tree_node_type.clone()),
                },
            ],
        };

        let _request_schema = converter.convert_type(&AnalysedType::Record(tree_request_type), &mut registry);

        // Convert tree response type
        let tree_metadata_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "created_at".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "modified_at".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "tags".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
            ],
        };

        let tree_response_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "node".to_string(),
                    typ: AnalysedType::Record(tree_node_type),
                },
                NameTypePair {
                    name: "metadata".to_string(),
                    typ: AnalysedType::Record(tree_metadata_type),
                },
            ],
        };

        let _response_schema = converter.convert_type(&AnalysedType::Record(tree_response_type), &mut registry);
        
        Json(TreeResponse {
            status: "success".to_string(),
            data: TreeResponseData {
                id: 1,
                node: body.0.root,
                metadata: TreeMetadata {
                    created_at: 1234567890,
                    modified_at: 1234567890,
                    tags: vec!["test".to_string()],
                },
            },
        })
    }

    /// Query tree
    #[oai(
        path = "/tree/:id", 
        method = "get", 
        tag = "ApiTags::RIB"
    )]
    async fn query_tree(&self, #[oai(name = "id")] id: Path<u32>, depth: Query<Option<u32>>) -> Json<TreeResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();
        
        // Define the recursive tree node type
        let child_node_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "children".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
            ],
        };

        // Create the parent tree node type
        let tree_node_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "value".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "children".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Record(child_node_type)) }),
                },
            ],
        };

        // Convert tree response type
        let tree_metadata_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "created_at".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "modified_at".to_string(),
                    typ: AnalysedType::U64(TypeU64),
                },
                NameTypePair {
                    name: "tags".to_string(),
                    typ: AnalysedType::List(TypeList { inner: Box::new(AnalysedType::Str(TypeStr)) }),
                },
            ],
        };

        let tree_response_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "id".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "node".to_string(),
                    typ: AnalysedType::Record(tree_node_type),
                },
                NameTypePair {
                    name: "metadata".to_string(),
                    typ: AnalysedType::Record(tree_metadata_type),
                },
            ],
        };

        let _response_schema = converter.convert_type(&AnalysedType::Record(tree_response_type), &mut registry);
        
        // Create a sample tree with depth based on the query parameter
        let mut node = TreeNode {
            value: "root".to_string(),
            children: vec![],
        };

        let depth = depth.0.unwrap_or(1);
        if depth >= 1 {
            node.children = vec![
                TreeNode {
                    value: "child1".to_string(),
                    children: if depth >= 2 {
                        vec![
                            TreeNode {
                                value: "grandchild1".to_string(),
                                children: vec![],
                            },
                        ]
                    } else {
                        vec![]
                    },
                },
            ];
        }

        Json(TreeResponse {
            status: "success".to_string(),
            data: TreeResponseData {
                id: *id,
                node,
                metadata: TreeMetadata {
                    created_at: 1234567890,
                    modified_at: 1234567890,
                    tags: vec!["test".to_string()],
                },
            },
        })
    }

    /// Modify tree
    #[oai(
        path = "/tree/modify",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn modify_tree(&self, _body: Json<Value>) -> Json<Value> {
        Json(serde_json::json!({
            "status": "success",
            "data": {
                "success": true,
                "operation_type": "insert",
                "nodes_affected": 1
            }
        }))
    }

    /// Export API definition
    #[oai(
        path = "/api/definitions/:api_id/version/:version/export", 
        method = "get", 
        tag = "ApiTags::RIB"
    )]
    async fn export_api_definition(&self, #[oai(name = "api_id")] api_id: Path<String>, #[oai(name = "version")] version: Path<String>) -> Json<Value> {
        let service = OpenApiService::new(
            RibApi::new(),
            format!("{} API", api_id.0),
            version.0.clone()
        )
        .server("http://localhost:3000");

        let spec = service.spec();
        Json(serde_json::from_str(&spec).unwrap())
    }

    /// Handle complex nested types
    #[oai(
        path = "/complex-nested",
        method = "post",
        tag = "ApiTags::RIB"
    )]
    async fn handle_complex_nested(&self, body: Json<ComplexNestedTypes>) -> Json<ComplexNestedTypesResponse> {
        let mut converter = RibConverter::new();
        let mut registry = Registry::new();

        // Define the string value type
        let string_value_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "string_val".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
            ],
        };

        // Define the nested data type
        let nested_data_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "name".to_string(),
                    typ: AnalysedType::Str(TypeStr),
                },
                NameTypePair {
                    name: "values".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(string_value_type)),
                    }),
                },
                NameTypePair {
                    name: "metadata".to_string(),
                    typ: AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    }),
                },
            ],
        };

        // Define the complex nested type
        let complex_type = TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "optional_numbers".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Option(TypeOption {
                            inner: Box::new(AnalysedType::S32(golem_wasm_ast::analysis::TypeS32)),
                        })),
                    }),
                },
                NameTypePair {
                    name: "feature_flags".to_string(),
                    typ: AnalysedType::U32(TypeU32),
                },
                NameTypePair {
                    name: "nested_data".to_string(),
                    typ: AnalysedType::Record(nested_data_type),
                },
            ],
        };

        let schema = match converter.convert_type(&AnalysedType::Record(complex_type), &mut registry) {
            Ok(schema) => schema,
            Err(e) => {
                return Json(ComplexNestedTypesResponse {
                    status: "error".to_string(),
                    data: serde_json::json!({
                        "error": format!("Failed to convert type: {}", e)
                    }),
                });
            }
        };

        Json(ComplexNestedTypesResponse {
            status: "success".to_string(),
            data: serde_json::json!({
                "schema": schema,
                "received_data": body.0,
            }),
        })
    }

    /// Export OpenAPI specification
    /// 
    /// Returns the OpenAPI specification for the RIB (Runtime Interface Builder) API.
    /// This endpoint provides a complete API schema that can be used for documentation,
    /// client generation, and API exploration through tools like Swagger UI.
    #[oai(path = "/export", method = "get", tag = "ApiTags::RIB")]
    async fn export_api(&self) -> Json<Value> {
        use crate::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};
        
        let exporter = OpenApiExporter;
        let format = OpenApiFormat::default();
        let spec = exporter.export_openapi(RibApi::new(), &format);
        
        Json(serde_json::from_str(&spec).unwrap())
    }
}

pub fn rib_routes() -> impl Endpoint {
    let api_service = OpenApiService::new(RibApi::new(), "RIB API", env!("CARGO_PKG_VERSION"))
        .server("http://localhost:3000")
        .description("Runtime Interface Builder (RIB) API provides endpoints for managing and converting runtime interfaces, supporting complex type operations, batch processing, and tree-based data structures.")
        .url_prefix("/api");

    Route::new()
        .nest("/api", api_service.clone().with(create_cors_middleware()))
        .nest("/api/openapi", api_service.spec_endpoint().with(create_cors_middleware()))
        .nest("/swagger-ui/rib", api_service.swagger_ui().with(create_cors_middleware()))
        .with(poem::middleware::AddData::new(()))
        .with(create_cors_middleware())
        .into_endpoint()
}