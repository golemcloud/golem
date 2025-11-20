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

use crate::agentic::{MultimodalSchema, Schema, UnstructuredBinary, UnstructuredText};
use crate::golem_agentic::golem::agent::common::{DataValue, ElementSchema, ElementValue};
use golem_wasm::{Value, WitValue};

/// Represents Multimodal input data for agent functions.
/// Note that you cannot mix a multimodal input with other input types
///
/// # Example
///
/// ```
/// use golem_rust::agentic::{Multimodal};
/// use golem_rust::MultimodalSchema;
///
/// // Create a multimodal dataset with text and image inputs
/// let multimodal_data = Multimodal::new([
///     Input::Text("foo".to_string()),
///     Input::Image(vec![1, 2, 3])
/// ]);
///
/// #[derive(MultimodalSchema)]
/// enum Input {
///     Text(String),
///     Image(Vec<u8>),
/// }
///
/// // Function that shows how an agent might receive multimodal input
/// fn my_agent_method(input: Multimodal<Input>) {
///     // handle the multimodal input here
/// }
///
/// my_agent_method(multimodal_data);
/// ```
///
/// # Notes
/// - Each variant of the `Input` enum represents a **modality**.
/// - The `Multimodal` type can take a variable number of such variants.
/// - If `Multimodal` is used in agents, their schema will reflect both
///   the **multimodal structure** and **type of each modality**.
/// - Unlike a plain `Vec<MultimodalInput>`, this type carries additional semantic and schema-level information
///   that indicates the data represents a *multimodal input* — not just a generic list.
///
pub struct Multimodal<T> {
    pub items: Vec<T>,
}

impl<T: MultimodalSchema> Multimodal<T> {
    /// Create a Multimodal input data for agent functions.
    /// Note that you cannot mix a multimodal input with other input types
    ///
    /// # Example
    ///
    /// ```
    /// use golem_rust::agentic::{Multimodal};
    /// use golem_rust::MultimodalSchema;
    ///
    /// // Create a multimodal dataset with text and image inputs
    /// let multimodal_data = Multimodal::new([
    ///     Input::Text("foo".to_string()),
    ///     Input::Image(vec![1, 2, 3])
    /// ]);
    ///
    /// #[derive(MultimodalSchema)]
    /// enum Input {
    ///     Text(String),
    ///     Image(Vec<u8>),
    /// }
    ///
    /// // Function that shows how an agent might receive multimodal input
    /// fn my_agent_method(input: Multimodal<Input>) {
    ///     // handle the multimodal input here
    /// }
    ///
    /// my_agent_method(multimodal_data);
    /// ```
    ///
    /// If you need a predefined basic multimodal type with text and binary data, you can use `MultimodalBasic` .
    ///
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn get_schema() -> Vec<(String, ElementSchema)> {
        T::get_multimodal_schema()
    }

    // With Multimodal schema we get name and element schema
    pub fn to_data_value(self) -> Result<DataValue, String> {
        let items = self.items;

        let mut elements = Vec::new();

        for item in items {
            let serialized = <T as MultimodalSchema>::to_element_value(item)?;
            elements.push(serialized);
        }

        Ok(DataValue::Multimodal(elements))
    }

    pub fn from_data_value(data: DataValue) -> Result<Self, String> {
        match data {
            DataValue::Multimodal(elements) => Self::from_element_values(elements),
            _ => Err("Expected Multimodal DataValue".to_string()),
        }
    }

    pub fn from_element_values(
        elems: Vec<(String, ElementValue)>,
    ) -> Result<Multimodal<T>, String> {
        let mut items = Vec::new();

        for elem in elems {
            let item = <T as MultimodalSchema>::from_element_value(elem)?;
            items.push(item);
        }

        Ok(Multimodal { items })
    }

    pub fn to_wit_value(self) -> Result<WitValue, String> {
        let schema = Self::get_schema();
        let mut variants: Vec<Value> = vec![];

        for v in self.items {
            let variant_name = <T as MultimodalSchema>::get_name(&v);
            let wit_value = <T as MultimodalSchema>::to_wit_value(v)?;
            let value = Value::from(wit_value);
            let variant_index = schema
                .iter()
                .position(|(name, _)| name == &variant_name)
                .ok_or_else(|| format!("Unknown modality name: {}", variant_name))?;

            variants.push(Value::Variant {
                case_idx: variant_index as u32,
                case_value: Some(Box::new(value)),
            });
        }

        let value = Value::List(variants);

        Ok(WitValue::from(value))
    }

