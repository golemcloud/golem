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

use crate::model::agent::{
    AgentConstructor, AgentDependency, AgentMethod, AgentType, ComponentModelElementSchema,
    DataSchema, DataValue, ElementSchema, ElementValue, ElementValues, NamedElementSchema,
    NamedElementSchemas, NamedElementValue, NamedElementValues,
};
use golem_wasm::analysis::{
    AnalysedType, NameOptionTypePair, NameTypePair, TypeEnum, TypeFlags, TypeHandle, TypeList,
    TypeOption, TypeRecord, TypeResult, TypeTuple, TypeVariant,
};
use golem_wasm::ValueAndType;
use super::{AgentHttpAuthContext, AgentHttpAuthDetails, CorsOptions, CustomHttpMethod, HeaderVariable, HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment, PathSegment, PathSegmentNode, PathVariable, QueryVariable, SystemVariable, SystemVariableSegment};

/// ToWitNaming allows converting discovered AgentTypes to WIT and WAVE compatible naming for named
/// elements
pub trait ToWitNaming {
    fn to_wit_naming(&self) -> Self;
}

impl ToWitNaming for String {
    fn to_wit_naming(&self) -> Self {
        // NOTE: wrap and include kebab case only here, in case we have to handle more WIT specific
        //       special cases.
        heck::ToKebabCase::to_kebab_case(self.as_str())
    }
}

impl<T> ToWitNaming for Option<T>
where
    T: ToWitNaming,
{
    fn to_wit_naming(&self) -> Self {
        self.as_ref().map(|t| t.to_wit_naming())
    }
}

impl<T> ToWitNaming for Box<T>
where
    T: ToWitNaming,
{
    fn to_wit_naming(&self) -> Self {
        Box::new(self.as_ref().to_wit_naming())
    }
}

impl<T> ToWitNaming for Vec<T>
where
    T: ToWitNaming,
{
    fn to_wit_naming(&self) -> Self {
        self.iter().map(|t| t.to_wit_naming()).collect()
    }
}

impl ToWitNaming for AgentType {
    fn to_wit_naming(&self) -> Self {
        Self {
            type_name: self.type_name.clone(),
            description: self.description.clone(),
            constructor: self.constructor.to_wit_naming(),
            methods: self.methods.to_wit_naming(),
            dependencies: self.dependencies.to_wit_naming(),
            mode: self.mode,
            http_mount: self.http_mount.as_ref().map(ToWitNaming::to_wit_naming)
        }
    }
}

impl ToWitNaming for AgentConstructor {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.to_wit_naming(),
            description: self.description.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema: self.input_schema.to_wit_naming(),
        }
    }
}

impl ToWitNaming for AgentMethod {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.to_wit_naming(),
            description: self.description.clone(),
            prompt_hint: self.prompt_hint.clone(),
            input_schema: self.input_schema.to_wit_naming(),
            output_schema: self.output_schema.to_wit_naming(),
            http_endpoint: self.http_endpoint.iter().map(ToWitNaming::to_wit_naming).collect()
        }
    }
}

impl ToWitNaming for AgentDependency {
    fn to_wit_naming(&self) -> Self {
        Self {
            type_name: self.type_name.to_wit_naming(),
            description: self.description.clone(),
            constructor: self.constructor.to_wit_naming(),
            methods: self.methods.to_wit_naming(),
        }
    }
}

impl ToWitNaming for DataSchema {
    fn to_wit_naming(&self) -> Self {
        match self {
            DataSchema::Tuple(elems) => DataSchema::Tuple(elems.to_wit_naming()),
            DataSchema::Multimodal(elems) => DataSchema::Multimodal(elems.to_wit_naming()),
        }
    }
}

impl ToWitNaming for NamedElementSchemas {
    fn to_wit_naming(&self) -> Self {
        Self {
            elements: self.elements.to_wit_naming(),
        }
    }
}

impl ToWitNaming for NamedElementSchema {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.to_wit_naming(),
            schema: self.schema.to_wit_naming(),
        }
    }
}

impl ToWitNaming for ElementSchema {
    fn to_wit_naming(&self) -> Self {
        match self {
            ElementSchema::ComponentModel(schema) => {
                ElementSchema::ComponentModel(schema.to_wit_naming())
            }
            ElementSchema::UnstructuredText(descriptor) => {
                ElementSchema::UnstructuredText(descriptor.clone())
            }
            ElementSchema::UnstructuredBinary(descriptor) => {
                ElementSchema::UnstructuredBinary(descriptor.clone())
            }
        }
    }
}

impl ToWitNaming for ComponentModelElementSchema {
    fn to_wit_naming(&self) -> Self {
        Self {
            element_type: self.element_type.to_wit_naming(),
        }
    }
}

