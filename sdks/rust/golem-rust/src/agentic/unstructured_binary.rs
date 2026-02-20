// Copyright 2024-2025 Golem Cloud
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

use crate::agentic::{Schema, StructuredSchema, StructuredValue};
use crate::golem_agentic::golem::agent::common::{
    BinaryDescriptor, BinaryReference, BinarySource, BinaryType, ElementSchema, ElementValue,
};
use golem_wasm::agentic::unstructured_binary::{AllowedMimeTypes, UnstructuredBinary};
use golem_wasm::Value;

impl<T: AllowedMimeTypes> Schema for UnstructuredBinary<T> {
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        let restrictions = if T::all().is_empty() {
            None
        } else {
            let restrictions = T::all()
                .iter()
                .map(|code| BinaryType {
                    mime_type: code.to_string(),
                })
                .collect::<Vec<_>>();

            Some(restrictions)
        };

        StructuredSchema::Default(ElementSchema::UnstructuredBinary(BinaryDescriptor {
            restrictions,
        }))
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        match self {
            UnstructuredBinary::Inline { data, mime_type } => {
                let mime_type = mime_type.to_string();

                Ok(StructuredValue::Default(ElementValue::UnstructuredBinary(
                    BinaryReference::Inline(BinarySource {
                        data,
                        binary_type: BinaryType { mime_type },
                    }),
                )))
            }

            UnstructuredBinary::Url(url) => Ok(StructuredValue::Default(
                ElementValue::UnstructuredBinary(BinaryReference::Url(url)),
            )),
        }
    }

    fn from_structured_value(
        value: StructuredValue,
        _schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        let element_value = match value {
            StructuredValue::Default(element_value) => Ok(element_value),
            StructuredValue::Multimodal(_) => {
                Err("type mismatch. expected default value, found multimodal")
            }
            StructuredValue::AutoInjected(_) => {
                Err("type mismatch. expected default value, found auto-injected")
            }
        }?;

        match element_value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredBinary ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredBinary(binary_reference) => match binary_reference {
                BinaryReference::Url(url) => Ok(UnstructuredBinary::Url(url)),
                BinaryReference::Inline(binary_source) => {
                    let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

                    let mime_type = binary_source.binary_type.mime_type;

                    if !allowed.is_empty() && !allowed.contains(&mime_type) {
                        return Err(format!(
                            "Mime type '{}' is not allowed. Allowed mime types: {:?}",
                            mime_type,
                            allowed.join(",")
                        ));
                    }

                    let mime_type = T::from_string(&mime_type).ok_or_else(|| {
                        format!(
                            "Failed to convert mime type '{}' to AllowedMimeType",
                            mime_type
                        )
                    })?;

                    Ok(UnstructuredBinary::Inline {
                        data: binary_source.data,
                        mime_type,
                    })
                }
            },
            ElementValue::UnstructuredText(_) => {
                Err("Expected UnstructuredBinary ElementValue, got UnstructuredBinary".to_string())
            }
        }
    }

    fn from_wit_value(
        wit_value: golem_wasm::WitValue,
        _schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        UnstructuredBinary::from_wit_value(wit_value)
    }

    fn to_wit_value(self) -> Result<golem_wasm::WitValue, String>
    where
        Self: Sized,
    {
        let value_type = self.to_structured_value()?;

        let element_value_result = match value_type {
            StructuredValue::Default(element_value) => Ok(element_value),
            StructuredValue::Multimodal(_) => {
                Err("Expected element value but found multimodal".to_string())
            }
            StructuredValue::AutoInjected(_) => {
                Err("Expected element value but found auto-injected".to_string())
            }
        };

        let element_value = element_value_result?;

        match element_value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredBinary ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredBinary(binary_reference) => match binary_reference {
                BinaryReference::Url(url) => {
                    let value = Value::Variant {
                        case_idx: 0,
                        case_value: Some(Box::new(Value::String(url))),
                    };

                    Ok(golem_wasm::WitValue::from(value))
                }
                BinaryReference::Inline(binary_source) => {
                    let restriction_record =
                        Value::Record(vec![Value::String(binary_source.binary_type.mime_type)]);

                    let value = Value::Variant {
                        case_idx: 1,
                        case_value: Some(Box::new(Value::Record(vec![
                            Value::List(binary_source.data.into_iter().map(Value::U8).collect()),
                            restriction_record,
                        ]))),
                    };

                    Ok(golem_wasm::WitValue::from(value))
                }
            },
            ElementValue::UnstructuredText(_) => {
                Err("Expected UnstructuredBinary ElementValue, got UnstructuredText".to_string())
            }
        }
    }
}
