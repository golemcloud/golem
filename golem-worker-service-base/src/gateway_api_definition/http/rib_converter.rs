use golem_wasm_ast::analysis::AnalysedType;
use utoipa::openapi::{
    schema::{Schema, Object, ObjectBuilder, Array, OneOf},
    SchemaType,
    RefOr,
};
use std::collections::BTreeMap;
use serde_json::Value;
use rib::RibInputTypeInfo;

#[derive(Debug, Clone, PartialEq)]
pub enum CustomSchemaType {
    Boolean,
    Integer,
    Number,
    String,
    Array,
    Object,
}

impl From<SchemaType> for CustomSchemaType {
    fn from(schema_type: SchemaType) -> Self {
        match schema_type {
            SchemaType::Boolean => CustomSchemaType::Boolean,
            SchemaType::Integer => CustomSchemaType::Integer,
            SchemaType::Number => CustomSchemaType::Number,
            SchemaType::String => CustomSchemaType::String,
            SchemaType::Array => CustomSchemaType::Array,
            SchemaType::Object => CustomSchemaType::Object,
            _ => CustomSchemaType::Object, // Default to Object for other types
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
            let mut obj = Object::with_type(SchemaType::Object);
            obj.properties = properties;
            Some(Schema::Object(obj))
        }
    }

    #[allow(clippy::only_used_in_recursion)]
    fn convert_type(&self, typ: &AnalysedType) -> Option<Schema> {
        match typ {
            AnalysedType::Bool(_) => {
                let mut obj = Object::with_type(SchemaType::Boolean);
                obj.description = Some("Boolean value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::U8(_) | AnalysedType::U32(_) | AnalysedType::U64(_) => {
                let mut obj = Object::with_type(SchemaType::Integer);
                obj.description = Some("Integer value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::F32(_) | AnalysedType::F64(_) => {
                let mut obj = Object::with_type(SchemaType::Number);
                obj.description = Some("Floating point value".to_string());
                Some(Schema::Object(obj))
            }
            AnalysedType::Str(_) => {
                let mut obj = Object::with_type(SchemaType::String);
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
                    let mut obj = Object::with_type(SchemaType::Object);
                    obj.properties = properties;
                    obj.required = required;
                    obj.description = Some("Record type".to_string());
                    Some(Schema::Object(obj))
                } else {
                    None
                }
            }
            AnalysedType::Enum(enum_type) => {
                let mut obj = Object::with_type(SchemaType::String);
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

                let mut schemas = Vec::new();
                for case in &variant_type.cases {
                    if let Some(typ) = &case.typ {
                        if let Some(case_schema) = self.convert_type(typ) {
                            let case_obj = ObjectBuilder::new()
                                .schema_type(SchemaType::Object)
                                .property(case.name.clone(), RefOr::T(case_schema))
                                .build();
                            schemas.push(Schema::Object(case_obj));
                        }
                    }
                }

                if !schemas.is_empty() {
                    let mut obj = Object::with_type(SchemaType::Object);
                    obj.description = Some("Variant type".to_string());
                    obj.properties = BTreeMap::new();
                    
                    let discriminator_obj = ObjectBuilder::new()
                        .schema_type(SchemaType::String)
                        .build();
                    obj.properties.insert("discriminator".to_string(), RefOr::T(Schema::Object(discriminator_obj)));
                    
                    let mut one_of = OneOf::new();
                    for schema in schemas {
                        one_of.items.push(RefOr::T(schema));
                    }
                    obj.properties.insert("value".to_string(), RefOr::T(Schema::OneOf(one_of)));
                    Some(Schema::Object(obj))
                } else {
                    None
                }
            }
            AnalysedType::Option(option_type) => {
                if let Some(inner_schema) = self.convert_type(&option_type.inner) {
                    let mut obj = Object::with_type(SchemaType::Object);
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
                    let mut obj = Object::with_type(SchemaType::Object);
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

    #[test]
    fn test_convert_type() {
        let converter = RibConverter;
        
        // Test string type
        let str_type = AnalysedType::Str(TypeStr);
        let schema = converter.convert_type(&str_type).unwrap();
        match &schema {
            Schema::Object(obj) => {
                assert!(matches!(obj.schema_type, SchemaType::String));
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
                assert!(obj.properties.contains_key("discriminator"));
                assert!(obj.properties.contains_key("value"));
            }
            _ => panic!("Expected object schema"),
        }
    }
}
