use poem::{Route, EndpointExt, Endpoint, IntoEndpoint};
use poem_openapi::*;
use poem_openapi::payload::Json;
use poem_openapi::types::{Type, ParseFromJSON, ToJSON};
use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
use golem_wasm_ast::analysis::*;
use crate::gateway_api_definition::http::rib_converter::RibConverter;
use poem::error::BadRequest;
use poem::Result;
use std::error::Error as StdError;
use crate::api::wit_types_api::WitTypesApiTags::WitTypes;
use serde::{Serialize, Deserialize};
use serde_json::{Value, Value as JsonValue};
use std::borrow::Cow;
use super::routes::create_cors_middleware;

#[derive(Debug)]
struct ConversionError(String);

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Conversion error: {}", self.0)
    }
}

impl StdError for ConversionError {}

/// API for handling WIT type conversions
#[derive(Tags)]
pub enum WitTypesApiTags {
    #[oai(rename = "WIT Types API")]
    /// WebAssembly Interface Types (WIT) API provides endpoints for converting and validating WIT data types, 
    /// handling complex nested structures, and performing type transformations between WIT and OpenAPI formats.
    WitTypes,
}

/// A wrapper around JSON that will be converted to RpcValue when needed
#[derive(Debug, Serialize, Deserialize)]
struct WitValue(JsonValue);

impl Type for WitValue {
    const IS_REQUIRED: bool = true;
    type RawValueType = JsonValue;
    type RawElementValueType = JsonValue;

    fn name() -> Cow<'static, str> {
        Cow::Borrowed("WitValue")
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Inline(Box::new(MetaSchema::new("object")))
    }

    fn register(_registry: &mut Registry) {}

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(&self.0)
    }

    fn raw_element_iter<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(std::iter::empty())
    }
}

impl ParseFromJSON for WitValue {
    fn parse_from_json(value: Option<JsonValue>) -> poem_openapi::types::ParseResult<Self> {
        let json = value.ok_or_else(|| poem_openapi::types::ParseError::custom("Missing value"))?;
        Ok(WitValue(json))
    }
}

impl ToJSON for WitValue {
    fn to_json(&self) -> Option<JsonValue> {
        Some(self.0.clone())
    }
}

/// Raw WIT format input that bypasses OpenAPI validation
#[derive(Debug, Object)]
struct WitInput {
    /// The raw WIT-formatted value to be converted
    value: WitValue,
}

/// Complex nested types for WIT type conversions
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct ComplexNestedTypes {
    /// Optional list of numbers
    #[oai(validator(max_items = 100))]
    pub optional_numbers: Vec<Option<i32>>,
    /// Feature flags as a 32-bit unsigned integer
    pub feature_flags: u32,
    /// Nested data structure
    pub nested_data: NestedData,
}

/// Nested data structure containing a list of values
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct NestedData {
    /// Name field
    pub name: String,
    /// List of value objects
    #[oai(validator(max_items = 100))]
    pub values: Vec<ValueObject>,
    /// Optional metadata string
    pub metadata: Option<String>,
}

/// Value object containing a string value
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct ValueObject {
    /// String value field
    pub string_val: String,
}

/// API for handling WIT type conversions
#[derive(Clone, Debug)]
pub struct WitTypesApi;

/// Create the complex type schema
fn create_complex_type() -> AnalysedType {
    AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "optional_numbers".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Option(TypeOption {
                        inner: Box::new(AnalysedType::S32(TypeS32)),
                    })),
                }),
            },
            NameTypePair {
                name: "feature_flags".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "nested_data".to_string(),
                typ: AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "name".to_string(),
                            typ: AnalysedType::Str(TypeStr),
                        },
                        NameTypePair {
                            name: "values".to_string(),
                            typ: AnalysedType::List(TypeList {
                                inner: Box::new(AnalysedType::Record(TypeRecord {
                                    fields: vec![
                                        NameTypePair {
                                            name: "string_val".to_string(),
                                            typ: AnalysedType::Str(TypeStr),
                                        },
                                    ],
                                })),
                            }),
                        },
                        NameTypePair {
                            name: "metadata".to_string(),
                            typ: AnalysedType::Option(TypeOption {
                                inner: Box::new(AnalysedType::Str(TypeStr)),
                            }),
                        },
                    ],
                }),
            },
        ],
    })
}

