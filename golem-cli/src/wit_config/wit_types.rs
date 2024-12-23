use serde::{Deserialize, Serialize};
use tailcall_valid::{Valid, Validator};

use crate::openapi::openapi_spec::{OpenApiSpec, Resolved, Schema};

#[derive(Debug, Clone, Default, PartialEq, Eq, strum_macros::Display, Serialize, Deserialize)]
pub enum WitType {
    // Primitive Types
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    Float32,
    Float64,
    Char,
    #[default] // TODO: maybe drop the default
    String,

    // Compound Types
    Option(Box<WitType>),       // Option<T>
    Result(Box<WitType>, Box<WitType>), // Result<T, E>
    List(Box<WitType>),         // List<T>
    Tuple(Vec<WitType>),        // (T1, T2, ...)

    // Custom Types
    Record(Vec<(String, WitType)>), // Record { field_name: Type }
    Variant(Vec<(String, Option<WitType>)>), // Variant { name: Option<Type> }
    Enum(Vec<String>),            // Enum { name1, name2, ... }
    Flags(Vec<String>),           // Flags { flag1, flag2, ... }

    // Special Types
    Handle(String),               // Handle<Resource>
    TypeAlias(String, Box<WitType>), // TypeAlias { alias_name, type }
    FieldTy(String) // Custom type to resolve field types
}

impl WitType {
    pub fn from_primitive_proto_type(proto_ty: &str) -> Valid<Self, anyhow::Error, anyhow::Error> {
        let binding = proto_ty.to_lowercase();
        let ty = binding.strip_prefix("type_").unwrap_or(proto_ty);
        match ty {
            "double" | "float" => Valid::succeed(WitType::Float64),
            "int32" | "sint32" | "fixed32" | "sfixed32" => Valid::succeed(WitType::S32),
            "int64" | "sint64" | "fixed64" | "sfixed64" => Valid::succeed(WitType::S64),
            "uint32" => Valid::succeed(WitType::U32),
            "uint64" => Valid::succeed(WitType::U64),
            "bool" => Valid::succeed(WitType::Bool),
            "string" => Valid::succeed(WitType::String),
            // Ideally, this should never be reached
            _ => Valid::fail(anyhow::anyhow!("Unknown/Complex type: {}", ty)),
        }
    }
    pub fn from_schema(
        schema: &Schema,
        openapi: &OpenApiSpec<Resolved>,
    ) -> Valid<WitType, anyhow::Error, anyhow::Error> {
        if let Some(reference) = &schema.ref_ {
            return Valid::from_option(
                openapi.resolve_ref(reference),
                anyhow::anyhow!("Failed to resolve reference: {}", reference),
            )
                .and_then(|resolved_schema| WitType::from_schema(resolved_schema, openapi));
        }

        Valid::from_option(schema.type_.as_ref(), anyhow::anyhow!("SchemaType is required")).and_then(|ty| {
            match ty.as_str() {
                "bool" | "boolean" => Valid::succeed(WitType::Bool),
                "integer" => Valid::succeed(WitType::S32),
                "number" => Valid::succeed(WitType::Float64),
                "string" => Valid::succeed(WitType::String),
                "array" => {
                    Valid::from_option(schema.items.as_ref(), anyhow::anyhow!("Items are required"))
                        .and_then(|items| WitType::from_schema(items, openapi))
                        .map(|items_ty| WitType::List(Box::new(items_ty)))
                }
                "object" => {
                    Valid::from_option(schema.properties.as_ref(), anyhow::anyhow!("Properties are required"))
                        .and_then(|properties| {
                            Valid::from_iter(properties.iter(), |(name, schema)| {
                                Valid::from(WitType::from_schema(schema, openapi)).map(|ty| (name.clone(), ty))
                            })
                        }
                        )
                        .map(|fields| WitType::Record(fields))
                }
                _ => Valid::fail(anyhow::anyhow!("Unknown type: {}", ty)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use crate::openapi::openapi_spec::Components;
    use super::*;

    #[test]
    fn test_ref_resolution() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "ReferencedSchema".to_string(),
            Schema {
                type_: Some("string".to_string()),
                ..Default::default()
            },
        );

        let openapi = OpenApiSpec {
            components: Some(Components {
                schemas: Some(schemas),
                ..Default::default()
            }),
            ..Default::default()
        };

        let schema = Schema {
            ref_: Some("#/components/schemas/ReferencedSchema".to_string()),
            ..Default::default()
        };

        let result = WitType::from_schema(&schema, &openapi).to_result().unwrap();
        assert_eq!(result, WitType::String);
    }

    #[test]
    fn test_array_with_ref() {
        let mut schemas = HashMap::new();
        schemas.insert(
            "ReferencedSchema".to_string(),
            Schema {
                type_: Some("string".to_string()),
                ..Default::default()
            },
        );

        let openapi = OpenApiSpec {
            components: Some(Components {
                schemas: Some(schemas),
                ..Default::default()
            }),
            ..Default::default()
        };

        let schema = Schema {
            type_: Some("array".to_string()),
            items: Some(Box::new(Schema {
                ref_: Some("#/components/schemas/ReferencedSchema".to_string()),
                ..Default::default()
            })),
            ..Default::default()
        };

        let result = WitType::from_schema(&schema, &openapi).to_result().unwrap();
        assert_eq!(result, WitType::List(Box::new(WitType::String)));
    }

    #[test]
    fn test_object_with_properties() {
        let properties = vec![
            (
                "id".to_string(),
                Schema {
                    type_: Some("integer".to_string()),
                    ..Default::default()
                },
            ),
            (
                "name".to_string(),
                Schema {
                    type_: Some("string".to_string()),
                    ..Default::default()
                },
            ),
        ];

        let schema = Schema {
            type_: Some("object".to_string()),
            properties: Some(properties.into_iter().collect()),
            ..Default::default()
        };

        let openapi = OpenApiSpec::default();

        let result = WitType::from_schema(&schema, &openapi).to_result().unwrap();
        assert_eq!(
            result,
            WitType::Record(vec![
                ("id".to_string(), WitType::S32),
                ("name".to_string(), WitType::String),
            ])
        );
    }
}
