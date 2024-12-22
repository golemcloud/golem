#[cfg(test)]
mod tests {
    use crate::gateway_api_definition::http::rib_converter::{RibConverter, CustomSchemaType};
    use golem_wasm_ast::analysis::{
        AnalysedType, NameTypePair, TypeBool, TypeEnum, TypeF32, TypeF64, TypeList, 
        TypeOption, TypeRecord, TypeResult, TypeStr, TypeU8, TypeU32, TypeU64, TypeVariant,
    };
    use utoipa::openapi::schema::{Schema, SchemaType};
    use std::collections::HashMap;
    use rib::RibInputTypeInfo;
    use utoipa::openapi::RefOr;

    fn create_converter() -> RibConverter {
        RibConverter
    }

    fn create_input_type(typ: AnalysedType) -> RibInputTypeInfo {
        let mut types = HashMap::new();
        types.insert("test".to_string(), typ);
        RibInputTypeInfo { types }
    }

    fn assert_schema_type(schema: &Schema, expected_type: CustomSchemaType) {
        match schema {
            Schema::Object(obj) => {
                assert_eq!(CustomSchemaType::from(obj.schema_type.clone()), expected_type);
            }
            Schema::Array(_) => {
                assert_eq!(expected_type, CustomSchemaType::Array);
            }
            _ => panic!("Unexpected schema type"),
        }
    }

    fn assert_schema_description(schema: Schema, expected_description: &str) {
        match schema {
            Schema::Object(obj) => {
                assert_eq!(obj.description.as_deref(), Some(expected_description));
            }
            _ => panic!("Expected Schema::Object"),
        }
    }

    #[test]
    fn test_complex_nested_type() {
        let converter = create_converter();
        
        let nested_type = create_input_type(AnalysedType::Record(TypeRecord {
            fields: vec![
                NameTypePair {
                    name: "items".to_string(),
                    typ: AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::Record(TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "id".to_string(),
                                    typ: AnalysedType::U64(TypeU64),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Option(TypeOption {
                                        inner: Box::new(AnalysedType::Str(TypeStr)),
                                    }),
                                },
                            ],
                        })),
                    }),
                },
            ],
        }));

        let schema = converter.convert_input_type(&nested_type).unwrap();
        
        match schema.properties.get("test").unwrap() {
            RefOr::T(Schema::Object(obj)) => {
                assert_eq!(CustomSchemaType::from(obj.schema_type.clone()), CustomSchemaType::Object);
                assert_eq!(obj.description.as_deref(), Some("Record type"));
                
                // Verify the nested structure
                let items_prop = obj.properties.get("items").unwrap();
                match items_prop {
                    RefOr::T(Schema::Array(array)) => {
                        if let Some(items) = &array.items {
                            match items.as_ref() {
                                RefOr::T(Schema::Object(item_obj)) => {
                                    assert_eq!(CustomSchemaType::from(item_obj.schema_type.clone()), CustomSchemaType::Object);
                                    
                                    // Check id field
                                    if let Some(RefOr::T(id_schema)) = item_obj.properties.get("id") {
                                        assert_schema_type(id_schema, CustomSchemaType::Integer);
                                    } else {
                                        panic!("Missing or invalid id field");
                                    }
                                    
                                    // Check name field
                                    if let Some(RefOr::T(name_schema)) = item_obj.properties.get("name") {
                                        assert_schema_type(name_schema, CustomSchemaType::Object);
                                        if let Schema::Object(name_obj) = name_schema {
                                            assert!(name_obj.properties.contains_key("value"));
                                            assert!(name_obj.required.is_empty());
                                        } else {
                                            panic!("Invalid name field schema");
                                        }
                                    } else {
                                        panic!("Missing or invalid name field");
                                    }
                                }
                                _ => panic!("Expected Schema::Object for array item"),
                            }
                        } else {
                            panic!("Missing array items");
                        }
                    }
                    _ => panic!("Expected Schema::Array for items property"),
                }
            }
            _ => panic!("Expected Schema::Object"),
        }
    }

    #[test]
    fn test_primitive_types() {
        let converter = create_converter();

        // Test boolean
        let bool_type = create_input_type(AnalysedType::Bool(TypeBool));
        let schema = converter.convert_input_type(&bool_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::Boolean);
            assert_schema_description(schema.clone(), "Boolean value");
        } else {
            panic!("Expected boolean schema");
        }

        // Test integer
        let int_type = create_input_type(AnalysedType::U32(TypeU32));
        let schema = converter.convert_input_type(&int_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::Integer);
            assert_schema_description(schema.clone(), "Integer value");
        } else {
            panic!("Expected integer schema");
        }

        // Test float
        let float_type = create_input_type(AnalysedType::F64(TypeF64));
        let schema = converter.convert_input_type(&float_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::Number);
            assert_schema_description(schema.clone(), "Floating point value");
        } else {
            panic!("Expected float schema");
        }

        // Test string
        let str_type = create_input_type(AnalysedType::Str(TypeStr));
        let schema = converter.convert_input_type(&str_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::String);
            assert_schema_description(schema.clone(), "String value");
        } else {
            panic!("Expected string schema");
        }
    }

    #[test]
    fn test_container_types() {
        let converter = create_converter();

        // Test list
        let list_type = create_input_type(AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        }));
        let schema = converter.convert_input_type(&list_type).unwrap();
        if let Some(RefOr::T(Schema::Array(_))) = schema.properties.get("test") {
            // Array type verified
        } else {
            panic!("Expected array schema");
        }

        // Test enum
        let enum_type = create_input_type(AnalysedType::Enum(TypeEnum {
            cases: vec!["case1".to_string(), "case2".to_string()],
        }));
        let schema = converter.convert_input_type(&enum_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::String);
            assert_schema_description(schema.clone(), "Enumerated type");
            if let Schema::Object(obj) = schema {
                assert!(obj.enum_values.is_some());
            }
        } else {
            panic!("Expected enum schema");
        }

        // Test option
        let option_type = create_input_type(AnalysedType::Option(TypeOption {
            inner: Box::new(AnalysedType::Str(TypeStr)),
        }));
        let schema = converter.convert_input_type(&option_type).unwrap();
        if let Some(RefOr::T(schema)) = schema.properties.get("test") {
            assert_schema_type(schema, CustomSchemaType::Object);
            assert_schema_description(schema.clone(), "Optional value");
            if let Schema::Object(obj) = schema {
                assert!(obj.properties.contains_key("value"));
                assert!(obj.required.is_empty());
            }
        } else {
            panic!("Expected option schema");
        }
    }
} 