impl ToWitNaming for AnalysedType {
    fn to_wit_naming(&self) -> Self {
        match self {
            AnalysedType::Variant(variant) => AnalysedType::Variant(TypeVariant {
                name: variant.name.to_wit_naming(),
                owner: variant.owner.to_wit_naming(),
                cases: variant.cases.to_wit_naming(),
            }),
            AnalysedType::Result(result) => AnalysedType::Result(TypeResult {
                name: result.name.to_wit_naming(),
                owner: result.owner.to_wit_naming(),
                ok: result.ok.to_wit_naming(),
                err: result.err.to_wit_naming(),
            }),
            AnalysedType::Option(option) => AnalysedType::Option(TypeOption {
                name: option.name.to_wit_naming(),
                owner: option.owner.to_wit_naming(),
                inner: option.inner.to_wit_naming(),
            }),
            AnalysedType::Enum(enum_type) => AnalysedType::Enum(TypeEnum {
                name: enum_type.name.to_wit_naming(),
                owner: enum_type.owner.to_wit_naming(),
                cases: enum_type.cases.to_wit_naming(),
            }),
            AnalysedType::Flags(flags) => AnalysedType::Flags(TypeFlags {
                name: flags.name.to_wit_naming(),
                owner: flags.owner.to_wit_naming(),
                names: flags.names.to_wit_naming(),
            }),
            AnalysedType::Record(record) => AnalysedType::Record(TypeRecord {
                name: record.name.to_wit_naming(),
                owner: record.owner.to_wit_naming(),
                fields: record.fields.to_wit_naming(),
            }),
            AnalysedType::Tuple(tuple) => AnalysedType::Tuple(TypeTuple {
                name: tuple.name.to_wit_naming(),
                owner: tuple.owner.to_wit_naming(),
                items: tuple.items.to_wit_naming(),
            }),
            AnalysedType::List(list) => AnalysedType::List(TypeList {
                name: list.name.to_wit_naming(),
                owner: list.owner.to_wit_naming(),
                inner: list.inner.to_wit_naming(),
            }),
            AnalysedType::Handle(handle) => AnalysedType::Handle(TypeHandle {
                name: handle.name.to_wit_naming(),
                owner: handle.owner.to_wit_naming(),
                resource_id: handle.resource_id,
                mode: handle.mode.clone(),
            }),
            AnalysedType::Str(_)
            | AnalysedType::Chr(_)
            | AnalysedType::F64(_)
            | AnalysedType::F32(_)
            | AnalysedType::U64(_)
            | AnalysedType::S64(_)
            | AnalysedType::U32(_)
            | AnalysedType::S32(_)
            | AnalysedType::U16(_)
            | AnalysedType::S16(_)
            | AnalysedType::U8(_)
            | AnalysedType::S8(_)
            | AnalysedType::Bool(_) => self.clone(),
        }
    }
}

impl ToWitNaming for NameOptionTypePair {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.to_wit_naming(),
            typ: self.typ.to_wit_naming(),
        }
    }
}

impl ToWitNaming for NameTypePair {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.to_wit_naming(),
            typ: self.typ.to_wit_naming(),
        }
    }
}

impl ToWitNaming for DataValue {
    fn to_wit_naming(&self) -> Self {
        match self {
            DataValue::Tuple(elems) => DataValue::Tuple(elems.to_wit_naming()),
            DataValue::Multimodal(elems) => DataValue::Multimodal(elems.to_wit_naming()),
        }
    }
}

impl ToWitNaming for ElementValues {
    fn to_wit_naming(&self) -> Self {
        Self {
            elements: self
                .elements
                .iter()
                .map(|elem| elem.to_wit_naming())
                .collect(),
        }
    }
}

impl ToWitNaming for NamedElementValues {
    fn to_wit_naming(&self) -> Self {
        Self {
            elements: self
                .elements
                .iter()
                .map(|elem| elem.to_wit_naming())
                .collect(),
        }
    }
}

impl ToWitNaming for NamedElementValue {
    fn to_wit_naming(&self) -> Self {
        Self {
            name: self.name.clone(),
            value: self.value.to_wit_naming(),
        }
    }
}

impl ToWitNaming for ElementValue {
    fn to_wit_naming(&self) -> Self {
        match self {
            ElementValue::ComponentModel(vnt) => ElementValue::ComponentModel(ValueAndType::new(
                vnt.value.clone(),
                vnt.typ.to_wit_naming(),
            )),
            ElementValue::UnstructuredText(_) => self.clone(),
            ElementValue::UnstructuredBinary(_) => self.clone(),
        }
    }
}

