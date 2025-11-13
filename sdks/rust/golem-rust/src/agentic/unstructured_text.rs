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

use crate::agentic::Schema;
use crate::golem_agentic::golem::agent::common::{
    ElementSchema, ElementValue, TextDescriptor, TextReference, TextSource, TextType,
};

pub enum UnstructuredText<LC> {
    Url(String),
    Text {
        text: String,
        language_code: Option<LC>,
    },
}

pub struct AnyLanguage;

impl<T: AllowedLanguages> UnstructuredText<T> {
    pub fn from_url(url: String) -> UnstructuredText<T> {
        UnstructuredText::Url(url)
    }

    pub fn from_inline(text: String, language_code: T) -> UnstructuredText<T> {
        UnstructuredText::Text {
            text,
            language_code: Some(language_code),
        }
    }

    pub fn from_inline_no_language_code(text: String) -> UnstructuredText<AnyLanguage> {
        UnstructuredText::Text {
            text,
            language_code: None,
        }
    }
}

pub trait AllowedLanguages {
    fn all() -> &'static [&'static str];

    // needed to implement Schema (which is not shown here)
    fn from_language_code(code: &str) -> Option<Self>
    where
        Self: Sized;

    fn to_language_code(&self) -> &'static str;
}

impl AllowedLanguages for AnyLanguage {
    fn all() -> &'static [&'static str] {
        &[]
    }

    fn from_language_code(_code: &str) -> Option<Self> {
        None
    }

    fn to_language_code(&self) -> &'static str {
        ""
    }
}

impl<T: AllowedLanguages> Schema for UnstructuredText<T> {
    fn get_type() -> ElementSchema {
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

        ElementSchema::UnstructuredText(TextDescriptor { restrictions })
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

    fn from_element_value(value: ElementValue, _schema: ElementSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            ElementValue::ComponentModel(_) => {
                Err("Expected UnstructuredText ElementValue, got ComponentModel".to_string())
            }
            ElementValue::UnstructuredText(text_reference) => match text_reference {
                TextReference::Url(url) => Ok(UnstructuredText::Url(url)),
                TextReference::Inline(text_source) => {
                    let language_code = match text_source.text_type {
                        Some(text_type) => match T::from_language_code(&text_type.language_code) {
                            Some(code) => Some(code),
                            None => {
                                return Err(format!(
                                    "Language code '{}' is not allowed",
                                    text_type.language_code
                                ));
                            }
                        },
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
}
