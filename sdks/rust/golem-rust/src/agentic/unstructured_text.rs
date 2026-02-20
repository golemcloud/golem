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
use crate::golem_agentic::golem::agent::common::{ElementSchema, ElementValue, TextDescriptor};
use golem_wasm::agentic::unstructured_text::{AllowedLanguages, UnstructuredText};
use golem_wasm::golem_core_1_5_x::types::{TextReference, TextSource, TextType};

impl<T: AllowedLanguages> Schema for UnstructuredText<T> {
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        let restrictions = if T::all().is_empty() {
            None
        } else {
            let restrictions = T::all()
                .iter()
                .map(|code| TextType {
                    language_code: code.to_string(),
                })
                .collect::<Vec<_>>();

            Some(restrictions)
        };

        StructuredSchema::Default(ElementSchema::UnstructuredText(TextDescriptor {
            restrictions,
        }))
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => {
                let text_type = language_code.map(|code| TextType {
                    language_code: code.to_language_code().to_string(),
                });

                Ok(StructuredValue::Default(ElementValue::UnstructuredText(
                    TextReference::Inline(TextSource {
                        data: text,
                        text_type,
                    }),
                )))
            }

            UnstructuredText::Url(url) => Ok(StructuredValue::Default(
                ElementValue::UnstructuredText(TextReference::Url(url)),
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
            StructuredValue::Default(value) => Ok(value),
            StructuredValue::Multimodal(_) => {
                Err("input mismatch. expected default value, found multimodal".to_string())
            }
            StructuredValue::AutoInjected(_) => {
                Err("input mismatch. expected default value, found auto-injected".to_string())
            }
        }?;

        match element_value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredText ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => Ok(UnstructuredText::Url(url)),
                TextReference::Inline(text_source) => {
                    let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

                    let language_code = match text_source.text_type {
                        Some(text_type) => {
                            if !allowed.is_empty() && !allowed.contains(&text_type.language_code) {
                                return Err(format!(
                                    "Language code '{}' is not allowed. Allowed codes: {}",
                                    text_type.language_code,
                                    allowed.join(", ")
                                ));
                            }

                            T::from_language_code(&text_type.language_code)
                        }
                        None => None,
                    };

                    Ok(UnstructuredText::Text {
                        text: text_source.data,
                        language_code,
                    })
                }
            },
            ElementValue::UnstructuredBinary(_) => {
                Err("Expected UnstructuredText ElementValue, got UnstructuredBinary".to_string())
            }
        }
    }

    fn to_element_value(self) -> Result<ElementValue, String> {
        match self {
            UnstructuredText::Text {
                text,
                language_code,
            } => {
                let text_type = language_code.map(|code| TextType {
                    language_code: code.to_language_code().to_string(),
                });
                Ok(ElementValue::UnstructuredText(TextReference::Inline(
                    TextSource {
                        data: text,
                        text_type,
                    },
                )))
            }
            UnstructuredText::Url(url) => {
                Ok(ElementValue::UnstructuredText(TextReference::Url(url)))
            }
        }
    }

    fn from_element_value(value: ElementValue) -> Result<Self, String> {
        match value {
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => Ok(UnstructuredText::Url(url)),
                TextReference::Inline(text_source) => {
                    let allowed = T::all().iter().map(|s| s.to_string()).collect::<Vec<_>>();

                    let language_code = match text_source.text_type {
                        Some(text_type) => {
                            if !allowed.is_empty() && !allowed.contains(&text_type.language_code) {
                                return Err(format!(
                                    "Language code '{}' is not allowed. Allowed codes: {}",
                                    text_type.language_code,
                                    allowed.join(", ")
                                ));
                            }

                            T::from_language_code(&text_type.language_code)
                        }
                        None => None,
                    };

                    Ok(UnstructuredText::Text {
                        text: text_source.data,
                        language_code,
                    })
                }
            },
            other => Err(format!(
                "Expected UnstructuredText ElementValue, got: {:?}",
                other
            )),
        }
    }
}
