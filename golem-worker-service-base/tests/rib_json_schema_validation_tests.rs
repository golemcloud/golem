use anyhow::Result;
test_r::enable!();

#[cfg(test)]
mod rib_json_schema_validation_tests {
    use super::*;
    use golem_worker_service_base::gateway_api_definition::http::rib_converter::RibConverter;
    use valico::json_schema;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_ast::analysis::{
        AnalysedType,
        TypeBool,
        TypeStr,
        TypeU32,
        TypeVariant,
        TypeRecord,
        TypeList,
        NameOptionTypePair,
        NameTypePair,
    };
    use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
    use utoipa::openapi::{
        schema::{Schema, Object, Array, Type},
        RefOr, OneOf,
    };
    use serde_json::Value;
    use std::collections::BTreeMap;

    fn validate_json_against_schema(json: Value, schema: &Schema) -> bool {
        let schema_json = serde_json::to_value(schema).unwrap();
        println!("Input JSON: {}", serde_json::to_string_pretty(&json).unwrap());
        println!("Schema JSON: {}", serde_json::to_string_pretty(&schema_json).unwrap());
        let mut scope = json_schema::Scope::new();
        let schema = scope.compile_and_return(schema_json, false).unwrap();
        let validation = schema.validate(&json);
        if !validation.is_valid() {
            println!("Validation errors: {:?}", validation.errors);
        }
        validation.is_valid()
    }

    fn create_rib_value(value: &str, typ: &AnalysedType) -> TypeAnnotatedValue {
        let json_value: Value = serde_json::from_str(value).unwrap();
        println!("Input JSON before parsing: {}", serde_json::to_string_pretty(&json_value).unwrap());
        let parsed_value = TypeAnnotatedValue::parse_with_type(&json_value, typ)
            .unwrap();
        println!("Output JSON after parsing: {}", serde_json::to_string_pretty(&parsed_value.to_json_value()).unwrap());
        parsed_value
    }

