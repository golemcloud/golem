use golem_wasm_ast::analysis::*;
use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
use serde_json::json;
use utoipa::openapi::Schema;
use valico::json_schema;

mod fixtures;
use fixtures::test_component::TestComponent;
use fixtures::comprehensive_wit_types::{
    // Search types
    SearchQuery, SearchFilters, SearchFlags, DateRange, Pagination,
    // Batch types
    BatchOptions,
    // Transformation types
    DataTransformation,
    // Tree types
    TreeNode, NodeMetadata, TreeOperation,
};

fn validate_json_against_schema(json: &serde_json::Value, schema: &Schema) -> bool {
    thread_local! {
        static SCOPE: std::cell::RefCell<json_schema::Scope> = std::cell::RefCell::new(json_schema::Scope::new());
    }
    
    let schema_json = serde_json::to_value(schema).unwrap();
    SCOPE.with(|scope| {
        let mut scope_ref = scope.borrow_mut();
        let schema = scope_ref.compile_and_return(schema_json.clone(), false).unwrap();
        schema.validate(&json).is_valid()
    })
}

#[test]
fn test_primitive_types_conversion() {
    let converter = RibConverter;
    let test_component = TestComponent;
    
    // Get test data from component
    let _primitives = test_component.test_primitives();
    
    // Convert to AnalysedType
    let record_type = AnalysedType::Record(TypeRecord {
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
    });
    
    // Get schema
    let schema = converter.convert_type(&record_type).unwrap();
    
    let test_json = json!({
        "bool_val": true,
        "u8_val": 255,
        "u16_val": 65535,
        "u32_val": 4294967295u64,
        "u64_val": 18446744073709551615u64,
        "s8_val": -128,
        "s16_val": -32768,
        "s32_val": -2147483648,
        "s64_val": -9223372036854775808i64,
        "f32_val": 3.14159,
        "f64_val": 2.718281828459045,
        "char_val": "🦀",
        "string_val": "Hello, WIT!"
    });

    println!("Schema: {}", serde_json::to_string_pretty(&schema).unwrap());
    println!("Test JSON: {}", serde_json::to_string_pretty(&test_json).unwrap());

    assert!(validate_json_against_schema(&test_json, &schema));
}

#[test]
fn test_complex_record_conversion() {
    let converter = RibConverter;
    let test_component = TestComponent;
    
    // Get test data from component
    let _profile = test_component.test_user_profile();

    // Create user profile type
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

    let profile_type = AnalysedType::Record(TypeRecord {
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
    });

    // Get schema
    let schema = converter.convert_type(&profile_type).unwrap();

    // Create test JSON
    let test_json = json!({
        "id": 42,
        "username": "test_user",
        "settings": {
            "value": {
                "theme": "dark",
                "notifications_enabled": true,
                "email_frequency": "daily"
            }
        },
        "permissions": {
            "can_read": true,
            "can_write": true,
            "can_delete": false,
            "is_admin": false
        }
    });

    assert!(validate_json_against_schema(&test_json, &schema));
}

#[test]
fn test_variant_type_conversion() {
    let converter = RibConverter;
    let test_component = TestComponent;
    
    // Get test data from component
    let _content_types = test_component.test_content_types();
    
    // Create content type variant
    let complex_data_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "data".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    };

    let variant_type = AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "Text".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
            NameOptionTypePair {
                name: "Number".to_string(),
                typ: Some(AnalysedType::F64(TypeF64)),
            },
            NameOptionTypePair {
                name: "Boolean".to_string(),
                typ: Some(AnalysedType::Bool(TypeBool)),
            },
            NameOptionTypePair {
                name: "Complex".to_string(),
                typ: Some(AnalysedType::Record(complex_data_type)),
            },
        ],
    });
    
    // Get schema
    let schema = converter.convert_type(&variant_type).unwrap();
    
    let test_cases = vec![
        json!({
            "discriminator": "Text",
            "value": "Plain text"
        }),
        json!({
            "discriminator": "Number",
            "value": 42.0
        }),
        json!({
            "discriminator": "Boolean",
            "value": true
        }),
        json!({
            "discriminator": "Complex",
            "value": {
                "id": 1,
                "data": ["data1", "data2"]
            }
        }),
    ];

    for test_json in test_cases {
        println!("Schema: {}", serde_json::to_string_pretty(&schema).unwrap());
        println!("Test JSON: {}", serde_json::to_string_pretty(&test_json).unwrap());
        assert!(validate_json_against_schema(&test_json, &schema));
    }
}

