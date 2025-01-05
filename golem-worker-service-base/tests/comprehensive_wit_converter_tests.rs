use golem_wasm_ast::analysis::*;
use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
use serde_json::json;

// Helper function to verify schema type and format
fn assert_schema_type(schema: &MetaSchemaRef, expected_type: &str, expected_format: Option<&str>) {
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, expected_type);
            if let Some(expected) = expected_format {
                assert_eq!(schema.format.as_deref(), Some(expected));
            }
        },
        MetaSchemaRef::Reference(_) => panic!("Expected inline schema, got reference"),
    }
}

// Helper function to find a property in a schema
fn find_property<'a>(properties: &'a [(& 'static str, MetaSchemaRef)], name: &str) -> Option<&'a MetaSchemaRef> {
    properties.iter()
        .find(|(key, _)| *key == name)
        .map(|(_, schema)| schema)
}

// Helper function to get schema from Box<MetaSchemaRef>
fn get_schema_from_box(boxed: &Box<MetaSchemaRef>) -> &MetaSchema {
    match &**boxed {
        MetaSchemaRef::Inline(schema) => schema,
        MetaSchemaRef::Reference(_) => panic!("Expected inline schema, got reference"),
    }
}

#[test]
fn test_primitive_types_openapi_schema() {
    let mut converter = RibConverter::new_openapi();
    let mut registry = Registry::new();

    // Test boolean
    let bool_type = AnalysedType::Bool(TypeBool);
    let schema = converter.convert_type(&bool_type, &mut registry).unwrap();
    assert_schema_type(&schema, "boolean", None);

    // Test integer types
    // 8-bit integers
    let u8_type = AnalysedType::U8(TypeU8);
    let schema = converter.convert_type(&u8_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int32"));
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.minimum, Some(0.0));
            assert_eq!(schema.maximum, Some(255.0));
        },
        _ => panic!("Expected inline schema"),
    }

    let s8_type = AnalysedType::S8(TypeS8);
    let schema = converter.convert_type(&s8_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int32"));
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.minimum, Some(-128.0));
            assert_eq!(schema.maximum, Some(127.0));
        },
        _ => panic!("Expected inline schema"),
    }

    // 16-bit integers
    let u16_type = AnalysedType::U16(TypeU16);
    let schema = converter.convert_type(&u16_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int32"));
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.minimum, Some(0.0));
            assert_eq!(schema.maximum, Some(65535.0));
        },
        _ => panic!("Expected inline schema"),
    }

    let s16_type = AnalysedType::S16(TypeS16);
    let schema = converter.convert_type(&s16_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int32"));
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.minimum, Some(-32768.0));
            assert_eq!(schema.maximum, Some(32767.0));
        },
        _ => panic!("Expected inline schema"),
    }

    // 32-bit and 64-bit integers
    let u32_type = AnalysedType::U32(TypeU32);
    let schema = converter.convert_type(&u32_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int32"));

    let s64_type = AnalysedType::S64(TypeS64);
    let schema = converter.convert_type(&s64_type, &mut registry).unwrap();
    assert_schema_type(&schema, "integer", Some("int64"));

    // Test float types
    let f32_type = AnalysedType::F32(TypeF32);
    let schema = converter.convert_type(&f32_type, &mut registry).unwrap();
    assert_schema_type(&schema, "number", Some("float"));

    let f64_type = AnalysedType::F64(TypeF64);
    let schema = converter.convert_type(&f64_type, &mut registry).unwrap();
    assert_schema_type(&schema, "number", Some("double"));

    // Test string types
    let str_type = AnalysedType::Str(TypeStr);
    let schema = converter.convert_type(&str_type, &mut registry).unwrap();
    assert_schema_type(&schema, "string", None);

    let char_type = AnalysedType::Chr(TypeChr);
    let schema = converter.convert_type(&char_type, &mut registry).unwrap();
    assert_schema_type(&schema, "string", None);

    // Test Option type
    let option_type = AnalysedType::Option(TypeOption {
        inner: Box::new(AnalysedType::U32(TypeU32)),
    });
    let schema = converter.convert_type(&option_type, &mut registry).unwrap();
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "integer");
            assert!(schema.nullable);
            assert_eq!(schema.format.as_deref(), Some("int32"));
        },
        _ => panic!("Expected inline schema"),
    }

    // Test Enum type
    let enum_type = AnalysedType::Enum(TypeEnum {
        cases: vec!["Red".to_string(), "Green".to_string(), "Blue".to_string()],
    });
    let schema = converter.convert_type(&enum_type, &mut registry).unwrap();
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "string");
            assert_eq!(schema.enum_items.len(), 3);
            assert!(schema.enum_items.contains(&json!("Red")));
            assert!(schema.enum_items.contains(&json!("Green")));
            assert!(schema.enum_items.contains(&json!("Blue")));
        },
        _ => panic!("Expected inline schema"),
    }

    // Test Tuple type
    let tuple_type = AnalysedType::Tuple(TypeTuple {
        items: vec![
            AnalysedType::U32(TypeU32),
            AnalysedType::Str(TypeStr),
        ],
    });
    let schema = converter.convert_type(&tuple_type, &mut registry).unwrap();
    match &schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "array");
            assert!(schema.min_items.is_some());
            assert_eq!(schema.min_items, Some(2));
            assert!(schema.max_items.is_some());
            assert_eq!(schema.max_items, Some(2));
            
            // Check tuple items
            let items = schema.items.as_ref().unwrap();
            match &**items {
                MetaSchemaRef::Inline(items_schema) => {
                    assert_eq!(items_schema.one_of.len(), 2);
                    
                    // First item should be integer
                    match &items_schema.one_of[0] {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "integer");
                            assert_eq!(schema.format.as_deref(), Some("int32"));
                        },
                        _ => panic!("Expected inline schema"),
                    }
                    
                    // Second item should be string
                    match &items_schema.one_of[1] {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "string");
                        },
                        _ => panic!("Expected inline schema"),
                    }
                },
                _ => panic!("Expected inline schema"),
            }
        },
        _ => panic!("Expected inline schema"),
    }
}