/// Primitive types wrapper
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct PrimitiveTypes {
    pub bool_val: bool,
    pub u8_val: u8,
    pub u16_val: u16,
    pub u32_val: u32,
    pub u64_val: u64,
    pub s8_val: i8,
    pub s16_val: i16,
    pub s32_val: i32,
    pub s64_val: i64,
    pub f32_val: f32,
    pub f64_val: f64,
    pub char_val: u32,
    pub string_val: String,
}

/// User settings record
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct UserSettings {
    pub theme: String,
    pub notifications_enabled: bool,
    pub email_frequency: String,
}

/// User permissions flags
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct UserPermissions {
    pub can_read: bool,
    pub can_write: bool,
    pub can_delete: bool,
    pub is_admin: bool,
}

/// User profile with optional fields
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: u32,
    pub username: String,
    pub settings: Option<UserSettings>,
    pub permissions: UserPermissions,
}

/// Complex data for variant
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct ComplexData {
    pub id: u32,
    pub data: Vec<String>,
}

/// Success response
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SuccessResponse {
    pub code: u16,
    pub message: String,
    pub data: Option<String>,
}

/// Error details
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct ErrorDetails {
    pub code: u16,
    pub message: String,
    pub details: Option<Vec<String>>,
}

/// Search query and related types
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SearchFlags {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex_enabled: bool,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct DateRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub items_per_page: u32,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SearchFilters {
    pub categories: Vec<String>,
    pub date_range: Option<DateRange>,
    pub flags: SearchFlags,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub filters: SearchFilters,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SearchMatch {
    pub id: u32,
    pub score: f64,
    pub context: String,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub total_count: u32,
    pub execution_time_ms: u32,
}

/// Batch operation types
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct BatchOptions {
    pub parallel: bool,
    pub retry_count: u32,
    pub timeout_ms: u32,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct BatchResult {
    pub successful: u32,
    pub failed: u32,
    pub errors: Vec<String>,
}

/// Tree operation types
#[derive(Debug, Object, Serialize, Deserialize)]
pub struct NodeMetadata {
    pub created_at: u64,
    pub modified_at: u64,
    pub tags: Vec<String>,
}

#[derive(Debug, Object, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: u32,
    pub value: String,
    pub children: Vec<TreeNode>,
    pub metadata: NodeMetadata,
}

/// Generic input that accepts any JSON value
#[derive(Debug, Object)]
struct GenericWitInput {
    /// Any valid JSON value
    value: WitValue,
}

/// API implementation for WIT types
#[OpenApi]
impl WitTypesApi {
    /// Test endpoint that accepts and returns complex WIT types
    #[oai(path = "/test", method = "post", tag = "WitTypes")]
    async fn test_wit_types(&self, payload: Json<WitInput>) -> Result<Json<ComplexNestedTypes>> {
        let mut converter = RibConverter::new_wit();
        let complex_type = create_complex_type();

        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &complex_type)
            .map_err(|e| BadRequest(ConversionError(e)))?;
        
        // Convert directly from WIT to OpenAPI format
        let complex_result: ComplexNestedTypes = match converter.convert_value(&parsed_value) {
            Ok(value) => match serde_json::from_value(value) {
                Ok(result) => result,
                Err(e) => {
                    println!("Error deserializing to ComplexNestedTypes: {:?}", e);
                    return Err(BadRequest(ConversionError(format!("{:?}", e))));
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(complex_result))
    }

    /// Get a sample of all WIT types
    #[oai(path = "/sample", method = "get", tag = "WitTypes")]
    async fn get_wit_types_sample(&self) -> Json<ComplexNestedTypes> {
        Json(ComplexNestedTypes {
            optional_numbers: vec![Some(42), None, Some(123)],
            feature_flags: 7,
            nested_data: NestedData {
                name: "test_nested".to_string(),
                values: vec![ValueObject {
                    string_val: "test".to_string(),
                }],
                metadata: Some("Additional info".to_string()),
            },
        })
    }

    /// Test primitive types
    #[oai(path = "/primitives", method = "post", tag = "WitTypes")]
    async fn test_primitives(&self, payload: Json<WitInput>) -> Result<Json<PrimitiveTypes>> {
        let mut converter = RibConverter::new_wit();
        converter.set_in_openapi_operation(false);  // Ensure we're in WIT mode
        let primitive_type = create_primitive_type();

        // Debug: Print the input value
        println!("Input value: {}", serde_json::to_string_pretty(&payload.0.value.0).unwrap());

        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &primitive_type)
            .map_err(|e| {
                println!("Error parsing WIT value: {}", e);
                BadRequest(ConversionError(e))
            })?;
        
        // Debug: Print the parsed value
        println!("Parsed value: {:?}", parsed_value);
        
        // Convert directly from WIT to OpenAPI format
        let primitive_result: PrimitiveTypes = match converter.convert_value(&parsed_value) {
            Ok(value) => {
                // Debug: Print the converted value
                println!("Converted value: {}", serde_json::to_string_pretty(&value).unwrap());
                match serde_json::from_value(value) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Error deserializing to PrimitiveTypes: {:?}", e);
                        return Err(BadRequest(ConversionError(format!("{:?}", e))));
                    }
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(primitive_result))
    }

    /// Create user profile
    #[oai(path = "/users/profile", method = "post", tag = "WitTypes")]
    async fn create_user_profile(&self, payload: Json<WitInput>) -> Result<Json<UserProfile>> {
        let mut converter = RibConverter::new_wit();
        converter.set_in_openapi_operation(false);  // Ensure we're in WIT mode
        let profile_type = create_user_profile_type();
        
        // Debug: Print the input value
        println!("Input value: {}", serde_json::to_string_pretty(&payload.0.value.0).unwrap());
        
        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &profile_type)
            .map_err(|e| {
                println!("Error parsing WIT value: {}", e);
                BadRequest(ConversionError(e))
            })?;
        
        // Debug: Print the parsed value
        println!("Parsed value: {:?}", parsed_value);
        
        // Convert directly from WIT to OpenAPI format
        let profile_result: UserProfile = match converter.convert_value(&parsed_value) {
            Ok(value) => {
                // Debug: Print the converted value
                println!("Converted value: {}", serde_json::to_string_pretty(&value).unwrap());
                match serde_json::from_value(value) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Error deserializing to UserProfile: {:?}", e);
                        return Err(BadRequest(ConversionError(format!("{:?}", e))));
                    }
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(profile_result))
    }

    /// Perform search operation
    #[oai(path = "/search", method = "post", tag = "WitTypes")]
    async fn perform_search(&self, payload: Json<WitInput>) -> Result<Json<SearchResult>> {
        let mut converter = RibConverter::new_wit();
        converter.set_in_openapi_operation(false);  // Ensure we're in WIT mode
        let search_type = create_search_type();
        
        // Debug: Print the input value
        println!("Input value: {}", serde_json::to_string_pretty(&payload.0.value.0).unwrap());
        
        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &search_type)
            .map_err(|e| {
                println!("Error parsing WIT value: {}", e);
                BadRequest(ConversionError(e))
            })?;
        
        // Debug: Print the parsed value
        println!("Parsed value: {:?}", parsed_value);
        
        // Convert directly from WIT to OpenAPI format
        let search_result: SearchResult = match converter.convert_value(&parsed_value) {
            Ok(value) => {
                // Debug: Print the converted value
                println!("Converted value: {}", serde_json::to_string_pretty(&value).unwrap());
                match serde_json::from_value(value) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Error deserializing to SearchResult: {:?}", e);
                        return Err(BadRequest(ConversionError(format!("{:?}", e))));
                    }
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(search_result))
    }

    /// Execute batch operation
    #[oai(path = "/batch", method = "post", tag = "WitTypes")]
    async fn execute_batch(&self, payload: Json<WitInput>) -> Result<Json<BatchResult>> {
        let mut converter = RibConverter::new_wit();
        converter.set_in_openapi_operation(false);  // Ensure we're in WIT mode
        let batch_type = create_batch_type();
        
        // Debug: Print the input value
        println!("Input value: {}", serde_json::to_string_pretty(&payload.0.value.0).unwrap());
        
        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &batch_type)
            .map_err(|e| {
                println!("Error parsing WIT value: {}", e);
                BadRequest(ConversionError(e))
            })?;
        
        // Debug: Print the parsed value
        println!("Parsed value: {:?}", parsed_value);
        
        // Convert directly from WIT to OpenAPI format
        let batch_result: BatchResult = match converter.convert_value(&parsed_value) {
            Ok(value) => {
                // Debug: Print the converted value
                println!("Converted value: {}", serde_json::to_string_pretty(&value).unwrap());
                match serde_json::from_value(value) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Error deserializing to BatchResult: {:?}", e);
                        return Err(BadRequest(ConversionError(format!("{:?}", e))));
                    }
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(batch_result))
    }

    /// Create tree node
    #[oai(path = "/tree", method = "post", tag = "WitTypes")]
    async fn create_tree(&self, payload: Json<WitInput>) -> Result<Json<TreeNode>> {
        let mut converter = RibConverter::new_wit();
        converter.set_in_openapi_operation(false);  // Ensure we're in WIT mode
        let tree_type = create_tree_type();
        
        // Debug: Print the input value
        println!("Input value: {}", serde_json::to_string_pretty(&payload.0.value.0).unwrap());
        
        // Parse the input using TypeAnnotatedValue with the correct type information
        let parsed_value = RibConverter::parse_wit_value(&payload.0.value.0, &tree_type)
            .map_err(|e| {
                println!("Error parsing WIT value: {}", e);
                BadRequest(ConversionError(e))
            })?;
        
        // Debug: Print the parsed value
        println!("Parsed value: {:?}", parsed_value);
        
        // Convert directly from WIT to OpenAPI format
        let tree_result: TreeNode = match converter.convert_value(&parsed_value) {
            Ok(value) => {
                // Debug: Print the converted value
                println!("Converted value: {}", serde_json::to_string_pretty(&value).unwrap());
                match serde_json::from_value(value) {
                    Ok(result) => result,
                    Err(e) => {
                        println!("Error deserializing to TreeNode: {:?}", e);
                        return Err(BadRequest(ConversionError(format!("{:?}", e))));
                    }
                }
            },
            Err(e) => {
                println!("Error converting WIT value: {:?}", e);
                return Err(BadRequest(ConversionError(format!("{:?}", e))));
            }
        };
        
        Ok(Json(tree_result))
    }

    /// Get success response
    #[oai(path = "/success", method = "get", tag = "WitTypes")]
    async fn get_success_response(&self) -> Json<SuccessResponse> {
        Json(SuccessResponse {
            code: 200,
            message: "Operation successful".to_string(),
            data: Some("Sample success data".to_string()),
        })
    }

    /// Get error details
    #[oai(path = "/error", method = "get", tag = "WitTypes")]
    async fn get_error_details(&self) -> Json<ErrorDetails> {
        Json(ErrorDetails {
            code: 400,
            message: "Sample error".to_string(),
            details: Some(vec!["Error detail 1".to_string(), "Error detail 2".to_string()]),
        })
    }

    /// Get sample search query
    #[oai(path = "/search/sample", method = "get", tag = "WitTypes")]
    async fn get_search_query_sample(&self) -> Json<SearchQuery> {
        Json(SearchQuery {
            query: "sample search".to_string(),
            filters: SearchFilters {
                categories: vec!["category1".to_string(), "category2".to_string()],
                date_range: Some(DateRange {
                    start: 1000000,
                    end: 2000000,
                }),
                flags: SearchFlags {
                    case_sensitive: true,
                    whole_word: false,
                    regex_enabled: true,
                },
            },
            pagination: Some(Pagination {
                page: 1,
                items_per_page: 10,
            }),
        })
    }

    /// Get sample batch options
    #[oai(path = "/batch/sample", method = "get", tag = "WitTypes")]
    async fn get_batch_options_sample(&self) -> Json<BatchOptions> {
        Json(BatchOptions {
            parallel: true,
            retry_count: 3,
            timeout_ms: 5000,
        })
    }

    /// Convert any JSON structure to OpenAPI format
    #[oai(path = "/convert", method = "post", tag = "WitTypes")]
    async fn convert_json(&self, input: Json<serde_json::Value>) -> Result<Json<serde_json::Value>> {
        let mut converter = RibConverter::new_openapi();
        converter.set_in_openapi_operation(true);
        
        // Parse WIT format JSON
        let typ = infer_type_from_json(&input.0);
        let parsed_value = RibConverter::parse_openapi_value(&input.0, &typ)
            .map_err(|e| BadRequest(ConversionError(format!("Error parsing JSON: {:?}", e))))?;
        
        // Convert using RibConverter
        let converted = converter.convert_value(&parsed_value)
            .map_err(|e| BadRequest(ConversionError(format!("Error converting to OpenAPI format: {:?}", e))))?;
        
        Ok(Json(converted))
    }

    /// Export OpenAPI specification
    /// 
    /// Returns the OpenAPI specification for the WIT (WebAssembly Interface Types) API.
    /// This endpoint provides a complete API schema for WIT type conversions and operations,
    /// which can be used for documentation, client generation, and API exploration through
    /// tools like Swagger UI.
    #[oai(path = "/export", method = "get", tag = "WitTypes")]
    async fn export_api(&self) -> Json<Value> {
        use crate::gateway_api_definition::http::openapi_export::{OpenApiExporter, OpenApiFormat};
        
        let exporter = OpenApiExporter;
        let format = OpenApiFormat::default();
        let spec = exporter.export_openapi(WitTypesApi, &format);
        
        Json(serde_json::from_str(&spec).unwrap())
    }
}

