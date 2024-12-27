use golem_wasm_ast::analysis::AnalysedType;
use utoipa::openapi::{
    schema::{Schema, Object, Array, Type, SchemaType},
    RefOr, OneOf,
};
use std::collections::BTreeMap;
use serde_json::Value;
use rib::RibInputTypeInfo;
use std::fmt;

#[derive(Clone, PartialEq)]
pub enum CustomSchemaType {
    Boolean,
    Integer,
    Number,
    String,
    Array,
    Object,
}

impl fmt::Debug for CustomSchemaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CustomSchemaType::Boolean => write!(f, "Boolean"),
            CustomSchemaType::Integer => write!(f, "Integer"),
            CustomSchemaType::Number => write!(f, "Number"),
            CustomSchemaType::String => write!(f, "String"),
            CustomSchemaType::Array => write!(f, "Array"),
            CustomSchemaType::Object => write!(f, "Object"),
        }
    }
}

impl From<Type> for CustomSchemaType {
    fn from(schema_type: Type) -> Self {
        match schema_type {
            Type::Boolean => CustomSchemaType::Boolean,
            Type::Integer => CustomSchemaType::Integer,
            Type::Number => CustomSchemaType::Number,
            Type::String => CustomSchemaType::String,
            Type::Array => CustomSchemaType::Array,
            Type::Object => CustomSchemaType::Object,
            _ => CustomSchemaType::Object, // Default to Object for other types
        }
    }
}

impl From<CustomSchemaType> for SchemaType {
    fn from(custom_type: CustomSchemaType) -> Self {
        match custom_type {
            CustomSchemaType::Boolean => SchemaType::new(Type::Boolean),
            CustomSchemaType::Integer => SchemaType::new(Type::Integer),
            CustomSchemaType::Number => SchemaType::new(Type::Number),
            CustomSchemaType::String => SchemaType::new(Type::String),
            CustomSchemaType::Array => SchemaType::new(Type::Array),
            CustomSchemaType::Object => SchemaType::new(Type::Object),
        }
    }
}

pub struct RibConverter;

