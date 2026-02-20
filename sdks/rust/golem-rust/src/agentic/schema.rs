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

use crate::golem_agentic::golem::agent::common::{
    DataValue, ElementSchema, ElementValue, Principal,
};
use crate::value_and_type::FromValueAndType;
use crate::value_and_type::IntoValue;
use golem_wasm::golem_core_1_5_x::types::ValueAndType;

pub trait Schema {
    /// Has to be consistent with the data returned by get_type
    const IS_COMPONENT_MODEL_SCHEMA: bool;

    fn get_type() -> StructuredSchema;
    fn to_structured_value(self) -> Result<StructuredValue, String>;
    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized;

    fn to_element_value(self) -> Result<ElementValue, String>
    where
        Self: Sized;

    fn from_element_value(value: ElementValue) -> Result<Self, String>
    where
        Self: Sized;

    fn to_data_value(self) -> Result<DataValue, String>
    where
        Self: Sized,
    {
        Ok(DataValue::Tuple(vec![self.to_element_value()?]))
    }

    fn from_data_value(value: DataValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            DataValue::Tuple(mut elements) if elements.len() == 1 => {
                Self::from_element_value(elements.remove(0))
            }
            other => Err(format!(
                "Expected single-element Tuple DataValue, got: {:?}",
                other
            )),
        }
    }
}

#[derive(Debug)]
pub enum StructuredSchema {
    AutoInject(AutoInjectedParamType),
    Default(ElementSchema),
    Multimodal(Vec<(String, ElementSchema)>),
}

#[derive(Debug, Clone)]
pub enum AutoInjectedParamType {
    Principal,
}

impl StructuredSchema {
    pub fn get_element_schema(self) -> Option<ElementSchema> {
        match self {
            StructuredSchema::Default(element_schema) => Some(element_schema),
            StructuredSchema::Multimodal(_) => None,
            StructuredSchema::AutoInject(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum StructuredValue {
    Default(ElementValue),
    Multimodal(Vec<(String, ElementValue)>),
    AutoInjected(AutoInjectedValue),
}

#[derive(Debug)]
pub enum AutoInjectedValue {
    Principal(Principal),
}

impl StructuredValue {
    pub fn get_element_value(self) -> Option<ElementValue> {
        match self {
            StructuredValue::Default(element_value) => Some(element_value),
            StructuredValue::Multimodal(_) => None,
            StructuredValue::AutoInjected(_) => None,
        }
    }

    pub fn get_multimodal_value(self) -> Option<Vec<(String, ElementValue)>> {
        match self {
            StructuredValue::Default(_) => None,
            StructuredValue::Multimodal(multimodal_values) => Some(multimodal_values),
            StructuredValue::AutoInjected(_) => None,
        }
    }

    pub fn get_principal_value(self) -> Option<Principal> {
        match self {
            StructuredValue::AutoInjected(AutoInjectedValue::Principal(principal)) => {
                Some(principal)
            }
            _ => None,
        }
    }
}

impl Schema for Principal {
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        StructuredSchema::AutoInject(AutoInjectedParamType::Principal)
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        Ok(StructuredValue::AutoInjected(AutoInjectedValue::Principal(
            self,
        )))
    }

    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        match (value, schema) {
            (
                StructuredValue::AutoInjected(AutoInjectedValue::Principal(principal)),
                StructuredSchema::AutoInject(AutoInjectedParamType::Principal),
            ) => Ok(principal),
            _ => Err("Mismatched value and schema for Principal".to_string()),
        }
    }

    fn to_element_value(self) -> Result<ElementValue, String> {
        Err("Principal is not expected to be converted to ElementValue".to_string())
    }

    fn from_element_value(_value: ElementValue) -> Result<Self, String> {
        Err("Principal is not expected to be converted from ElementValue".to_string())
    }
}

// Handles the component model types via the IntoValue and FromValueAndType traits
// This doesn't cover UnstructuredText, UnstructuredBinary
impl<T: IntoValue + FromValueAndType> Schema for T {
    const IS_COMPONENT_MODEL_SCHEMA: bool = true;

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
                let value_and_type = ValueAndType {
                    value: wit_value,
                    typ: match schema {
                        StructuredSchema::Default(ElementSchema::ComponentModel(wit_type)) => {
                            wit_type
                        }
                        _ => {
                            return Err(format!(
                                "Expected ComponentModel schema, got: {:?}",
                                schema
                            ))
                        }
                    },
                };
                T::from_value_and_type(value_and_type)
            }
            _ => Err(format!("Expected ComponentModel value, got: {:?}", value)),
        }
    }

    fn to_element_value(self) -> Result<ElementValue, String> {
        Ok(ElementValue::ComponentModel(self.into_value()))
    }

    fn from_element_value(value: ElementValue) -> Result<Self, String> {
        match value {
            ElementValue::ComponentModel(wv) => {
                let value_and_type = ValueAndType {
                    value: wv,
                    typ: T::get_type(),
                };
                T::from_value_and_type(value_and_type)
            }
            other => Err(format!(
                "Expected ComponentModel ElementValue, got: {:?}",
                other
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

    fn to_raw_element_value(self) -> Result<ElementValue, String>
    where
        Self: Sized;

    fn from_raw_element_value(name: String, value: ElementValue) -> Result<Self, String>
    where
        Self: Sized;
}
