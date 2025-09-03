// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod conversions;

#[cfg(feature = "agent-extraction")]
pub mod extraction;
#[cfg(feature = "protobuf")]
mod protobuf;
#[cfg(test)]
mod tests;

pub mod bindings {
    wasmtime::component::bindgen!({
          path: "wit",
          world: "golem-common",
          async: true,
          trappable_imports: true,
          with: {
            "golem:rpc/types": golem_wasm_rpc::golem_rpc_0_2_x::types,
          },
          wasmtime_crate: ::wasmtime
    });
}

use crate::model::component_metadata::ComponentMetadata;
use crate::model::ComponentId;
use async_trait::async_trait;
use base64::Engine;
use bincode::{Decode, Encode};
use golem_wasm_ast::analysis::analysed_type::{case, variant};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{parse_value_and_type, print_value_and_type, IntoValue, Value, ValueAndType};
use golem_wasm_rpc_derive::IntoValue;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::future::Future;
use std::pin::Pin;
// NOTE: The primary reason for duplicating the model with handwritten Rust types is to avoid the need
// to work with WitValue and WitType directly in the application code. Instead, we are converting them
// to Value and AnalysedType which are much more ergonomic to work with.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentConstructor {
    pub name: Option<String>,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentDependency {
    pub type_name: String,
    pub description: Option<String>,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
}

