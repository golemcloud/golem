use test_r::test_gen;
use anyhow::Result;
use test_r::core::DynamicTestRegistration;

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
    use utoipa::openapi::Schema;
    use serde_json::Value;

    fn validate_json_against_schema(json: Value, schema: &Schema) -> bool {
        let schema_json = serde_json::to_value(schema).unwrap();
        let mut scope = json_schema::Scope::new();
        let schema = scope.compile_and_return(schema_json, false).unwrap();
        schema.validate(&json).is_valid()
    }

    fn create_rib_value(value: &str, typ: &AnalysedType) -> TypeAnnotatedValue {
        let json_value: Value = serde_json::from_str(value).unwrap();
        let parsed_value = TypeAnnotatedValue::parse_with_type(&json_value, typ)
            .unwrap();
        parsed_value
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_primitive_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
        let converter = RibConverter;

        // Test boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).ok_or_else(|| anyhow::anyhow!("Failed to convert bool type to schema"))?;
        let rib_value = create_rib_value("true", &bool_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test integer
        let int_type = AnalysedType::U32(TypeU32);
        let schema = converter.convert_type(&int_type).ok_or_else(|| anyhow::anyhow!("Failed to convert integer type to schema"))?;
        let rib_value = create_rib_value("42", &int_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test string
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).ok_or_else(|| anyhow::anyhow!("Failed to convert string type to schema"))?;
        let rib_value = create_rib_value("\"hello\"", &str_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        Ok(())
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_record_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
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
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_variant_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
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

        let schema = converter.convert_type(&variant_type).ok_or_else(|| anyhow::anyhow!("Failed to convert variant type to schema"))?;
        
        // Test Case1
        let json_str = r#"{"discriminator": "Case1", "value": {"Case1": 42}}"#;
        let rib_value = create_rib_value(json_str, &variant_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        // Test Case2
        let json_str = r#"{"discriminator": "Case2", "value": {"Case2": "hello"}}"#;
        let rib_value = create_rib_value(json_str, &variant_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        Ok(())
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_list_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
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
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_complex_nested_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
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
            inner: Box::new(variant_type),
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

        let schema = converter.convert_type(&record_type).ok_or_else(|| anyhow::anyhow!("Failed to convert complex record type to schema"))?;
        
        let json_str = r#"{
            "items": [
                {"discriminator": "Number", "value": {"Number": 42}},
                {"discriminator": "Text", "value": {"Text": "hello"}}
            ],
            "name": "test"
        }"#;

        let rib_value = create_rib_value(json_str, &record_type);
        let json = rib_value.to_json_value();
        assert!(validate_json_against_schema(json, &schema));

        Ok(())
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_invalid_json_schema_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
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
    }

    #[allow(unused_must_use)]
    #[must_use]
    #[test_gen(unwrap)]
    async fn test_negative_primitive_validation(_test: &mut DynamicTestRegistration) -> Result<()> {
        let converter = RibConverter;

        // Test wrong type for boolean
        let bool_type = AnalysedType::Bool(TypeBool);
        let schema = converter.convert_type(&bool_type).ok_or_else(|| anyhow::anyhow!("Failed to convert bool type to schema"))?;
        let invalid_json = serde_json::json!(42); // number instead of boolean
        assert!(!validate_json_against_schema(invalid_json, &schema));

        Ok(())
    }
} 