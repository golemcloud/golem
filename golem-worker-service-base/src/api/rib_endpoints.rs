use poem::{
    handler,
    web::{Json, Path, Query},
    Result,
    Route,
};
use golem_wasm_ast::analysis::*;
use crate::gateway_api_definition::http::{
    rib_converter::RibConverter,
    openapi_export::{OpenApiExporter, OpenApiFormat},
};
use serde_json::Value;
use poem_openapi::{OpenApi};
use utoipa::openapi::OpenApi as UtoipaOpenApi;
use serde::Deserialize;

pub struct RibApi;

#[OpenApi]
impl RibApi {
    /// Get health status
    #[oai(path = "/api/v1/rib/healthcheck", method = "get")]
    async fn healthcheck(&self) -> poem_openapi::payload::Json<Value> {
        poem_openapi::payload::Json(serde_json::json!({
            "status": "success",
            "data": {}
        }))
    }

    /// Get version information
    #[oai(path = "/api/v1/rib/version", method = "get")]
    async fn version(&self) -> poem_openapi::payload::Json<Value> {
        poem_openapi::payload::Json(serde_json::json!({
            "status": "success",
            "data": {
                "version": env!("CARGO_PKG_VERSION")
            }
        }))
    }
}

impl RibApi {
    pub fn new() -> Self {
        RibApi
    }
}

pub fn rib_routes() -> Route {
    Route::new()
        // Basic endpoints
        .at("/healthcheck", poem::get(healthcheck))
        .at("/version", poem::get(version))
        
        // Primitive types demo
        .at("/primitives", poem::get(get_primitive_types).post(create_primitive_types))
        
        // User management
        .at("/users/:id/profile", poem::get(get_user_profile))
        .at("/users/:id/settings", poem::post(update_user_settings))
        .at("/users/:id/permissions", poem::get(get_user_permissions))
        
        // Content handling
        .at("/content", poem::post(create_content))
        .at("/content/:id", poem::get(get_content))
        
        // Search functionality
        .at("/search", poem::post(perform_search))
        .at("/search/validate", poem::post(validate_search))
        
        // Batch operations
        .at("/batch/process", poem::post(batch_process))
        .at("/batch/validate", poem::post(batch_validate))
        .at("/batch/:id/status", poem::get(get_batch_status))
        
        // Data transformations
        .at("/transform", poem::post(apply_transformation))
        .at("/transform/chain", poem::post(chain_transformations))
        
        // Tree operations
        .at("/tree/modify", poem::post(modify_tree))
        .at("/tree/:id", poem::get(query_tree))
        .at("/tree", poem::post(create_tree))

        // Export API definition
        .at("/v1/api/definitions/:api_id/version/:version/export", poem::get(export_api_definition))
}

// Basic endpoints
#[handler]
async fn healthcheck() -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {}
    })))
}

#[handler]
async fn version() -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "version": env!("CARGO_PKG_VERSION")
        }
    })))
}

// Primitive types endpoints
#[handler]
async fn get_primitive_types() -> Result<Json<Value>> {
    let converter = RibConverter;
    
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
    
    let schema = converter.convert_type(&record_type)
        .expect("Failed to convert primitive types");
        
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "schema": schema,
            "example": {
                "bool_val": true,
                "u32_val": 42,
                "f64_val": 3.14,
                "string_val": "Hello RIB!"
            }
        }
    })))
}

#[handler]
async fn create_primitive_types(body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": body.0
    })))
}

// User profile endpoints
#[handler]
async fn get_user_profile(Path(id): Path<u32>) -> Result<Json<Value>> {
    let converter = RibConverter;
    
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

    let schema = converter.convert_type(&AnalysedType::Record(profile_type))
        .expect("Failed to convert profile type");

    let profile = serde_json::json!({
        "id": id,
        "settings": {
            "theme": "light",
            "notifications_enabled": true
        },
        "permissions": {
            "can_read": true,
            "can_write": true
        }
    });

    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "schema": schema,
            "profile": profile
        }
    })))
}

#[handler]
async fn update_user_settings(Path(id): Path<u32>, body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "id": id,
            "settings": body.0
        }
    })))
}

#[handler]
async fn get_user_permissions(Path(_id): Path<u32>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "permissions": {
                "can_read": true,
                "can_write": true,
                "can_delete": false,
                "is_admin": false
            }
        }
    })))
}

// Content endpoints
#[handler]
async fn create_content(body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": body.0
    })))
}

#[handler]
async fn get_content(Path(id): Path<u32>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "content": {
                "id": id,
                "title": "Sample Content",
                "body": "This is sample content"
            }
        }
    })))
}

// Search endpoints
#[handler]
async fn perform_search(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "matches": [],
            "total_count": 0,
            "execution_time_ms": 0
        }
    })))
}

#[handler]
async fn validate_search(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "valid": true
        }
    })))
}

// Batch endpoints
#[handler]
async fn batch_process(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "successful": [],
            "failed": []
        }
    })))
}

#[handler]
async fn batch_validate(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "valid": true
        }
    })))
}

#[handler]
async fn get_batch_status(Path(_id): Path<u32>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "status": "in_progress",
            "progress": 50,
            "successful": 5,
            "failed": 1
        }
    })))
}

// Transform endpoints
#[handler]
async fn apply_transformation(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
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
    })))
}

#[handler]
async fn chain_transformations(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
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
    })))
}

// Tree endpoints
#[handler]
async fn create_tree(body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": body.0
    })))
}

#[derive(Deserialize)]
struct TreeQueryParams {
    depth: Option<u32>,
}

#[handler]
async fn query_tree(Path(id): Path<u32>, params: Query<TreeQueryParams>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "id": id,
            "depth": params.depth.unwrap_or(1),
            "node": {
                "id": id,
                "value": "root",
                "children": [],
                "metadata": {
                    "created_at": 1234567890,
                    "modified_at": 1234567890,
                    "tags": ["test"]
                }
            }
        }
    })))
}

#[handler]
async fn modify_tree(_body: Json<Value>) -> Result<Json<Value>> {
    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "success": true,
            "operation_type": "insert",
            "nodes_affected": 1
        }
    })))
}

// Export API endpoint
#[handler]
async fn export_api_definition(Path((api_id, api_version)): Path<(String, String)>) -> Result<Json<Value>> {
    let info = utoipa::openapi::InfoBuilder::new()
        .title(format!("{} API", api_id))
        .version(api_version.clone())
        .build();

    let paths = utoipa::openapi::Paths::new();
    let openapi = UtoipaOpenApi::new(info, paths);

    let exporter = OpenApiExporter;
    let format = OpenApiFormat { json: true };
    let _openapi_json = exporter.export_openapi(&api_id, &api_version, openapi, &format);

    Ok(Json(serde_json::json!({
        "status": "success",
        "data": {
            "openapi": "3.1.0",
            "info": {
                "title": format!("{} API", api_id),
                "version": api_version
            }
        }
    })))
} 