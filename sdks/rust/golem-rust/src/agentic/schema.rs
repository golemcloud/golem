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

use crate::golem_agentic::golem::agent::common::ElementSchema;
use crate::golem_agentic::golem::agent::common::ElementValue;
use crate::value_and_type::FromValueAndType;
use crate::value_and_type::IntoValue;
use crate::wasm_rpc::WitValue;
use golem_wasm::golem_rpc_0_2_x::types::ValueAndType;

pub trait Schema {
    fn get_type() -> StructuredSchema;
    fn to_structured_value(self) -> Result<StructuredValue, String>;
    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized;

    fn from_wit_value(wit_value: WitValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized;
}

#[derive(Debug)]
pub enum StructuredSchema {
    Default(ElementSchema),
    Multimodal(Vec<(String, ElementSchema)>),
}

impl StructuredSchema {
    pub fn get_element_schema(self) -> Option<ElementSchema> {
        match self {
            StructuredSchema::Default(element_schema) => Some(element_schema),
            StructuredSchema::Multimodal(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum StructuredValue {
    Default(ElementValue),
    Multimodal(Vec<(String, ElementValue)>),
}

impl StructuredValue {
    pub fn get_element_value(self) -> Option<ElementValue> {
        match self {
            StructuredValue::Default(element_value) => Some(element_value),
            StructuredValue::Multimodal(_) => None,
        }
    }

    pub fn get_multimodal_value(self) -> Option<Vec<(String, ElementValue)>> {
        match self {
            StructuredValue::Default(_) => None,
            StructuredValue::Multimodal(multimodal_values) => Some(multimodal_values),
        }
    }
}

// Handles the component model types via the IntoValue and FromValueAndType traits
// This doesn't cover UnstructuredText, UnstructuredBinary
impl<T: IntoValue + FromValueAndType> Schema for T {
    fn get_type() -> StructuredSchema {
        StructuredSchema::Default(ElementSchema::ComponentModel(T::get_type()))
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        let wit_value = self.into_value();
        Ok(StructuredValue::Default(ElementValue::ComponentModel(
            wit_value,
        )))
    }

    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String> {
        match value {
            StructuredValue::Default(ElementValue::ComponentModel(wit_value)) => {
                T::from_wit_value(wit_value, schema)
            }
            _ => Err(format!("Expected ComponentModel value, got: {:?}", value)),
        }
    }

    fn from_wit_value(wit_value: WitValue, schema: StructuredSchema) -> Result<Self, String> {
        let value_and_type = ValueAndType {
            value: wit_value,
            typ: match schema {
                StructuredSchema::Default(ElementSchema::ComponentModel(wit_type)) => wit_type,
                _ => return Err(format!("Expected ComponentModel schema, got: {:?}", schema)),
            },
        };

        T::from_value_and_type(value_and_type)
    }

    fn to_wit_value(self) -> Result<WitValue, String> {
        let value_out = self.to_structured_value()?;

        let element_value_result = match value_out {
            StructuredValue::Default(element_value) => Ok(element_value),
            StructuredValue::Multimodal(_) => {
                Err("Expected element value but found multimodal".to_string())
            }
        };

        let element_value = element_value_result?;

        match element_value {
            ElementValue::ComponentModel(wit_value) => Ok(wit_value),
            ElementValue::UnstructuredBinary(binary_reference) => Err(format!(
                "Expected ComponentModel value, got UnstructuredBinary: {:?}",
                binary_reference
            )),
            ElementValue::UnstructuredText(text_reference) => Err(format!(
                "Expected ComponentModel value, got UnstructuredText: {:?}",
                text_reference
            )),
        }
    }
}

pub trait MultimodalSchema {
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)>;

    fn get_name(&self) -> String;

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized;

    fn from_element_value(elem: (String, ElementValue)) -> Result<Self, String>
    where
        Self: Sized;

    fn from_wit_value(wit_value: (String, WitValue)) -> Result<Self, String>
    where
        Self: Sized;

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized;
}