#[test]
fn test_complex_types_openapi_schema() {
    let mut converter = RibConverter::new_openapi();
    let mut registry = Registry::new();

    // Test list type
    let list_type = AnalysedType::List(TypeList {
        inner: Box::new(AnalysedType::Str(TypeStr)),
    });
    let schema = converter.convert_type(&list_type, &mut registry).unwrap();
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "array");
            assert!(schema.items.is_some());
            let item_schema = get_schema_from_box(schema.items.as_ref().unwrap());
            assert_eq!(item_schema.ty, "string");
        },
        _ => panic!("Expected inline schema"),
    }

    // Test record type
    let record_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "id".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "name".to_string(),
                typ: AnalysedType::Str(TypeStr),
            },
        ],
    });
    let schema = converter.convert_type(&record_type, &mut registry).unwrap();
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "object");
            assert_eq!(schema.required.len(), 2);
            assert!(schema.required.contains(&"id"));
            assert!(schema.required.contains(&"name"));

            let id_schema = find_property(&schema.properties, "id").unwrap();
            assert_schema_type(id_schema, "integer", Some("int32"));

            let name_schema = find_property(&schema.properties, "name").unwrap();
            assert_schema_type(name_schema, "string", None);
        },
        _ => panic!("Expected inline schema"),
    }

    // Test variant type
    let variant_type = AnalysedType::Variant(TypeVariant {
        cases: vec![
            NameOptionTypePair {
                name: "Number".to_string(),
                typ: Some(AnalysedType::U32(TypeU32)),
            },
            NameOptionTypePair {
                name: "Text".to_string(),
                typ: Some(AnalysedType::Str(TypeStr)),
            },
        ],
    });
    let schema = converter.convert_type(&variant_type, &mut registry).unwrap();
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "object");
            assert!(schema.required.contains(&"type"));
            
            // Check discriminator
            let type_schema = find_property(&schema.properties, "type").unwrap();
            match type_schema {
                MetaSchemaRef::Inline(type_schema) => {
                    assert_eq!(type_schema.ty, "string");
                    assert_eq!(type_schema.enum_items.len(), 2);
                    assert!(type_schema.enum_items.contains(&json!("Number")));
                    assert!(type_schema.enum_items.contains(&json!("Text")));
                },
                _ => panic!("Expected inline schema for type field"),
            }

            // Check value field
            let value_schema = find_property(&schema.properties, "value").unwrap();
            match value_schema {
                MetaSchemaRef::Inline(value_schema) => {
                    assert_eq!(value_schema.ty, "object");
                    assert!(!value_schema.one_of.is_empty());
                    
                    // Verify the oneOf variants
                    assert_eq!(value_schema.one_of.len(), 2);
                    
                    // Check Number variant
                    let number_schema = &value_schema.one_of[0];
                    match number_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "integer");
                            assert_eq!(schema.format.as_deref(), Some("int32"));
                        },
                        _ => panic!("Expected inline schema for Number variant"),
                    }
                    
                    // Check Text variant
                    let text_schema = &value_schema.one_of[1];
                    match text_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "string");
                        },
                        _ => panic!("Expected inline schema for Text variant"),
                    }
                },
                _ => panic!("Expected inline schema for value field"),
            }
        },
        _ => panic!("Expected inline schema"),
    }
}