    #[test]
    fn test_record_json_schema_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            let record_type = AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "field1".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                    NameTypePair {
                        name: "field2".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            });

            let schema = converter.convert_type(&record_type).ok_or_else(|| anyhow::anyhow!("Failed to convert record type to schema"))?;
            let json_str = r#"{"field1": 42, "field2": "hello"}"#;
            let rib_value = create_rib_value(json_str, &record_type);
            let json = rib_value.to_json_value();
            assert!(validate_json_against_schema(json, &schema));

            Ok(())
        })
    }

    #[test]
    fn test_variant_json_schema_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            let variant_type = AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "Case1".to_string(),
                        typ: Some(AnalysedType::U32(TypeU32)),
                    },
                    NameOptionTypePair {
                        name: "Case2".to_string(),
                        typ: Some(AnalysedType::Str(TypeStr)),
                    },
                ],
            });

            // Create a schema that matches TypeAnnotatedValue's format
            let mut one_of = OneOf::new();
            
            // Add a schema for each variant case
            if let AnalysedType::Variant(variant) = &variant_type {
                for case in &variant.cases {
                    let mut case_obj = Object::with_type(Type::Object);
                    let mut case_props = BTreeMap::new();
                    if let Some(typ) = &case.typ {
                        if let Some(case_schema) = converter.convert_type(typ) {
                            case_props.insert(case.name.clone(), RefOr::T(case_schema));
                            case_obj.properties = case_props;
                            case_obj.required = vec![case.name.clone()];
                            one_of.items.push(RefOr::T(Schema::Object(case_obj)));
                        }
                    }
                }
            }
            
            let schema = Schema::OneOf(one_of);
            
            // Test Case1
            let json_str = r#"{"Case1": 42}"#;
            let rib_value = create_rib_value(json_str, &variant_type);
            let json = rib_value.to_json_value();
            println!("Actual JSON: {}", serde_json::to_string_pretty(&json).unwrap());
            println!("Schema: {}", serde_json::to_string_pretty(&serde_json::to_value(&schema).unwrap()).unwrap());
            assert!(validate_json_against_schema(json, &schema));

            // Test Case2
            let json_str = r#"{"Case2": "hello"}"#;
            let rib_value = create_rib_value(json_str, &variant_type);
            let json = rib_value.to_json_value();
            assert!(validate_json_against_schema(json, &schema));

            // Test invalid case
            let json_str = r#"{"InvalidCase": 42}"#;
            let json: Value = serde_json::from_str(json_str).unwrap();
            assert!(!validate_json_against_schema(json, &schema));

            Ok(())
        })
    }

    #[test]
    fn test_list_json_schema_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            let list_type = AnalysedType::List(TypeList {
                inner: Box::new(AnalysedType::U32(TypeU32)),
            });

            let schema = converter.convert_type(&list_type).ok_or_else(|| anyhow::anyhow!("Failed to convert list type to schema"))?;
            let json_str = "[1, 2, 3, 4, 5]";
            let rib_value = create_rib_value(json_str, &list_type);
            let json = rib_value.to_json_value();
            assert!(validate_json_against_schema(json, &schema));

            Ok(())
        })
    }

    #[test]
    fn test_complex_nested_json_schema_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            // Create a record containing a list of variants
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

            let list_type = AnalysedType::List(TypeList {
                inner: Box::new(variant_type.clone()),
            });

            let record_type = AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "items".to_string(),
                        typ: list_type,
                    },
                    NameTypePair {
                        name: "name".to_string(),
                        typ: AnalysedType::Str(TypeStr),
                    },
                ],
            });

            // Create variant schema that matches TypeAnnotatedValue's format
            let mut one_of = OneOf::new();
            
            // Add a schema for each variant case
            if let AnalysedType::Variant(variant) = &variant_type {
                for case in &variant.cases {
                    let mut case_obj = Object::with_type(Type::Object);
                    let mut case_props = BTreeMap::new();
                    if let Some(typ) = &case.typ {
                        if let Some(case_schema) = converter.convert_type(typ) {
                            case_props.insert(case.name.clone(), RefOr::T(case_schema));
                            case_obj.properties = case_props;
                            case_obj.required = vec![case.name.clone()];
                            one_of.items.push(RefOr::T(Schema::Object(case_obj)));
                        }
                    }
                }
            }
            
            let variant_schema = Schema::OneOf(one_of);
            
            // Create list schema
            let array = Array::new(RefOr::T(variant_schema));
            let list_schema = Schema::Array(array);
            
            // Create record schema
            let mut record_obj = Object::with_type(Type::Object);
            let mut record_props = BTreeMap::new();
            record_props.insert("items".to_string(), RefOr::T(list_schema));
            record_props.insert("name".to_string(), RefOr::T(Schema::Object(Object::with_type(Type::String))));
            record_obj.properties = record_props;
            record_obj.required = vec!["items".to_string(), "name".to_string()];
            let schema = Schema::Object(record_obj);
            
            let json_str = r#"{
                "items": [
                    {"Number": 42},
                    {"Text": "hello"}
                ],
                "name": "test"
            }"#;

            let rib_value = create_rib_value(json_str, &record_type);
            let json = rib_value.to_json_value();
            assert!(validate_json_against_schema(json, &schema));

            // Test invalid variant in list
            let json_str = r#"{
                "items": [
                    {"InvalidType": 42},
                    {"Text": "hello"}
                ],
                "name": "test"
            }"#;
            let json: Value = serde_json::from_str(json_str).unwrap();
            assert!(!validate_json_against_schema(json, &schema));

            Ok(())
        })
    }

    #[test]
    fn test_invalid_json_schema_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            // Test with wrong type
            let int_type = AnalysedType::U32(TypeU32);
            let schema = converter.convert_type(&int_type).ok_or_else(|| anyhow::anyhow!("Failed to convert integer type to schema"))?;
            let json = serde_json::json!("not a number");
            assert!(!validate_json_against_schema(json, &schema));

            // Test with missing required field
            let record_type = AnalysedType::Record(TypeRecord {
                fields: vec![
                    NameTypePair {
                        name: "required_field".to_string(),
                        typ: AnalysedType::U32(TypeU32),
                    },
                ],
            });
            let schema = converter.convert_type(&record_type).ok_or_else(|| anyhow::anyhow!("Failed to convert record type to schema"))?;
            let json = serde_json::json!({});
            assert!(!validate_json_against_schema(json, &schema));

            // Test with wrong variant case
            let variant_type = AnalysedType::Variant(TypeVariant {
                cases: vec![
                    NameOptionTypePair {
                        name: "Case1".to_string(),
                        typ: Some(AnalysedType::U32(TypeU32)),
                    },
                ],
            });
            let schema = converter.convert_type(&variant_type).ok_or_else(|| anyhow::anyhow!("Failed to convert variant type to schema"))?;
            let json = serde_json::json!({
                "discriminator": "NonexistentCase",
                "value": {"NonexistentCase": 42}
            });
            assert!(!validate_json_against_schema(json, &schema));

            Ok(())
        })
    }

    #[test]
    fn test_negative_primitive_validation() -> Result<()> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let converter = RibConverter;

            // Test wrong type for boolean
            let bool_type = AnalysedType::Bool(TypeBool);
            let schema = converter.convert_type(&bool_type).ok_or_else(|| anyhow::anyhow!("Failed to convert bool type to schema"))?;
            let invalid_json = serde_json::json!(42); // number instead of boolean
            assert!(!validate_json_against_schema(invalid_json, &schema));

            Ok(())
        })
    }
} 