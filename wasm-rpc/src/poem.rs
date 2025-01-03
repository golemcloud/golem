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

use crate::protobuf::type_annotated_value::TypeAnnotatedValue;
use crate::ValueAndType;
use golem_wasm_ast::analysis::AnalysedType;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef, Registry};
use poem_openapi::types::{IsObjectType, ParseFromJSON, ParseResult, ToJSON, Type};
use serde_json::Value;
use std::borrow::Cow;

impl Type for TypeAnnotatedValue {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "TypeAnnotatedValue".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |registry| {
            AnalysedType::register(registry);
            serde_json::Value::register(registry);
            let mut schema = MetaSchema::new("object");
            schema.required = vec!["typ", "value"];
            schema.properties = vec![
                ("typ", AnalysedType::schema_ref()),
                ("value", Value::schema_ref()),
            ];

            schema
        });
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ToJSON for TypeAnnotatedValue {
    fn to_json(&self) -> Option<Value> {
        serde_json::to_value(self).ok()
    }
}

impl ParseFromJSON for TypeAnnotatedValue {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        Ok(serde_json::from_value(value.unwrap_or_default())?)
    }
}

impl IsObjectType for TypeAnnotatedValue {}

impl Type for ValueAndType {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "ValueAndType".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        MetaSchemaRef::Reference(Self::name().into_owned())
    }

    fn register(registry: &mut Registry) {
        registry.create_schema::<Self, _>(Self::name().into_owned(), |registry| {
            AnalysedType::register(registry);
            serde_json::Value::register(registry);
            let mut schema = MetaSchema::new("object");
            schema.required = vec!["typ", "value"];
            schema.properties = vec![
                ("typ", AnalysedType::schema_ref()),
                ("value", Value::schema_ref()),
            ];

            schema
        });
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ToJSON for ValueAndType {
    fn to_json(&self) -> Option<Value> {
        serde_json::to_value(self).ok()
    }
}

impl ParseFromJSON for ValueAndType {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        Ok(serde_json::from_value(value.unwrap_or_default())?)
    }
}

impl IsObjectType for ValueAndType {}