#[derive(Debug, Clone, Encode, Decode, IntoValue)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(#[wit_field(convert = golem_wasm_rpc::WitValue)] ValueAndType),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethod {
    pub name: String,
    pub description: String,
    pub prompt_hint: Option<String>,
    pub input_schema: DataSchema,
    pub output_schema: DataSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentType {
    pub type_name: String,
    pub description: String,
    pub constructor: AgentConstructor,
    pub methods: Vec<AgentMethod>,
    pub dependencies: Vec<AgentDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryDescriptor {
    pub restrictions: Option<Vec<BinaryType>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum BinaryReference {
    Url(Url),
    Inline(BinarySource),
}

impl Display for BinaryReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryReference::Url(url) => write!(f, "{url}"),
            BinaryReference::Inline(binary_source) => write!(f, "{binary_source}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinarySource {
    pub data: Vec<u8>,
    pub binary_type: BinaryType,
}

impl Display for BinarySource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]\"{}\"",
            self.binary_type.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&self.data)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct BinaryType {
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchema {
    pub name: String,
    pub schema: ElementSchema,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementSchemas {
    pub elements: Vec<NamedElementSchema>,
}

impl NamedElementSchemas {
    pub fn empty() -> Self {
        Self {
            elements: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum DataSchema {
    Tuple(NamedElementSchemas),
    Multimodal(NamedElementSchemas),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum DataValue {
    Tuple(ElementValues),
    Multimodal(NamedElementValues),
}

impl DataValue {
    pub fn parse(s: &str, schema: &DataSchema) -> Result<Self, String> {
        match schema {
            DataSchema::Tuple(element_schemas) => {
                let element_strings = split_top_level_commas(s);
                if element_strings.len() != element_schemas.elements.len() {
                    Err(format!(
                        "Unexpected number of parameters: got {}, expected {}",
                        element_strings.len(),
                        element_schemas.elements.len()
                    ))
                } else {
                    let mut element_values = Vec::with_capacity(element_strings.len());
                    for (s, schema) in element_strings.iter().zip(element_schemas.elements.iter()) {
                        element_values.push(ElementValue::parse(s, &schema.schema)?);
                    }
                    Ok(DataValue::Tuple(ElementValues {
                        elements: element_values,
                    }))
                }
            }
            DataSchema::Multimodal(element_schemas) => {
                let element_strings = split_top_level_commas(s);
                let mut element_values = Vec::with_capacity(element_strings.len());
                for s in element_strings {
                    if let Some((element_name, element_value)) = s.split_once('(') {
                        if let Some(element_value) = element_value.strip_suffix(')') {
                            let element_schema = element_schemas
                                .elements
                                .iter()
                                .find(|element_schema| element_schema.name == element_name)
                                .ok_or_else(|| {
                                    format!(
                                        "Unknown multimodal element name: `{}`. Should be one of {}",
                                        element_name,
                                        element_schemas.elements.iter().map(|element_schema| element_schema.name.clone()).collect::<Vec<_>>().join(", ")
                                    )
                                })?;
                            let element_value =
                                ElementValue::parse(element_value, &element_schema.schema)?;
                            element_values.push(NamedElementValue {
                                name: element_name.to_string(),
                                value: element_value,
                            })
                        } else {
                            return Err(format!(
                                "Multimodal value does not end with `)`: {s}; expected to be `name(value)`"
                            ));
                        }
                    } else {
                        return Err(format!(
                            "Invalid multimodal value: {s}; expected to be `name(value)`"
                        ));
                    }
                }
                Ok(DataValue::Multimodal(NamedElementValues {
                    elements: element_values,
                }))
            }
        }
    }
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut result = Vec::new();

    let chars = s.char_indices();
    let mut start = 0;
    let mut nesting = 0;
    let mut in_string = false;
    let mut skip_next = false;
    for (idx, ch) in chars {
        if !skip_next {
            match ch {
                ',' if !in_string => {
                    if nesting == 0 {
                        result.push(&s[start..idx]);
                        start = idx + 1;
                    }
                }
                '\\' if in_string => {
                    skip_next = true;
                }
                '"' => {
                    in_string = !in_string;
                }
                '(' | '[' | '{' if !in_string => {
                    nesting += 1;
                }
                ')' | ']' | '}' if !in_string => {
                    nesting -= 1;
                }
                _ => {}
            }
        } else {
            skip_next = false;
        }
    }
    if start < s.len() {
        result.push(&s[start..]);
    }

    result
}

impl Display for DataValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            DataValue::Tuple(values) => write!(f, "{values}"),
            DataValue::Multimodal(values) => write!(f, "{values}"),
        }
    }
}

impl IntoValue for DataValue {
    fn into_value(self) -> Value {
        match self {
            DataValue::Tuple(elements) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(elements.elements.into_value())),
            },
            DataValue::Multimodal(elements) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(elements.elements.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("tuple", Vec::<ElementValue>::get_type()),
            case("multimodal", Vec::<NamedElementValue>::get_type()),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ElementValues {
    pub elements: Vec<ElementValue>,
}

impl Display for ElementValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.elements
                .iter()
                .map(|element_value| element_value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValues {
    pub elements: Vec<NamedElementValue>,
}

impl Display for NamedElementValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.elements
                .iter()
                .map(|element_value| element_value.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct NamedElementValue {
    pub name: String,
    pub value: ElementValue,
}

impl Display for NamedElementValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ElementValue {
    ComponentModel(#[wit_field(convert = golem_wasm_rpc::WitValue)] ValueAndType),
    UnstructuredText(TextReference),
    UnstructuredBinary(BinaryReference),
}

impl ElementValue {
    pub fn parse(s: &str, schema: &ElementSchema) -> Result<Self, String> {
        match schema {
            ElementSchema::ComponentModel(typ) => {
                let value_and_type = parse_value_and_type(&typ.element_type, s)
                    .map_err(|e| format!("Failed to parse parameter value {s}: {e}"))?;
                Ok(ElementValue::ComponentModel(value_and_type))
            }
            ElementSchema::UnstructuredText(_) => {
                if s.starts_with('"') && s.ends_with('"') {
                    Ok(ElementValue::UnstructuredText(TextReference::Inline(
                        TextSource {
                            data: s[1..s.len() - 1].to_string(),
                            text_type: None,
                        },
                    )))
                } else if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let language_code = &prefix[1..];
                            let data = &rest[1..rest.len() - 1];
                            Ok(ElementValue::UnstructuredText(TextReference::Inline(
                                TextSource {
                                    data: data.to_string(),
                                    text_type: Some(TextType {
                                        language_code: language_code.to_string(),
                                    }),
                                },
                            )))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredText(TextReference::Url(Url {
                        value: url.to_string(),
                    })))
                }
            }
            ElementSchema::UnstructuredBinary(_) => {
                if s.starts_with('[') {
                    if let Some((prefix, rest)) = s.split_once(']') {
                        if rest.starts_with('"') && rest.ends_with('"') {
                            let mime_type = &prefix[1..];
                            let base64_data = &rest[1..rest.len() - 1];
                            let data = base64::engine::general_purpose::STANDARD
                                .decode(base64_data.as_bytes())
                                .map_err(|e| format!("Failed to decode base64 data: {e}"))?;
                            Ok(ElementValue::UnstructuredBinary(BinaryReference::Inline(
                                BinarySource {
                                    data,
                                    binary_type: BinaryType {
                                        mime_type: mime_type.to_string(),
                                    },
                                },
                            )))
                        } else {
                            Err(format!("Invalid unstructured text parameter syntax: {s}"))
                        }
                    } else {
                        Err(format!("Invalid unstructured text parameter syntax: {s}"))
                    }
                } else {
                    let url = ::url::Url::parse(s)
                        .map_err(|e| format!("Failed to parse parameter value {s} as URL: {e}"))?;
                    Ok(ElementValue::UnstructuredBinary(BinaryReference::Url(
                        Url {
                            value: url.to_string(),
                        },
                    )))
                }
            }
        }
    }
}

impl Display for ElementValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementValue::ComponentModel(value) => {
                write!(f, "{}", print_value_and_type(value).unwrap_or_default())
                // NOTE: this is expected to be always working, because we only use values in ElementValues that are printable
            }
            ElementValue::UnstructuredText(text_reference) => write!(f, "{text_reference}"),
            ElementValue::UnstructuredBinary(binary_reference) => write!(f, "{binary_reference}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum ElementSchema {
    ComponentModel(ComponentModelElementSchema),
    UnstructuredText(TextDescriptor),
    UnstructuredBinary(BinaryDescriptor),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ComponentModelElementSchema {
    pub element_type: AnalysedType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextDescriptor {
    pub restrictions: Option<Vec<TextType>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum TextReference {
    Url(Url),
    Inline(TextSource),
}

impl Display for TextReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TextReference::Url(url) => write!(f, "{url}"),
            TextReference::Inline(text_source) => write!(f, "{text_source}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct Url {
    pub value: String,
}

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextSource {
    pub data: String,
    pub text_type: Option<TextType>,
}

impl Display for TextSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.text_type {
            None => write!(f, "\"{}\"", self.data),
            Some(text_type) => write!(f, "[{}]\"{}\"", text_type.language_code, self.data),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct TextType {
    pub language_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Encode, Decode, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentTypes {
    pub types: Vec<AgentType>,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize, IntoValue)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAgentType {
    pub agent_type: AgentType,
    pub implemented_by: ComponentId,
}

/// Identifies a deployed, instantiated agent.
///
/// AgentId is convertible to and from string, and is used as _worker names_.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentId {
    pub agent_type: String,
    pub parameters: DataValue,
}

impl AgentId {
    pub async fn parse(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<Self, String> {
        Self::parse_and_resolve_type(s, resolver)
            .await
            .map(|(agent_id, _)| agent_id)
    }

    pub async fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeResolver,
    ) -> Result<(Self, AgentType), String> {
        let s = s.as_ref();

        if let Some((agent_type, param_list)) = s.split_once('(') {
            if let Some(param_list) = param_list.strip_suffix(')') {
                let agent_type = resolver.resolve_agent_type(agent_type).await?;
                let value = DataValue::parse(param_list, &agent_type.constructor.input_schema)?;
                Ok((
                    AgentId {
                        agent_type: agent_type.type_name.clone(),
                        parameters: value,
                    },
                    agent_type,
                ))
            } else {
                Err("Unexpected agent-id format - missing closing )".to_string())
            }
        } else {
            Err(format!(
                "Invalid agent-id {}. Unexpected agent-id format - must be agent-type(...)",
                s
            ))
        }
    }

    pub fn parse_agent_type(s: impl AsRef<str>) -> Result<String, String> {
        let s = s.as_ref();
        if let Some((agent_type, _)) = s.split_once('(') {
            Ok(agent_type.to_string())
        } else {
            Err("Unexpected agent-id format - must be agent-type(...)".to_string())
        }
    }
}

impl Display for AgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.agent_type, self.parameters)
    }
}

#[async_trait]
pub trait AgentTypeResolver {
    async fn resolve_agent_type(&self, agent_type: &str) -> Result<AgentType, String>;
}

#[async_trait]
impl<F> AgentTypeResolver for F
where
    F: for<'a> Fn(&'a str) -> Pin<Box<dyn Future<Output = Result<AgentType, String>> + 'a + Send>>
        + Send
        + Sync,
{
    async fn resolve_agent_type(&self, agent_type: &str) -> Result<AgentType, String> {
        self(agent_type).await
    }
}

#[async_trait]
impl AgentTypeResolver for &ComponentMetadata {
    async fn resolve_agent_type(&self, agent_type: &str) -> Result<AgentType, String> {
        let result = self.find_agent_type(agent_type).await?;
        result.ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}
