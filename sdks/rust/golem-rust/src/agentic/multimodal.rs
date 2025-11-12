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

use crate::agentic::MultimodalSchema;
use crate::golem_agentic::golem::agent::common::{DataValue, ElementSchema};

/// Represents Multimodal input data for agent functions.
/// Note that you cannot mix a multimodal input with other input types
///
/// # Example
///
/// ```
/// use golem_rust::agentic::{Multimodal, MultiModalInput};
///
/// #[derive(MultiModalSchema)]
/// enum Input {
///     Text(String),
///     Image(Vec<u8>),
/// }
///
/// use MyMultimodalInput::*;
///
/// // Create a multimodal dataset with text and image inputs
/// let multimodal_data = Multimodal::new([
///     Text("my_text".into()),
///     Image(vec![1, 2, 3]),
///     Text("another_text".into()),
///     Image(vec![4, 5, 6]),
/// ]);
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
pub struct MultiModal<T> {
    pub items: Vec<T>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: MultimodalSchema> MultiModal<T> {
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            items: items.into_iter().collect(),
            _marker: std::marker::PhantomData,
        }
    }

    pub fn get_schema() -> Vec<(String, ElementSchema)> {
        T::get_multimodal_schema()
    }

    // With Multimodal schema we get name and element schema
    pub fn serialize(self) -> Result<DataValue, String> {
        let items = self.items;

        let mut elements = Vec::new();

        for item in items {
            let serialized = <T as MultimodalSchema>::to_element_value(item)?;
            elements.push(serialized);
        }

        Ok(DataValue::Multimodal(elements))
    }

    pub fn deserialize(data: DataValue) -> Result<Self, String> {
        match data {
            DataValue::Multimodal(elements) => {
                let mut items = Vec::new();

                for elem in elements {
                    let item = <T as MultimodalSchema>::from_element_value(elem)?;
                    items.push(item);
                }

                Ok(MultiModal {
                    items,
                    _marker: std::marker::PhantomData,
                })
            }
            _ => Err("Expected Multimodal DataValue".to_string()),
        }
    }
}