impl RibConverter {
    pub fn convert_input_type(&self, input_type: &RibInputTypeInfo) -> Option<Schema> {
        let mut properties = BTreeMap::new();

        for (name, typ) in &input_type.types {
            if let Some(schema) = self.convert_type(typ) {
                properties.insert(name.clone(), RefOr::T(schema));
            }
        }

        if properties.is_empty() {
            None
        } else {
            let mut obj = Object::with_type(Type::Object);
            obj.properties = properties;
            Some(Schema::Object(obj))
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    pub fn convert_type(&self, typ: &AnalysedType) -> Option<Schema> {
        match typ {
            AnalysedType::Bool(_) => {
                let mut obj = Object::with_type(Type::Boolean);
                obj.description = Some("Boolean value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::U8(_) | AnalysedType::U16(_) | AnalysedType::U32(_) | AnalysedType::U64(_) |
            AnalysedType::S8(_) | AnalysedType::S16(_) | AnalysedType::S32(_) | AnalysedType::S64(_) => {
                let mut obj = Object::with_type(Type::Integer);
                obj.description = Some("Integer value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::F32(_) | AnalysedType::F64(_) => {
                let mut obj = Object::with_type(Type::Number);
                obj.description = Some("Floating point value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::Str(_) | AnalysedType::Chr(_) => {
                let mut obj = Object::with_type(Type::String);
                obj.description = Some("String value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::List(list_type) => {
                if let Some(items_schema) = self.convert_type(&list_type.inner) {
                    let array = Array::new(RefOr::T(items_schema));
                    Some(Schema::Array(array))
                } else {
                    None
                }
            }
            AnalysedType::Record(record_type) => {
                let mut properties = BTreeMap::new();
                let mut required = Vec::new();

                for field in &record_type.fields {
                    if let Some(field_schema) = self.convert_type(&field.typ) {
                        properties.insert(field.name.clone(), RefOr::T(field_schema));
                        required.push(field.name.clone());
                    }
                }

                if !properties.is_empty() {
                    let mut obj = Object::with_type(Type::Object);
                    obj.properties = properties;
                    obj.required = required;
                    obj.description = Some("Record type".to_string());
                    Some(Schema::Object(obj))
                } else {
                    None
                }
            }
            AnalysedType::Enum(enum_type) => {
                let mut obj = Object::with_type(Type::String);
                obj.enum_values = Some(enum_type.cases.iter()
                    .map(|case| Value::String(case.clone()))
                    .collect());
                obj.description = Some("Enumerated type".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::Variant(variant_type) => {
                if variant_type.cases.is_empty() {
                    return None;
                }

                // Create a oneOf schema for the value field
                let mut one_of = OneOf::new();
                for case in &variant_type.cases {
                    if let Some(typ) = &case.typ {
                        if let Some(case_schema) = self.convert_type(typ) {
                            one_of.items.push(RefOr::T(case_schema));
                        }
                    } else {
                        one_of.items.push(RefOr::T(Schema::Object(Object::with_type(Type::Null))));
                    }
                }

                // Create the main object schema with discriminator and value fields
                let mut properties = BTreeMap::new();
                
                // Add discriminator field (string enum of variant names)
                let mut discriminator_obj = Object::with_type(Type::String);
                discriminator_obj.enum_values = Some(variant_type.cases.iter()
                    .map(|case| Value::String(case.name.clone()))
                    .collect());
                properties.insert("discriminator".to_string(), RefOr::T(Schema::Object(discriminator_obj)));
                
                // Add value field with oneOf schema
                properties.insert("value".to_string(), RefOr::T(Schema::OneOf(one_of)));

                let mut obj = Object::with_type(Type::Object);
                obj.properties = properties;
                obj.required = vec!["discriminator".to_string(), "value".to_string()];
                obj.description = Some("Variant type".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::Option(option_type) => {
                if let Some(inner_schema) = self.convert_type(&option_type.inner) {
                    let mut obj = Object::with_type(Type::Object);
                    obj.description = Some("Optional value".to_string());
                    obj.properties = BTreeMap::new();
                    obj.properties.insert("value".to_string(), RefOr::T(inner_schema));
                    obj.required = vec![];
                    Some(Schema::Object(obj))
                } else {
                    None
                }
            }
            AnalysedType::Result(result_type) => {
                let mut properties = BTreeMap::new();

                if let Some(ok_type) = &result_type.ok {
                    if let Some(ok_schema) = self.convert_type(ok_type) {
                        properties.insert("ok".to_string(), RefOr::T(ok_schema));
                    }
                }

                if let Some(err_type) = &result_type.err {
                    if let Some(err_schema) = self.convert_type(err_type) {
                        properties.insert("err".to_string(), RefOr::T(err_schema));
                    }
                }

                if !properties.is_empty() {
                    let mut obj = Object::with_type(Type::Object);
                    obj.properties = properties;
                    obj.description = Some("Result type".to_string());
                    Some(Schema::Object(obj))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_wasm_ast::analysis::{
        TypeStr,
        TypeVariant,
        NameOptionTypePair,
    };
    use test_r::test;

    fn verify_schema_type(actual: &SchemaType, expected_type: Type) {
        let expected = SchemaType::new(expected_type);
        assert!(actual == &expected, "Schema type mismatch");
    }

    #[test]
    fn test_convert_type() {
        let converter = RibConverter;

        // Test string type
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                verify_schema_type(&obj.schema_type, Type::String);
            }
            _ => panic!("Expected object schema"),
        }

        // Test variant type
        let variant = AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "case1".to_string(),
                    typ: Some(AnalysedType::Str(TypeStr)),
                },
            ],
        });
        let schema = converter.convert_type(&variant).unwrap();
        match &schema {
            Schema::Object(obj) => {
                verify_schema_type(&obj.schema_type, Type::Object);
                assert!(obj.properties.contains_key("discriminator"));
                assert!(obj.properties.contains_key("value"));

                // Verify discriminator field
                if let Some(RefOr::T(Schema::Object(discriminator_obj))) = obj.properties.get("discriminator") {
                    verify_schema_type(&discriminator_obj.schema_type, Type::String);
                    assert!(discriminator_obj.enum_values.is_some());
                    let enum_values = discriminator_obj.enum_values.as_ref().unwrap();
                    assert_eq!(enum_values.len(), 1);
                    assert_eq!(enum_values[0], Value::String("case1".to_string()));
                } else {
                    panic!("Expected discriminator to be a string schema with enum values");
                }

                // Verify value field
                if let Some(RefOr::T(Schema::OneOf(one_of))) = obj.properties.get("value") {
                    assert_eq!(one_of.items.len(), 1);
                    if let RefOr::T(Schema::Object(value_obj)) = &one_of.items[0] {
                        verify_schema_type(&value_obj.schema_type, Type::String);
                    } else {
                        panic!("Expected string schema in oneOf items");
                    }
                } else {
                    panic!("Expected value to be a oneOf schema");
                }
            }
            _ => panic!("Expected object schema"),
        }
    }
}