    // Mainly exists because the rpc invoke result is a wit value and it's a list of variants
    pub fn from_wit_value(wit_value: WitValue) -> Result<Self, String> {
        let value = Value::from(wit_value);

        match value {
            Value::List(variants) => {
                let mut items = Vec::new();
                let schema = Self::get_schema();

                for variant in variants {
                    if let Value::Variant {
                        case_idx,
                        case_value,
                    } = variant
                    {
                        let modality_schema = schema
                            .get(case_idx as usize)
                            .ok_or_else(|| format!("Invalid modality index: {}", case_idx))?;

                        let modality_name = &modality_schema.0;

                        let case_value = case_value.ok_or_else(|| {
                            format!("Missing value for modality: {}", modality_name)
                        })?;

                        let wit_value = WitValue::from(*case_value);

                        let item = <T as MultimodalSchema>::from_wit_value((
                            modality_name.to_string(),
                            wit_value,
                        ))?;

                        items.push(item);
                    } else {
                        return Err("Expected Variant value in Multimodal list".to_string());
                    }
                }

                Ok(Multimodal { items })
            }
            _ => Err("Expected List value for Multimodal".to_string()),
        }
    }
}

pub type MultimodalBasic = Multimodal<MultimodalBasicType>;

impl Multimodal<MultimodalBasicType> {
    /// Create a Multimodal input data for agent functions with basic types: Text and Binary.
    ///
    /// # Example
    /// ```
    /// use golem_rust::agentic::{Multimodal, MultimodalBasicType, UnstructuredText, UnstructuredBinary, MultimodalBasic};
    /// use golem_rust::MultimodalSchema;
    ///
    /// // Create a multimodal dataset with text and binary inputs
    /// let multimodal_data = Multimodal::new_basic([
    ///     MultimodalBasicType::Text(UnstructuredText::from_inline_any("foo".to_string())),
    ///     MultimodalBasicType::Binary(UnstructuredBinary::from_inline(vec![1, 2, 3], "image/png".to_string()))
    /// ]);
    ///
    /// // Function that shows how an agent might receive multimodal input
    /// fn my_agent_method(input: MultimodalBasic) {
    ///     // handle the multimodal input here
    /// }
    ///
    /// my_agent_method(multimodal_data);
    /// ```
    pub fn new_basic<I>(items: I) -> Self
    where
        I: IntoIterator<Item = MultimodalBasicType>,
    {
        Self {
            items: items.into_iter().collect(),
        }
    }
}

pub enum MultimodalBasicType {
    Text(UnstructuredText),
    Binary(UnstructuredBinary<String>),
}

impl MultimodalSchema for MultimodalBasicType {
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)> {
        vec![
            ("Text".to_string(), <UnstructuredText>::get_type()),
            (
                "Binary".to_string(),
                UnstructuredBinary::<String>::get_type(),
            ),
        ]
    }

    fn get_name(&self) -> String {
        match self {
            MultimodalBasicType::Text(_) => "Text".to_string(),
            MultimodalBasicType::Binary(_) => "Binary".to_string(),
        }
    }

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized,
    {
        match self {
            MultimodalBasicType::Text(text) => {
                let elem_value = text.to_element_value()?;
                Ok(("Text".to_string(), elem_value))
            }
            MultimodalBasicType::Binary(binary) => {
                let elem_value = binary.to_element_value()?;
                Ok(("Binary".to_string(), elem_value))
            }
        }
    }

    fn from_element_value(elem: (String, ElementValue)) -> Result<Self, String>
    where
        Self: Sized,
    {
        let (name, value) = elem;

        match name.as_str() {
            "Text" => {
                let schema = <UnstructuredText>::get_type();
                let text = UnstructuredText::from_element_value(value, schema)?;
                Ok(MultimodalBasicType::Text(text))
            }
            "Binary" => {
                let schema = <UnstructuredBinary<String>>::get_type();
                let binary = UnstructuredBinary::<String>::from_element_value(value, schema)?;
                Ok(MultimodalBasicType::Binary(binary))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }

    fn from_wit_value(wit_value: (String, WitValue)) -> Result<Self, String>
    where
        Self: Sized,
    {
        let (name, value) = wit_value;

        match name.as_str() {
            "Text" => {
                let text = UnstructuredText::from_wit_value(value)?;
                Ok(MultimodalBasicType::Text(text))
            }
            "Binary" => {
                let binary = UnstructuredBinary::<String>::from_wit_value(value)?;
                Ok(MultimodalBasicType::Binary(binary))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        match self {
            MultimodalBasicType::Text(text) => text.to_wit_value(),
            MultimodalBasicType::Binary(binary) => binary.to_wit_value(),
        }
    }
}