#[test]
fn test_result_type_openapi_schema() {
    let mut converter = RibConverter::new_openapi();
    let mut registry = Registry::new();

    let result_type = AnalysedType::Result(TypeResult {
        ok: Some(Box::new(AnalysedType::U32(TypeU32))),
        err: Some(Box::new(AnalysedType::Str(TypeStr))),
    });

    let schema = converter.convert_type(&result_type, &mut registry).unwrap();
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "object");
            assert!(schema.required.contains(&"type"));
            
            // Check discriminator
            let type_schema = find_property(&schema.properties, "type").unwrap();
            match type_schema {
                MetaSchemaRef::Inline(type_schema) => {
                    assert_eq!(type_schema.ty, "string");
                    assert_eq!(type_schema.enum_items.len(), 2);
                    assert!(type_schema.enum_items.contains(&json!("ok")));
                    assert!(type_schema.enum_items.contains(&json!("error")));
                },
                _ => panic!("Expected inline schema for type field"),
            }

            // Check value field
            let value_schema = find_property(&schema.properties, "value").unwrap();
            match value_schema {
                MetaSchemaRef::Inline(value_schema) => {
                    assert_eq!(value_schema.ty, "object");
                    assert!(!value_schema.one_of.is_empty());
                    
                    // Verify the oneOf variants
                    assert_eq!(value_schema.one_of.len(), 2);
                    
                    // Check ok variant
                    let ok_schema = &value_schema.one_of[0];
                    match ok_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "integer");
                            assert_eq!(schema.format.as_deref(), Some("int32"));
                        },
                        _ => panic!("Expected inline schema for ok variant"),
                    }
                    
                    // Check error variant
                    let error_schema = &value_schema.one_of[1];
                    match error_schema {
                        MetaSchemaRef::Inline(schema) => {
                            assert_eq!(schema.ty, "string");
                        },
                        _ => panic!("Expected inline schema for error variant"),
                    }
                },
                _ => panic!("Expected inline schema for value field"),
            }
        },
        _ => panic!("Expected inline schema"),
    }
}

#[test]
fn test_openapi_schema_generation() {
    let mut converter = RibConverter::new_openapi();
    let mut registry = Registry::new();

    // Create a complex API response type
    let response_type = AnalysedType::Record(TypeRecord {
        fields: vec![
            NameTypePair {
                name: "items".to_string(),
                typ: AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::Record(TypeRecord {
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
                                name: "email".to_string(),
                                typ: AnalysedType::Str(TypeStr),
                            },
                        ],
                    })),
                }),
            },
            NameTypePair {
                name: "total".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
            NameTypePair {
                name: "page".to_string(),
                typ: AnalysedType::U32(TypeU32),
            },
        ],
    });

    let schema = converter.convert_type(&response_type, &mut registry).unwrap();
    match schema {
        MetaSchemaRef::Inline(schema) => {
            assert_eq!(schema.ty, "object");
            assert_eq!(schema.required.len(), 3);

            // Verify items array
            let items_schema = find_property(&schema.properties, "items").unwrap();
            match items_schema {
                MetaSchemaRef::Inline(items_schema) => {
                    assert_eq!(items_schema.ty, "array");
                    assert!(items_schema.items.is_some());

                    // Verify array item schema
                    let item_schema = get_schema_from_box(items_schema.items.as_ref().unwrap());
                    assert_eq!(item_schema.ty, "object");
                    assert_eq!(item_schema.required.len(), 3);
                    
                    // Verify email field has email format
                    let email_schema = find_property(&item_schema.properties, "email").unwrap();
                    match email_schema {
                        MetaSchemaRef::Inline(email_schema) => {
                            assert_eq!(email_schema.ty, "string");
                            assert_eq!(email_schema.format.as_deref(), Some("email"));
                        },
                        _ => panic!("Expected inline schema for email field"),
                    }
                },
                _ => panic!("Expected inline schema for items field"),
            }

            // Verify pagination fields
            let total_schema = find_property(&schema.properties, "total").unwrap();
            assert_schema_type(total_schema, "integer", Some("int32"));

            let page_schema = find_property(&schema.properties, "page").unwrap();
            assert_schema_type(page_schema, "integer", Some("int32"));
        },
        _ => panic!("Expected inline schema"),
    }
} 