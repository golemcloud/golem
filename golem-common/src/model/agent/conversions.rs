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

use super::{
    AgentHttpAuthDetails, AgentPrincipal, CorsOptions, CustomHttpMethod, HeaderVariable,
    HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment, PathSegment, PathVariable,
    QueryVariable, SystemVariable, SystemVariableSegment,
};
use crate::base_model::agent::{GolemUserPrincipal, OidcPrincipal, Principal};
use crate::model::agent::{
    AgentConstructor, AgentDependency, AgentError, AgentMethod, AgentMode, AgentType,
    AgentTypeName, BinaryDescriptor, BinaryReference, BinarySource, BinaryType,
    ComponentModelElementSchema, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues,
    RegisteredAgentType, Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation,
    SnapshottingPeriodic, TextDescriptor, TextReference, TextSource, TextType, UntypedDataValue,
    UntypedElementValue, Url,
};
use crate::model::Empty;
use golem_wasm::analysis::AnalysedType;
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
            http_endpoint: value.http_endpoint.into_iter().map(|v| v.into()).collect(),
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
            http_endpoint: value.http_endpoint.into_iter().map(|v| v.into()).collect(),
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
            http_mount: value.http_mount.map(|v| v.into()),
            snapshotting: value.snapshotting.into(),
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
            http_mount: value.http_mount.map(|v| v.into()),
            snapshotting: value.snapshotting.into(),
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
                if tuple.len() != schema.elements.len() {
                    return Err("Tuple length mismatch".to_string());
                }
                Ok(DataValue::Tuple(ElementValues {
                    elements: tuple
                        .into_iter()
                        .zip(schema.elements)
                        .map(|(value, schema)| ElementValue::try_from_untyped(value, schema.schema))
                        .collect::<Result<Vec<_>, _>>()?,
                }))
            }
            (UntypedDataValue::Multimodal(multimodal), DataSchema::Multimodal(schema)) => {
                Ok(DataValue::Multimodal(NamedElementValues {
                    elements: multimodal
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
                UntypedElementValue::ComponentModel(value),
                ElementSchema::ComponentModel(component_model_schema),
            ) => {
                let typ: AnalysedType = component_model_schema.element_type;
                Ok(ElementValue::ComponentModel(ValueAndType::new(value, typ)))
            }
            (
                UntypedElementValue::UnstructuredText(text_ref),
                ElementSchema::UnstructuredText(_),
            ) => Ok(ElementValue::UnstructuredText(text_ref.value)),
            (
                UntypedElementValue::UnstructuredBinary(binary_ref),
                ElementSchema::UnstructuredBinary(_),
            ) => Ok(ElementValue::UnstructuredBinary(binary_ref.value)),
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

impl From<RegisteredAgentType> for super::bindings::golem::agent::common::RegisteredAgentType {
    fn from(value: RegisteredAgentType) -> Self {
        super::bindings::golem::agent::common::RegisteredAgentType {
            agent_type: value.agent_type.into(),
            implemented_by: value.implemented_by.component_id.into(),
        }
    }
}

impl From<HttpMountDetails> for super::bindings::golem::agent::common::HttpMountDetails {
    fn from(value: HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: value.cors_options.into(),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<super::bindings::golem::agent::common::HttpMountDetails> for HttpMountDetails {
    fn from(value: super::bindings::golem::agent::common::HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: value.cors_options.into(),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<HttpEndpointDetails> for super::bindings::golem::agent::common::HttpEndpointDetails {
    fn from(value: HttpEndpointDetails) -> Self {
        Self {
            http_method: value.http_method.into(),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: value.cors_options.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::HttpEndpointDetails> for HttpEndpointDetails {
    fn from(value: super::bindings::golem::agent::common::HttpEndpointDetails) -> Self {
        Self {
            http_method: value.http_method.into(),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: value.cors_options.into(),
        }
    }
}

impl From<HttpMethod> for super::bindings::golem::agent::common::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        match value {
            HttpMethod::Get(_) => Self::Get,
            HttpMethod::Head(_) => Self::Head,
            HttpMethod::Post(_) => Self::Post,
            HttpMethod::Put(_) => Self::Put,
            HttpMethod::Delete(_) => Self::Delete,
            HttpMethod::Connect(_) => Self::Connect,
            HttpMethod::Options(_) => Self::Options,
            HttpMethod::Trace(_) => Self::Trace,
            HttpMethod::Patch(_) => Self::Patch,
            HttpMethod::Custom(c) => Self::Custom(c.value),
        }
    }
}

impl From<super::bindings::golem::agent::common::HttpMethod> for HttpMethod {
    fn from(value: super::bindings::golem::agent::common::HttpMethod) -> Self {
        match value {
            super::bindings::golem::agent::common::HttpMethod::Get => Self::Get(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Head => Self::Head(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Post => Self::Post(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Put => Self::Put(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Delete => Self::Delete(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Connect => Self::Connect(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Options => Self::Options(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Trace => Self::Trace(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Patch => Self::Patch(Empty {}),
            super::bindings::golem::agent::common::HttpMethod::Custom(value) => {
                Self::Custom(CustomHttpMethod { value })
            }
        }
    }
}

impl From<CorsOptions> for super::bindings::golem::agent::common::CorsOptions {
    fn from(value: CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl From<super::bindings::golem::agent::common::CorsOptions> for CorsOptions {
    fn from(value: super::bindings::golem::agent::common::CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl From<PathSegment> for super::bindings::golem::agent::common::PathSegment {
    fn from(value: PathSegment) -> Self {
        match value {
            PathSegment::Literal(v) => Self::Literal(v.value),
            PathSegment::SystemVariable(v) => Self::SystemVariable(v.value.into()),
            PathSegment::PathVariable(v) => Self::PathVariable(v.into()),
            PathSegment::RemainingPathVariable(v) => Self::RemainingPathVariable(v.into()),
        }
    }
}

impl From<super::bindings::golem::agent::common::PathSegment> for PathSegment {
    fn from(value: super::bindings::golem::agent::common::PathSegment) -> Self {
        match value {
            super::bindings::golem::agent::common::PathSegment::Literal(value) => {
                Self::Literal(LiteralSegment { value })
            }
            super::bindings::golem::agent::common::PathSegment::SystemVariable(value) => {
                Self::SystemVariable(SystemVariableSegment {
                    value: value.into(),
                })
            }
            super::bindings::golem::agent::common::PathSegment::PathVariable(v) => {
                Self::PathVariable(v.into())
            }
            super::bindings::golem::agent::common::PathSegment::RemainingPathVariable(v) => {
                Self::RemainingPathVariable(v.into())
            }
        }
    }
}

impl From<SystemVariable> for super::bindings::golem::agent::common::SystemVariable {
    fn from(value: SystemVariable) -> Self {
        match value {
            SystemVariable::AgentType => Self::AgentType,
            SystemVariable::AgentVersion => Self::AgentVersion,
        }
    }
}

impl From<super::bindings::golem::agent::common::SystemVariable> for SystemVariable {
    fn from(value: super::bindings::golem::agent::common::SystemVariable) -> Self {
        match value {
            super::bindings::golem::agent::common::SystemVariable::AgentType => Self::AgentType,
            super::bindings::golem::agent::common::SystemVariable::AgentVersion => {
                Self::AgentVersion
            }
        }
    }
}

impl From<PathVariable> for super::bindings::golem::agent::common::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<super::bindings::golem::agent::common::PathVariable> for PathVariable {
    fn from(value: super::bindings::golem::agent::common::PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl From<HeaderVariable> for super::bindings::golem::agent::common::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<super::bindings::golem::agent::common::HeaderVariable> for HeaderVariable {
    fn from(value: super::bindings::golem::agent::common::HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<QueryVariable> for super::bindings::golem::agent::common::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<super::bindings::golem::agent::common::QueryVariable> for QueryVariable {
    fn from(value: super::bindings::golem::agent::common::QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl From<AgentHttpAuthDetails> for super::bindings::golem::agent::common::AuthDetails {
    fn from(value: AgentHttpAuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl From<super::bindings::golem::agent::common::AuthDetails> for AgentHttpAuthDetails {
    fn from(value: super::bindings::golem::agent::common::AuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl From<Principal> for super::bindings::golem::agent::common::Principal {
    fn from(value: Principal) -> Self {
        match value {
            Principal::Oidc(inner) => Self::Oidc(inner.into()),
            Principal::Agent(inner) => Self::Agent(inner.into()),
            Principal::GolemUser(inner) => Self::GolemUser(inner.into()),
            Principal::Anonymous(_) => Self::Anonymous,
        }
    }
}

impl From<super::bindings::golem::agent::common::Principal> for Principal {
    fn from(value: super::bindings::golem::agent::common::Principal) -> Self {
        use super::bindings::golem::agent::common::Principal as Value;

        match value {
            Value::Oidc(inner) => Self::Oidc(inner.into()),
            Value::Agent(inner) => Self::Agent(inner.into()),
            Value::GolemUser(inner) => Self::GolemUser(inner.into()),
            Value::Anonymous => Self::Anonymous(Empty {}),
        }
    }
}

impl From<OidcPrincipal> for super::bindings::golem::agent::common::OidcPrincipal {
    fn from(value: OidcPrincipal) -> Self {
        Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        }
    }
}

impl From<super::bindings::golem::agent::common::OidcPrincipal> for OidcPrincipal {
    fn from(value: super::bindings::golem::agent::common::OidcPrincipal) -> Self {
        Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        }
    }
}

impl From<AgentPrincipal> for super::bindings::golem::agent::common::AgentPrincipal {
    fn from(value: AgentPrincipal) -> Self {
        Self {
            agent_id: value.agent_id.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::AgentPrincipal> for AgentPrincipal {
    fn from(value: super::bindings::golem::agent::common::AgentPrincipal) -> Self {
        Self {
            agent_id: value.agent_id.into(),
        }
    }
}

impl From<GolemUserPrincipal> for super::bindings::golem::agent::common::GolemUserPrincipal {
    fn from(value: GolemUserPrincipal) -> Self {
        Self {
            account_id: value.account_id.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::GolemUserPrincipal> for GolemUserPrincipal {
    fn from(value: super::bindings::golem::agent::common::GolemUserPrincipal) -> Self {
        Self {
            account_id: value.account_id.into(),
        }
    }
}

impl From<super::bindings::golem::agent::common::Snapshotting> for Snapshotting {
    fn from(value: super::bindings::golem::agent::common::Snapshotting) -> Self {
        match value {
            super::bindings::golem::agent::common::Snapshotting::Disabled => {
                Self::Disabled(Empty {})
            }
            super::bindings::golem::agent::common::Snapshotting::Enabled(config) => {
                Self::Enabled(config.into())
            }
        }
    }
}

impl From<Snapshotting> for super::bindings::golem::agent::common::Snapshotting {
    fn from(value: Snapshotting) -> Self {
        match value {
            Snapshotting::Disabled(_) => Self::Disabled,
            Snapshotting::Enabled(config) => Self::Enabled(config.into()),
        }
    }
}

impl From<super::bindings::golem::agent::common::SnapshottingConfig> for SnapshottingConfig {
    fn from(value: super::bindings::golem::agent::common::SnapshottingConfig) -> Self {
        match value {
            super::bindings::golem::agent::common::SnapshottingConfig::Default => {
                Self::Default(Empty {})
            }
            super::bindings::golem::agent::common::SnapshottingConfig::Periodic(nanos) => {
                Self::Periodic(SnapshottingPeriodic {
                    duration_nanos: nanos,
                })
            }
            super::bindings::golem::agent::common::SnapshottingConfig::EveryNInvocation(n) => {
                Self::EveryNInvocation(SnapshottingEveryNInvocation { count: n })
            }
        }
    }
}

impl From<SnapshottingConfig> for super::bindings::golem::agent::common::SnapshottingConfig {
    fn from(value: SnapshottingConfig) -> Self {
        match value {
            SnapshottingConfig::Default(_) => Self::Default,
            SnapshottingConfig::Periodic(periodic) => Self::Periodic(periodic.duration_nanos),
            SnapshottingConfig::EveryNInvocation(every_n) => Self::EveryNInvocation(every_n.count),
        }
    }
}