#[test]
fn test_result_type_conversion() {
    let converter = RibConverter;
    let test_component = TestComponent;
    
    // Get test data from component
    let _success_result = test_component.test_operation_result(true);
    let _error_result = test_component.test_operation_result(false);

    // Create result type
    let success_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "code".to_string(),
                typ: AnalysedType::U16(TypeU16),
            },
            NameTypePair {
                name: "message".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "data".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::Str(TypeStr)),
                }),
            },
        ],
    };

    let error_type = TypeRecord {
        fields: vec![
            NameTypePair {
                name: "code".to_string(),
                typ: AnalysedType::U16(TypeU16),
            },
            NameTypePair {
                name: "message".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
            NameTypePair {
                name: "details".to_string(),
                typ: AnalysedType::Option(TypeOption {
                    inner: Box::new(AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Str(TypeStr)),
                    })),
                }),
            },
        ],
    };

    let result_type = AnalysedType::Result(TypeResult {
        ok: Some(Box::new(AnalysedType::Record(success_type))),
        err: Some(Box::new(AnalysedType::Record(error_type))),
    });

    // Get schema
    let schema = converter.convert_type(&result_type).unwrap();

    // Test success case
    let success_json = json!({
        "ok": {
            "code": 200,
            "message": "Operation successful",
            "data": {
                "value": "Additional data"
            }
        }
    });

    // Test error case
    let error_json = json!({
        "err": {
            "code": 400,
            "message": "Operation failed",
            "details": {
                "value": ["Invalid input", "Please try again"]
            }
        }
    });

    assert!(validate_json_against_schema(&success_json, &schema));
    assert!(validate_json_against_schema(&error_json, &schema));
}

#[test]
fn test_search_functionality() {
    let _converter = RibConverter;
    let test_component = TestComponent;

    // Test search query
    let query = SearchQuery {
        query: "test".to_string(),
        filters: SearchFilters {
            categories: vec!["docs".to_string()],
            date_range: Some(DateRange {
                start: 1234567890,
                end: 1234567899,
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
    };

    // Test search result
    let result = test_component.perform_search(query.clone());
    assert_eq!(result.total_count, 2);
    assert_eq!(result.matches.len(), 2);

    // Test query validation
    assert!(test_component.validate_search_query(query).is_ok());
}

#[test]
fn test_batch_operations() {
    let _converter = RibConverter;
    let test_component = TestComponent;

    let items = vec!["item1".to_string(), "item2".to_string(), "item3".to_string()];
    let options = BatchOptions {
        parallel: true,
        retry_count: 3,
        timeout_ms: 5000,
    };

    // Test batch processing
    let result = test_component.batch_process(items.clone(), options.clone());
    assert_eq!(result.successful + result.failed, items.len() as u32);

    // Test batch validation
    let validation_results = test_component.batch_validate(items.clone());
    assert_eq!(validation_results.len(), items.len());

    // Test async batch processing
    let batch_id = test_component.process_batch_async(items, options).unwrap();
    let status = test_component.get_batch_status(batch_id).unwrap();
    assert!(status.successful > 0);
}

#[test]
fn test_transformations() {
    let _converter = RibConverter;
    let test_component = TestComponent;

    let data = vec!["data1".to_string(), "data2".to_string()];
    let transform = DataTransformation::Sort {
        field: "name".to_string(),
        ascending: true,
    };

    // Test single transformation
    let result = test_component.apply_transformation(data.clone(), transform);
    assert!(result.success);
    assert_eq!(result.output.len(), data.len());

    // Test chained transformations
    let transforms = vec![
        DataTransformation::Sort {
            field: "name".to_string(),
            ascending: true,
        },
        DataTransformation::Filter {
            predicate: "length > 3".to_string(),
        },
    ];
    let chain_result = test_component.chain_transformations(data, transforms).unwrap();
    assert!(chain_result.success);
}

#[test]
fn test_tree_operations() {
    let _converter = RibConverter;
    let test_component = TestComponent;

    let root = TreeNode {
        id: 1,
        value: "root".to_string(),
        children: vec![],
        metadata: NodeMetadata {
            created_at: 1234567890,
            modified_at: 1234567890,
            tags: vec!["root".to_string()],
        },
    };

    // Test tree creation
    let created = test_component.create_tree(root.clone()).unwrap();
    assert_eq!(created.id, root.id);

    // Test tree modification
    let operation = TreeOperation::Insert {
        parent_id: 1,
        node: TreeNode {
            id: 2,
            value: "child".to_string(),
            children: vec![],
            metadata: NodeMetadata {
                created_at: 1234567890,
                modified_at: 1234567890,
                tags: vec!["child".to_string()],
            },
        },
    };
    let stats = test_component.modify_tree(operation).unwrap();
    assert_eq!(stats.nodes_affected, 1);

    // Test tree query
    let node = test_component.query_tree(1, Some(2)).unwrap();
    assert_eq!(node.id, 1);
}

#[test]
fn test_complex_validation() {
    let _converter = RibConverter;
    let test_component = TestComponent;

    let profile = test_component.test_user_profile();
    let query = SearchQuery {
        query: "test".to_string(),
        filters: SearchFilters {
            categories: vec![],
            date_range: None,
            flags: SearchFlags {
                case_sensitive: false,
                whole_word: false,
                regex_enabled: false,
            },
        },
        pagination: None,
    };
    let options = BatchOptions {
        parallel: true,
        retry_count: 3,
        timeout_ms: 5000,
    };

    let result = test_component.validate_complex_input(profile, query, options);
    assert!(result.is_ok());
} 