// Helper functions to create WIT types

fn create_primitive_type() -> AnalysedType {
    AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "bool_val".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
            NameTypePair {
                name: "u8_val".to_string(),
                typ: AnalysedType::U8(TypeU8),
            },
            NameTypePair {
                name: "u16_val".to_string(),
                typ: AnalysedType::U16(TypeU16),
            },
            NameTypePair {
                name: "u32_val".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "u64_val".to_string(),
                typ: AnalysedType::U64(TypeU64),
            },
            NameTypePair {
                name: "s8_val".to_string(),
                typ: AnalysedType::S8(TypeS8),
            },
            NameTypePair {
                name: "s16_val".to_string(),
                typ: AnalysedType::S16(TypeS16),
            },
            NameTypePair {
                name: "s32_val".to_string(),
                typ: AnalysedType::S32(TypeS32),
            },
            NameTypePair {
                name: "s64_val".to_string(),
                typ: AnalysedType::S64(TypeS64),
            },
            NameTypePair {
                name: "f32_val".to_string(),
                typ: AnalysedType::F32(TypeF32),
            },
            NameTypePair {
                name: "f64_val".to_string(),
                typ: AnalysedType::F64(TypeF64),
            },
            NameTypePair {
                name: "char_val".to_string(),
                typ: AnalysedType::Chr(TypeChr),
            },
            NameTypePair {
                name: "string_val".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    })
}

