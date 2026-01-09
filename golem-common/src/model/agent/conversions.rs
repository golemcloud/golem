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

use crate::model::agent::bindings::golem::agent::host;
use crate::model::agent::{
    AgentConstructor, AgentDependency, AgentError, AgentMethod, AgentMode, AgentType,
    AgentTypeName, BinaryDescriptor, BinaryReference, BinarySource, BinaryType,
    ComponentModelElementSchema, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues,
    RegisteredAgentType, TextDescriptor, TextReference, TextSource, TextType, UntypedDataValue,
    UntypedElementValue, Url,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use golem_wasm::{Value, ValueAndType};

impl From<super::bindings::golem::agent::common::AgentMode> for AgentMode {
    fn from(value: super::bindings::golem::agent::common::AgentMode) -> Self {
        match value {
            super::bindings::golem::agent::common::AgentMode::Durable => Self::Durable,
            super::bindings::golem::agent::common::AgentMode::Ephemeral => Self::Ephemeral,
        }
    }
}

impl From<AgentMode> for super::bindings::golem::agent::common::AgentMode {
    fn from(value: AgentMode) -> Self {
        match value {
            AgentMode::Durable => super::bindings::golem::agent::common::AgentMode::Durable,
            AgentMode::Ephemeral => super::bindings::golem::agent::common::AgentMode::Ephemeral,
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentConstructor> for AgentConstructor {
    fn from(value: crate::model::agent::bindings::golem::agent::common::AgentConstructor) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: DataSchema::from(value.input_schema),
        }
    }
}

impl From<AgentConstructor> for super::bindings::golem::agent::common::AgentConstructor {
    fn from(value: AgentConstructor) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value.input_schema.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentDependency> for AgentDependency {
    fn from(value: crate::model::agent::bindings::golem::agent::common::AgentDependency) -> Self {
        Self {
            type_name: value.type_name,
            description: value.description,
            constructor: AgentConstructor::from(value.constructor),
            methods: value.methods.into_iter().map(AgentMethod::from).collect(),
        }
    }
}

impl From<AgentDependency> for super::bindings::golem::agent::common::AgentDependency {
    fn from(value: AgentDependency) -> Self {
        Self {
            type_name: value.type_name,
            description: value.description,
            constructor: value.constructor.into(),
            methods: value.methods.into_iter().map(AgentMethod::into).collect(),
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentError> for AgentError {
    fn from(value: crate::model::agent::bindings::golem::agent::common::AgentError) -> Self {
        match value {
            crate::model::agent::bindings::golem::agent::common::AgentError::InvalidInput(msg) => {
                AgentError::InvalidInput(msg)
            }
            crate::model::agent::bindings::golem::agent::common::AgentError::InvalidMethod(msg) => {
                AgentError::InvalidMethod(msg)
            }
            crate::model::agent::bindings::golem::agent::common::AgentError::InvalidType(msg) => {
                AgentError::InvalidType(msg)
            }
            crate::model::agent::bindings::golem::agent::common::AgentError::InvalidAgentId(
                msg,
            ) => AgentError::InvalidAgentId(msg),
            crate::model::agent::bindings::golem::agent::common::AgentError::CustomError(value) => {
                AgentError::CustomError(value.into())
            }
        }
    }
}

impl From<AgentError> for super::bindings::golem::agent::common::AgentError {
    fn from(value: AgentError) -> Self {
        match value {
            AgentError::InvalidInput(msg) => {
                super::bindings::golem::agent::common::AgentError::InvalidInput(msg)
            }
            AgentError::InvalidMethod(msg) => {
                super::bindings::golem::agent::common::AgentError::InvalidMethod(msg)
            }
            AgentError::InvalidType(msg) => {
                super::bindings::golem::agent::common::AgentError::InvalidType(msg)
            }
            AgentError::InvalidAgentId(msg) => {
                super::bindings::golem::agent::common::AgentError::InvalidAgentId(msg)
            }
            AgentError::CustomError(value) => {
                super::bindings::golem::agent::common::AgentError::CustomError(value.into())
            }
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentMethod> for AgentMethod {
    fn from(value: crate::model::agent::bindings::golem::agent::common::AgentMethod) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: DataSchema::from(value.input_schema),
            output_schema: DataSchema::from(value.output_schema),
        }
    }
}

impl From<AgentMethod> for super::bindings::golem::agent::common::AgentMethod {
    fn from(value: AgentMethod) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value.input_schema.into(),
            output_schema: value.output_schema.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentType> for AgentType {
    fn from(value: crate::model::agent::bindings::golem::agent::common::AgentType) -> Self {
        Self {
            type_name: AgentTypeName(value.type_name),
            description: value.description,
            constructor: AgentConstructor::from(value.constructor),
            methods: value.methods.into_iter().map(AgentMethod::from).collect(),
            dependencies: value
                .dependencies
                .into_iter()
                .map(AgentDependency::from)
                .collect(),
            mode: value.mode.into(),
        }
    }
}

impl From<AgentType> for super::bindings::golem::agent::common::AgentType {
    fn from(value: AgentType) -> Self {
        Self {
            type_name: value.type_name.0,
            description: value.description,
            constructor: value.constructor.into(),
            methods: value.methods.into_iter().map(AgentMethod::into).collect(),
            dependencies: value
                .dependencies
                .into_iter()
                .map(AgentDependency::into)
                .collect(),
            mode: value.mode.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::DataSchema> for DataSchema {
    fn from(value: crate::model::agent::bindings::golem::agent::common::DataSchema) -> Self {
        match value {
            crate::model::agent::bindings::golem::agent::common::DataSchema::Tuple(tuple) => {
                DataSchema::Tuple(NamedElementSchemas {
                    elements: tuple
                        .into_iter()
                        .map(|(name, schema)| NamedElementSchema {
                            name,
                            schema: ElementSchema::from(schema),
                        })
                        .collect(),
                })
            }
            crate::model::agent::bindings::golem::agent::common::DataSchema::Multimodal(
                multimodal,
            ) => DataSchema::Multimodal(NamedElementSchemas {
                elements: multimodal
                    .into_iter()
                    .map(|(name, schema)| NamedElementSchema {
                        name,
                        schema: ElementSchema::from(schema),
                    })
                    .collect(),
            }),
        }
    }
}

impl From<DataSchema> for super::bindings::golem::agent::common::DataSchema {
    fn from(value: DataSchema) -> Self {
        match value {
            DataSchema::Tuple(tuple) => super::bindings::golem::agent::common::DataSchema::Tuple(
                tuple
                    .elements
                    .into_iter()
                    .map(|named| (named.name, named.schema.into()))
                    .collect(),
            ),
            DataSchema::Multimodal(multimodal) => {
                super::bindings::golem::agent::common::DataSchema::Multimodal(
                    multimodal
                        .elements
                        .into_iter()
                        .map(|named| (named.name, named.schema.into()))
                        .collect(),
                )
            }
        }
    }
}

impl DataValue {
    pub fn try_from_bindings(
        value: crate::model::agent::bindings::golem::agent::common::DataValue,
        schema: crate::model::agent::bindings::golem::agent::common::DataSchema,
    ) -> Result<Self, String> {
        match (value, schema) {
            (
                crate::model::agent::bindings::golem::agent::common::DataValue::Tuple(tuple),
                crate::model::agent::bindings::golem::agent::common::DataSchema::Tuple(schema),
            ) => {
                if tuple.len() != schema.len() {
                    return Err("Tuple length mismatch".to_string());
                }
                Ok(DataValue::Tuple(ElementValues {
                    elements: tuple
                        .into_iter()
                        .zip(schema)
                        .map(|(value, schema)| ElementValue::try_from_bindings(value, schema.1))
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            (
                crate::model::agent::bindings::golem::agent::common::DataValue::Multimodal(
                    multimodal,
                ),
                crate::model::agent::bindings::golem::agent::common::DataSchema::Multimodal(schema),
            ) => Ok(DataValue::Multimodal(NamedElementValues {
                elements: multimodal
                    .into_iter()
                    .zip(schema)
                    .map(|((name, value), schema)| {
                        ElementValue::try_from_bindings(value, schema.1)
                            .map(|v| NamedElementValue { name, value: v })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            })),
            _ => Err("Data value does not match schema".to_string()),
        }
    }

    pub fn try_from_untyped(value: UntypedDataValue, schema: DataSchema) -> Result<Self, String> {
        match (value, schema) {
            (UntypedDataValue::Tuple(tuple), DataSchema::Tuple(schema)) => {
                if tuple.elements.len() != schema.elements.len() {
                    return Err("Tuple length mismatch".to_string());
                }
                Ok(DataValue::Tuple(ElementValues {
                    elements: tuple
                        .elements
                        .into_iter()
                        .zip(schema.elements)
                        .map(|(value, schema)| ElementValue::try_from_untyped(value, schema.schema))
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            (UntypedDataValue::Multimodal(multimodal), DataSchema::Multimodal(schema)) => {
                Ok(DataValue::Multimodal(NamedElementValues {
                    elements: multimodal
                        .elements
                        .into_iter()
                        .zip(schema.elements)
                        .map(|(value, schema)| {
                            ElementValue::try_from_untyped(value.value, schema.schema).map(|v| {
                                NamedElementValue {
                                    name: value.name,
                                    value: v,
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            _ => Err("Data value does not match schema".to_string()),
        }
    }
}

impl From<DataValue> for super::bindings::golem::agent::common::DataValue {
    fn from(value: DataValue) -> Self {
        match value {
            DataValue::Tuple(tuple) => super::bindings::golem::agent::common::DataValue::Tuple(
                tuple.elements.into_iter().map(ElementValue::into).collect(),
            ),
            DataValue::Multimodal(multimodal) => {
                super::bindings::golem::agent::common::DataValue::Multimodal(
                    multimodal
                        .elements
                        .into_iter()
                        .map(|v| (v.name, ElementValue::into(v.value)))
                        .collect(),
                )
            }
        }
    }
}

impl From<super::bindings::golem::agent::common::ElementSchema> for ElementSchema {
    fn from(value: crate::model::agent::bindings::golem::agent::common::ElementSchema) -> Self {
        match value {
            crate::model::agent::bindings::golem::agent::common::ElementSchema::ComponentModel(wit_type) => {
                ElementSchema::ComponentModel(ComponentModelElementSchema {
                    element_type: wit_type.into(),
                })
            }
            crate::model::agent::bindings::golem::agent::common::ElementSchema::UnstructuredText(text) => {
                ElementSchema::UnstructuredText(text.into())
            }
            crate::model::agent::bindings::golem::agent::common::ElementSchema::UnstructuredBinary(binary) => {
                ElementSchema::UnstructuredBinary(binary.into())
            }
        }
    }
}

impl From<ElementSchema> for super::bindings::golem::agent::common::ElementSchema {
    fn from(value: ElementSchema) -> Self {
        match value {
            ElementSchema::ComponentModel(component_model_element_schema) => {
                super::bindings::golem::agent::common::ElementSchema::ComponentModel(
                    component_model_element_schema.element_type.into(),
                )
            }
            ElementSchema::UnstructuredText(text) => {
                super::bindings::golem::agent::common::ElementSchema::UnstructuredText(text.into())
            }
            ElementSchema::UnstructuredBinary(binary) => {
                super::bindings::golem::agent::common::ElementSchema::UnstructuredBinary(
                    binary.into(),
                )
            }
        }
    }
}

impl ElementValue {
    pub fn try_from_bindings(
        value: crate::model::agent::bindings::golem::agent::common::ElementValue,
        schema: crate::model::agent::bindings::golem::agent::common::ElementSchema,
    ) -> Result<Self, String> {
        match (value, schema) {
            (
                crate::model::agent::bindings::golem::agent::common::ElementValue::ComponentModel(wit_value),
                crate::model::agent::bindings::golem::agent::common::ElementSchema::ComponentModel(wit_schema),
            ) => {
                let val: Value = wit_value.into();
                let typ: AnalysedType = wit_schema.into();
                Ok(ElementValue::ComponentModel(ValueAndType::new(val, typ)))
            }
            (
                crate::model::agent::bindings::golem::agent::common::ElementValue::UnstructuredText(text),
                crate::model::agent::bindings::golem::agent::common::ElementSchema::UnstructuredText(_),
            ) => {
                Ok(ElementValue::UnstructuredText(text.into()))
            }
            (
                crate::model::agent::bindings::golem::agent::common::ElementValue::UnstructuredBinary(binary),
                crate::model::agent::bindings::golem::agent::common::ElementSchema::UnstructuredBinary(_),
            ) => {
                Ok(ElementValue::UnstructuredBinary(binary.into()))
            }
            _ => Err("Element value does not match schema".to_string()),
        }
    }

    pub fn try_from_untyped(
        value: UntypedElementValue,
        schema: ElementSchema,
    ) -> Result<Self, String> {
        match (value, schema) {
            (
                UntypedElementValue::ComponentModel(json_value),
                ElementSchema::ComponentModel(component_model_schema),
            ) => {
                let typ: AnalysedType = component_model_schema.element_type;
                let value_and_type = ValueAndType::parse_with_type(&json_value.value, &typ)
                    .map_err(|errors: Vec<String>| {
                        format!(
                            "Failed to parse JSON as ComponentModel value: {}",
                            errors.join(", ")
                        )
                    })?;
                Ok(ElementValue::ComponentModel(value_and_type))
            }
            (UntypedElementValue::UnstructuredText(text), ElementSchema::UnstructuredText(_)) => {
                Ok(ElementValue::UnstructuredText(text.value))
            }
            (
                UntypedElementValue::UnstructuredBinary(binary),
                ElementSchema::UnstructuredBinary(_),
            ) => Ok(ElementValue::UnstructuredBinary(binary.value)),
            _ => Err("Element value does not match schema".to_string()),
        }
    }
}

impl From<ElementValue> for super::bindings::golem::agent::common::ElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(wit_value) => {
                super::bindings::golem::agent::common::ElementValue::ComponentModel(
                    wit_value.into(),
                )
            }
            ElementValue::UnstructuredText(text) => {
                super::bindings::golem::agent::common::ElementValue::UnstructuredText(text.into())
            }
            ElementValue::UnstructuredBinary(binary) => {
                super::bindings::golem::agent::common::ElementValue::UnstructuredBinary(
                    binary.into(),
                )
            }
        }
    }
}

impl From<super::bindings::golem::agent::common::BinaryDescriptor> for BinaryDescriptor {
    fn from(value: crate::model::agent::bindings::golem::agent::common::BinaryDescriptor) -> Self {
        Self {
            restrictions: value
                .restrictions
                .map(|r| r.into_iter().map(BinaryType::from).collect()),
        }
    }
}

impl From<BinaryDescriptor> for super::bindings::golem::agent::common::BinaryDescriptor {
    fn from(value: BinaryDescriptor) -> Self {
        Self {
            restrictions: value.restrictions.map(|r| {
                r.into_iter()
                    .map(super::bindings::golem::agent::common::BinaryType::from)
                    .collect()
            }),
        }
    }
}

impl From<super::bindings::golem::agent::common::BinaryReference> for BinaryReference {
    fn from(value: crate::model::agent::bindings::golem::agent::common::BinaryReference) -> Self {
        match value {
            crate::model::agent::bindings::golem::agent::common::BinaryReference::Url(url) => {
                BinaryReference::Url(Url { value: url })
            }
            crate::model::agent::bindings::golem::agent::common::BinaryReference::Inline(
                source,
            ) => BinaryReference::Inline(source.into()),
        }
    }
}

impl From<BinaryReference> for super::bindings::golem::agent::common::BinaryReference {
    fn from(value: BinaryReference) -> Self {
        match value {
            BinaryReference::Url(url) => {
                super::bindings::golem::agent::common::BinaryReference::Url(url.value)
            }
            BinaryReference::Inline(source) => {
                super::bindings::golem::agent::common::BinaryReference::Inline(source.into())
            }
        }
    }
}

impl From<super::bindings::golem::agent::common::BinarySource> for BinarySource {
    fn from(value: crate::model::agent::bindings::golem::agent::common::BinarySource) -> Self {
        Self {
            data: value.data,
            binary_type: value.binary_type.into(),
        }
    }
}

impl From<BinarySource> for super::bindings::golem::agent::common::BinarySource {
    fn from(value: BinarySource) -> Self {
        Self {
            data: value.data,
            binary_type: value.binary_type.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::BinaryType> for BinaryType {
    fn from(value: super::bindings::golem::agent::common::BinaryType) -> Self {
        Self {
            mime_type: value.mime_type,
        }
    }
}

impl From<BinaryType> for super::bindings::golem::agent::common::BinaryType {
    fn from(value: BinaryType) -> Self {
        Self {
            mime_type: value.mime_type,
        }
    }
}

impl From<super::bindings::golem::agent::common::TextDescriptor> for TextDescriptor {
    fn from(value: crate::model::agent::bindings::golem::agent::common::TextDescriptor) -> Self {
        Self {
            restrictions: value
                .restrictions
                .map(|r| r.into_iter().map(TextType::from).collect()),
        }
    }
}

impl From<TextDescriptor> for super::bindings::golem::agent::common::TextDescriptor {
    fn from(value: TextDescriptor) -> Self {
        Self {
            restrictions: value.restrictions.map(|r| {
                r.into_iter()
                    .map(super::bindings::golem::agent::common::TextType::from)
                    .collect()
            }),
        }
    }
}

impl From<super::bindings::golem::agent::common::TextReference> for TextReference {
    fn from(value: crate::model::agent::bindings::golem::agent::common::TextReference) -> Self {
        match value {
            crate::model::agent::bindings::golem::agent::common::TextReference::Url(url) => {
                TextReference::Url(Url { value: url })
            }
            crate::model::agent::bindings::golem::agent::common::TextReference::Inline(source) => {
                TextReference::Inline(source.into())
            }
        }
    }
}

impl From<TextReference> for super::bindings::golem::agent::common::TextReference {
    fn from(value: TextReference) -> Self {
        match value {
            TextReference::Url(url) => {
                super::bindings::golem::agent::common::TextReference::Url(url.value)
            }
            TextReference::Inline(source) => {
                super::bindings::golem::agent::common::TextReference::Inline(source.into())
            }
        }
    }
}

impl From<super::bindings::golem::agent::common::TextSource> for TextSource {
    fn from(value: crate::model::agent::bindings::golem::agent::common::TextSource) -> Self {
        Self {
            data: value.data,
            text_type: value.text_type.map(TextType::from),
        }
    }
}

impl From<TextSource> for super::bindings::golem::agent::common::TextSource {
    fn from(value: TextSource) -> Self {
        Self {
            data: value.data,
            text_type: value
                .text_type
                .map(super::bindings::golem::agent::common::TextType::from),
        }
    }
}

impl From<super::bindings::golem::agent::common::TextType> for TextType {
    fn from(value: crate::model::agent::bindings::golem::agent::common::TextType) -> Self {
        Self {
            language_code: value.language_code,
        }
    }
}

impl From<TextType> for super::bindings::golem::agent::common::TextType {
    fn from(value: TextType) -> Self {
        Self {
            language_code: value.language_code,
        }
    }
}

impl From<RegisteredAgentType> for host::RegisteredAgentType {
    fn from(value: RegisteredAgentType) -> Self {
        host::RegisteredAgentType {
            agent_type: value.agent_type.into(),
            implemented_by: value.implemented_by.component_id.into(),
        }
    }
}
