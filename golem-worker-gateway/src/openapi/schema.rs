// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use golem_api_grpc::proto::golem::rib::{RibInputType, RibOutputType};
use openapiv3::{Schema, SchemaKind, Type, ArrayType, ObjectType};
use std::collections::HashMap;

/// Converts WIT/Component Model types to OpenAPI schemas
pub struct WitTypeConverter {
    type_cache: HashMap<String, Schema>,
}

impl WitTypeConverter {
    pub fn new() -> Self {
        Self {
            type_cache: HashMap::new(),
        }
    }

    pub fn convert_input_type(&self, input_type: &RibInputType) -> Result<Schema> {
        // Convert RibInputType to OpenAPI Schema
        let mut properties = HashMap::new();
        
        for (name, wit_type) in &input_type.types {
            let property_schema = self.convert_wit_type(wit_type)?;
            properties.insert(name.clone(), ReferenceOr::Item(property_schema));
        }

        Ok(Schema {
            schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                properties,
                required: input_type.types.keys().cloned().collect(),
                ..Default::default()
            })),
            schema_data: Default::default(),
        })
    }

    pub fn convert_output_type(&self, output_type: &RibOutputType) -> Result<Schema> {
        if let Some(wit_type) = &output_type.type_ {
            self.convert_wit_type(wit_type)
        } else {
            Err(OpenApiError::InvalidType("Missing output type".to_string()))
        }
    }

    fn convert_wit_type(&self, wit_type: &golem_api_grpc::proto::wasm::ast::Type) -> Result<Schema> {
        use golem_api_grpc::proto::wasm::ast::type_::Kind;

        match &wit_type.kind {
            Some(Kind::I32(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::Integer(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::I64(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::Integer(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::F32(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::Number(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::F64(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::Number(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::String(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::String(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::Bool(_)) => Ok(Schema {
                schema_kind: SchemaKind::Type(Type::Boolean(Default::default())),
                schema_data: Default::default(),
            }),
            Some(Kind::List(list_type)) => {
                if let Some(element_type) = &list_type.element_type {
                    let items = self.convert_wit_type(element_type)?;
                    Ok(Schema {
                        schema_kind: SchemaKind::Type(Type::Array(ArrayType {
                            items: Box::new(ReferenceOr::Item(items)),
                            min_items: None,
                            max_items: None,
                            unique_items: false,
                        })),
                        schema_data: Default::default(),
                    })
                } else {
                    Err(OpenApiError::InvalidType("Missing list element type".to_string()))
                }
            },
            Some(Kind::Option(option_type)) => {
                if let Some(inner_type) = &option_type.inner {
                    let schema = self.convert_wit_type(inner_type)?;
                    Ok(schema) // In OpenAPI, optional fields are represented by omitting them from the required list
                } else {
                    Err(OpenApiError::InvalidType("Missing option inner type".to_string()))
                }
            },
            Some(Kind::Result(result_type)) => {
                // Convert Result type to a union type in OpenAPI
                let mut schemas = vec![];
                
                if let Some(ok_type) = &result_type.ok {
                    schemas.push(self.convert_wit_type(ok_type)?);
                }
                
                if let Some(err_type) = &result_type.err {
                    schemas.push(self.convert_wit_type(err_type)?);
                }
                
                Ok(Schema {
                    schema_kind: SchemaKind::OneOf(schemas),
                    schema_data: Default::default(),
                })
            },
            Some(Kind::Record(record_type)) => {
                let mut properties = HashMap::new();
                let mut required = Vec::new();
                
                for field in &record_type.fields {
                    if let Some(field_type) = &field.type_ {
                        let property_schema = self.convert_wit_type(field_type)?;
                        properties.insert(
                            field.name.clone(),
                            ReferenceOr::Item(property_schema),
                        );
                        required.push(field.name.clone());
                    }
                }
                
                Ok(Schema {
                    schema_kind: SchemaKind::Type(Type::Object(ObjectType {
                        properties,
                        required,
                        ..Default::default()
                    })),
                    schema_data: Default::default(),
                })
            },
            _ => Err(OpenApiError::InvalidType("Unsupported WIT type".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_api_grpc::proto::wasm::ast::{Type, type_::Kind};

    #[test]
    fn test_convert_basic_types() {
        let converter = WitTypeConverter::new();
        
        // Test integer conversion
        let i32_type = Type {
            kind: Some(Kind::I32(Default::default())),
        };
        let schema = converter.convert_wit_type(&i32_type).unwrap();
        assert!(matches!(schema.schema_kind, SchemaKind::Type(Type::Integer(_))));
        
        // Test string conversion
        let string_type = Type {
            kind: Some(Kind::String(Default::default())),
        };
        let schema = converter.convert_wit_type(&string_type).unwrap();
        assert!(matches!(schema.schema_kind, SchemaKind::Type(Type::String(_))));
    }
}