fn create_user_profile_type() -> AnalysedType {
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
            NameTypePair {
                name: "email_frequency".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    };

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
            NameTypePair {
                name: "can_delete".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
            NameTypePair {
                name: "is_admin".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
        ],
    };

    AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "username".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "settings".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Record(settings_type)),
                }),
            },
            NameTypePair {
                name: "permissions".to_string(),
                typ: AnalysedType::Record(permissions_type),
            },
        ],
    })
}

fn create_search_type() -> AnalysedType {
    let date_range_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "start".to_string(),
                typ: AnalysedType::U64(TypeU64),
            },
            NameTypePair {
                name: "end".to_string(),
                typ: AnalysedType::U64(TypeU64),
            },
        ],
    };

    let search_flags_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "case_sensitive".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
            NameTypePair {
                name: "whole_word".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
            NameTypePair {
                name: "regex_enabled".to_string(),
                typ: AnalysedType::Bool(TypeBool),
            },
        ],
    };

    let pagination_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "page".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "items_per_page".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
        ],
    };

    let search_filters_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "categories".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
            NameTypePair {
                name: "date_range".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Record(date_range_type)),
                }),
            },
            NameTypePair {
                name: "flags".to_string(),
                typ: AnalysedType::Record(search_flags_type),
            },
        ],
    };

    let search_match_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "score".to_string(),
                typ: AnalysedType::F64(TypeF64),
            },
            NameTypePair {
                name: "context".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    };

    AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "matches".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Record(search_match_type)),
                }),
            },
            NameTypePair {
                name: "total_count".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "execution_time_ms".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "query".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "filters".to_string(),
                typ: AnalysedType::Record(search_filters_type),
            },
            NameTypePair {
                name: "pagination".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Record(pagination_type)),
                }),
            },
        ],
    })
}