impl ToWitNaming for HttpMountDetails {
    fn to_wit_naming(&self) -> Self {
        Self {
            path_prefix: self.path_prefix.iter().map(ToWitNaming::to_wit_naming).collect(),
            header_vars: self.header_vars.iter().map(ToWitNaming::to_wit_naming).collect(),
            query_vars: self.query_vars.iter().map(ToWitNaming::to_wit_naming).collect(),
            auth_details: self.auth_details.as_ref().map(ToWitNaming::to_wit_naming),
            phantom_agent: self.phantom_agent,
            cors_options: self.cors_options.to_wit_naming(),
            webhook_suffix: self.webhook_suffix.iter().map(ToWitNaming::to_wit_naming).collect(),
        }
    }
}

impl ToWitNaming for HttpEndpointDetails {
    fn to_wit_naming(&self) -> Self {
        Self {
            http_method: self.http_method.to_wit_naming(),
            path_suffix: self.path_suffix.iter().map(ToWitNaming::to_wit_naming).collect(),
            header_vars: self.header_vars.iter().map(ToWitNaming::to_wit_naming).collect(),
            query_vars: self.query_vars.iter().map(ToWitNaming::to_wit_naming).collect(),
            auth_details: self.auth_details.as_ref().map(ToWitNaming::to_wit_naming),
            cors_options: self.cors_options.to_wit_naming(),
        }
    }
}

impl ToWitNaming for HttpMethod {
    fn to_wit_naming(&self) -> Self {
        match self {
            HttpMethod::Get(e) => HttpMethod::Get(e.clone()),
            HttpMethod::Put(e) => HttpMethod::Put(e.clone()),
            HttpMethod::Post(e) => HttpMethod::Post(e.clone()),
            HttpMethod::Delete(e) => HttpMethod::Delete(e.clone()),
            HttpMethod::Custom(c) => HttpMethod::Custom(c.to_wit_naming()),
        }
    }
}

impl ToWitNaming for CustomHttpMethod {
    fn to_wit_naming(&self) -> Self {
        Self {
            value: self.value.to_wit_naming(),
        }
    }
}

impl ToWitNaming for CorsOptions {
    fn to_wit_naming(&self) -> Self {
        Self {
            allowed_patterns: self.allowed_patterns.iter().map(ToWitNaming::to_wit_naming).collect(),
        }
    }
}

impl ToWitNaming for PathSegment {
    fn to_wit_naming(&self) -> Self {
        Self {
            concat: self.concat.iter().map(ToWitNaming::to_wit_naming).collect(),
        }
    }
}

impl ToWitNaming for PathSegmentNode {
    fn to_wit_naming(&self) -> Self {
        match self {
            PathSegmentNode::Literal(v) => Self::Literal(v.to_wit_naming()),
            PathSegmentNode::SystemVariable(v) => Self::SystemVariable(v.to_wit_naming()),
            PathSegmentNode::PathVariable(v) => Self::PathVariable(v.to_wit_naming()),
        }
    }
}

impl ToWitNaming for LiteralSegment {
    fn to_wit_naming(&self) -> Self {
        Self {
            value: self.value.to_wit_naming(),
        }
    }
}

impl ToWitNaming for SystemVariableSegment {
    fn to_wit_naming(&self) -> Self {
        Self {
            value: self.value,
        }
    }
}

impl ToWitNaming for SystemVariable {
    fn to_wit_naming(&self) -> Self {
        *self
    }
}

impl ToWitNaming for PathVariable {
    fn to_wit_naming(&self) -> Self {
        Self {
            variable_name: self.variable_name.to_wit_naming(),
        }
    }
}

impl ToWitNaming for HeaderVariable {
    fn to_wit_naming(&self) -> Self {
        Self {
            header_name: self.header_name.to_wit_naming(),
            variable_name: self.variable_name.to_wit_naming(),
        }
    }
}

impl ToWitNaming for QueryVariable {
    fn to_wit_naming(&self) -> Self {
        Self {
            query_param_name: self.query_param_name.to_wit_naming(),
            variable_name: self.variable_name.to_wit_naming(),
        }
    }
}

impl ToWitNaming for AgentHttpAuthDetails {
    fn to_wit_naming(&self) -> Self {
        Self {
            required: self.required,
        }
    }
}

impl ToWitNaming for AgentHttpAuthContext {
    fn to_wit_naming(&self) -> Self {
        Self {
            sub: self.sub.to_wit_naming(),
            provider: self.provider.to_wit_naming(),
            email: self.email.to_wit_naming(),
            name: self.name.to_wit_naming(),
            email_verified: self.email_verified,
            given_name: self.given_name.as_ref().map(ToWitNaming::to_wit_naming),
            family_name: self.family_name.as_ref().map(ToWitNaming::to_wit_naming),
            picture: self.picture.as_ref().map(ToWitNaming::to_wit_naming),
            preferred_username: self.preferred_username.as_ref().map(ToWitNaming::to_wit_naming),
            claims: self.claims.to_wit_naming(),
        }
    }
}