fn create_batch_type() -> AnalysedType {
    AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "successful".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "failed".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "errors".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    })
}

fn create_tree_type() -> AnalysedType {
    let metadata_type = TypeRecord {
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
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    };

    let tree_node_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "value".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "children".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Record(TypeRecord {
                        fields: vec![
                            NameTypePair {
                                name: "id".to_string(),
                                typ: AnalysedType::U32(TypeU32),
                            },
                            NameTypePair {
                                name: "value".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                            NameTypePair {
                                name: "metadata".to_string(),
                                typ: AnalysedType::Record(metadata_type.clone()),
                            },
                            NameTypePair {
                                name: "children".to_string(),
                                typ: AnalysedType::List(TypeList {
                                    inner: Box::new(AnalysedType::Record(TypeRecord {
                                        fields: vec![],
                                    })),
                                }),
                            },
                        ],
                    })),
                }),
            },
            NameTypePair {
                name: "metadata".to_string(),
                typ: AnalysedType::Record(metadata_type),
            },
        ],
    };

    AnalysedType::Record(tree_node_type)
}

/// Infer WIT type from JSON value
fn infer_type_from_json(json: &JsonValue) -> AnalysedType {
    match json {
        JsonValue::Null => AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        }),
        JsonValue::Bool(_) => AnalysedType::Bool(TypeBool),
        JsonValue::Number(n) => {
            if n.is_i64() {
                AnalysedType::S64(TypeS64)
            } else if n.is_u64() {
                AnalysedType::U64(TypeU64)
            } else {
                AnalysedType::F64(TypeF64)
            }
        },
        JsonValue::String(_) => AnalysedType::Str(TypeStr),
        JsonValue::Array(arr) => {
            if arr.is_empty() {
                AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)), // default to string for empty arrays
                })
            } else {
                // Infer type from first element and use it for the whole array
                AnalysedType::List(TypeList {
                    inner: Box::new(infer_type_from_json(&arr[0])),
                })
            }
        },
        JsonValue::Object(map) => {
            let fields = map
                .iter()
                .map(|(k, v)| NameTypePair {
                    name: k.clone(),
                    typ: infer_type_from_json(v),
                })
                .collect();
            
            AnalysedType::Record(TypeRecord { fields })
        },
    }
}

/// Create WIT Types API routes with CORS configuration
pub fn wit_types_routes() -> impl Endpoint {
    let api_service = OpenApiService::new(WitTypesApi, "WIT Types API", env!("CARGO_PKG_VERSION"))
        .server("http://localhost:3000")
        .description("WebAssembly Interface Types (WIT) API provides endpoints for converting and validating WIT data types, handling complex nested structures, and performing type transformations between WIT and OpenAPI formats.")
        .url_prefix("/api/wit-types");

    Route::new()
        .nest("/api/wit-types", api_service.clone().with(create_cors_middleware()))
        .nest("/api/wit-types/doc", api_service.spec_endpoint().with(create_cors_middleware()))
        .nest("/swagger-ui/wit-types", api_service.swagger_ui().with(create_cors_middleware()))
        .with(poem::middleware::AddData::new(()))
        .with(create_cors_middleware())
        .into_endpoint()
}

#[derive(Debug, Object)]
struct ConversionResponse {
    /// The converted value in WIT format
    wit: JsonValue,
    /// The converted value in OpenAPI format
    openapi: JsonValue,
}