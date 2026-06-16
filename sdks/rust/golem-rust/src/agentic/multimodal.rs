// Copyright 2024-2026 Golem Cloud
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

use crate::SchemaValue;
use crate::agentic::{
    MultimodalSchema, Schema, StructuredSchema, UnstructuredBinary, UnstructuredText,
};
use crate::schema::SchemaGraph;
use crate::schema::VariantValuePayload;

/// Represents Multimodal input data for agent functions.
/// Note that you cannot mix a multimodal input with other input types
///
/// # Example
///
/// ```
/// use golem_rust::agentic::{MultimodalAdvanced};
/// use golem_rust::MultimodalSchema;
///
/// // Create a multimodal dataset with text and image inputs
/// let multimodal_data = MultimodalAdvanced::new([
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
/// fn my_agent_method(input: MultimodalAdvanced<Input>) {
///     // handle the multimodal input here
/// }
///
/// my_agent_method(multimodal_data);
/// ```
///
/// The dynamic representation of this type would have variants corresponding to each variant of the `Input` enum,
/// and they are `text` and `image` holding `String` and `Vec<u8>` respectively.
///
/// # Notes
/// - Each variant of the `Input` enum represents a **modality**.
/// - The `Multimodal` type can take a variable number of such variants.
/// - If `Multimodal` is used in agents, their schema will reflect both
///   the **multimodal structure** and **type of each modality**.
/// - Unlike a plain `Vec<MultimodalInput>`, this type carries additional semantic and schema-level information
///   that indicates the data represents a *multimodal input* — not just a generic list.
///
pub struct MultimodalAdvanced<T> {
    pub items: Vec<T>,
}

impl<T: MultimodalSchema> MultimodalAdvanced<T> {
    /// Create a Multimodal input data for agent functions.
    /// Note that you cannot mix a multimodal input with other input types
    ///
    /// # Example
    ///
    /// ```
    /// use golem_rust::agentic::{MultimodalAdvanced};
    /// use golem_rust::MultimodalSchema;
    ///
    /// // Create a multimodal dataset with text and image inputs
    /// let multimodal_data = MultimodalAdvanced::new([
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
    /// fn my_agent_method(input: MultimodalAdvanced<Input>) {
    ///     // handle the multimodal input here
    /// }
    ///
    /// my_agent_method(multimodal_data);
    /// ```
    ///
    /// If you need a predefined basic multimodal type with text and binary data, you can use `Multimodal` .
    ///
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn get_schema() -> Vec<(String, SchemaGraph)> {
        T::get_multimodal_schema()
    }

    // With Multimodal schema we get name and schema value
    pub fn to_name_and_schema_values(self) -> Result<Vec<(String, SchemaValue)>, String> {
        let items = self.items;

        let mut elements = Vec::new();

        for item in items {
            let serialized = <T as MultimodalSchema>::to_schema_value(item)?;
            elements.push(serialized);
        }

        Ok(elements)
    }

    pub fn from_schema_values(
        elems: Vec<(String, SchemaValue)>,
    ) -> Result<MultimodalAdvanced<T>, String> {
        let mut items = Vec::new();

        for (name, value) in elems {
            let item = <T as MultimodalSchema>::from_schema_value(name, value)?;
            items.push(item);
        }

        Ok(MultimodalAdvanced { items })
    }

    pub fn convert_to_schema_value(self) -> Result<SchemaValue, String> {
        let schemas = T::get_multimodal_schema();
        let mut elements = Vec::new();
        for item in self.items {
            let name = <T as MultimodalSchema>::get_name(&item);
            let (value_name, element) = <T as MultimodalSchema>::to_schema_value(item)?;
            if value_name != name {
                return Err(format!(
                    "Multimodal item name mismatch: get_name returned '{name}', to_schema_value returned '{value_name}'"
                ));
            }
            let Some(case) = schemas
                .iter()
                .position(|(schema_name, _)| schema_name == &name)
            else {
                return Err(format!("Unknown multimodal item '{name}'"));
            };
            elements.push(SchemaValue::Variant(VariantValuePayload {
                case: case as u32,
                payload: Some(Box::new(element)),
            }));
        }

        Ok(SchemaValue::List { elements })
    }

    pub fn convert_from_schema_value(
        value: SchemaValue,
        case_names: Vec<String>,
    ) -> Result<Self, String> {
        match value {
            SchemaValue::List { elements } => {
                let mut items = Vec::new();
                for value in elements {
                    let SchemaValue::Variant(VariantValuePayload { case, payload }) = value else {
                        return Err(format!("Expected multimodal variant item, got {value:?}"));
                    };
                    let name = case_names
                        .get(case as usize)
                        .ok_or_else(|| format!("Unknown multimodal case index: {case}"))?;
                    let payload = payload
                        .ok_or_else(|| format!("Missing payload for multimodal item '{name}'"))?;
                    let item = <T as MultimodalSchema>::from_schema_value(name.clone(), *payload)?;
                    items.push(item);
                }
                Ok(MultimodalAdvanced { items })
            }
            other => Err(format!("Expected Multimodal list, got {other:?}")),
        }
    }
}

impl<T: MultimodalSchema> Schema for MultimodalAdvanced<T> {
    fn get_type() -> StructuredSchema {
        StructuredSchema::Default(crate::agentic::multimodal_schema_graph(
            &T::get_multimodal_schema(),
        ))
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        self.convert_to_schema_value()
    }

    fn from_schema_value(value: SchemaValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match schema {
            StructuredSchema::Default(schema) => Self::convert_from_schema_value(
                value,
                multimodal_case_names_from_schema_graph(&schema)
                    .ok_or_else(|| "Expected Multimodal schema".to_string())?,
            ),
            _ => Err("Expected Multimodal schema".to_string()),
        }
    }
}

fn multimodal_case_names_from_schema_graph(schema: &SchemaGraph) -> Option<Vec<String>> {
    if schema.root.metadata().role.as_ref() != Some(&crate::schema::Role::Multimodal) {
        return None;
    }

    let crate::schema::SchemaType::List { element, .. } = &schema.root else {
        return None;
    };
    let crate::schema::SchemaType::Variant { cases, .. } = element.as_ref() else {
        return None;
    };

    Some(cases.iter().map(|case| case.name.clone()).collect())
}

pub struct Multimodal {
    value: MultimodalAdvanced<BasicModality>,
}

impl Multimodal {
    /// Create a Multimodal input data for agent functions with basic types: Text and Binary.
    ///
    /// # Example
    /// ```
    /// use golem_rust::agentic::*;
    /// use golem_rust::MultimodalSchema;
    ///
    /// // Create a multimodal dataset with text and binary inputs
    /// let multimodal_data = Multimodal::new([
    ///     BasicModality::text("foo".to_string()),
    ///     BasicModality::binary(vec![1, 2, 3], "image/png")
    /// ]);
    ///
    /// // Function that shows how an agent might receive multimodal input
    /// fn my_agent_method(input: Multimodal) {
    ///     // handle the multimodal input here
    /// }
    ///
    /// my_agent_method(multimodal_data);
    /// ```
    ///
    /// The dynamic representation of this type would have two variants: "text" and "binary",
    /// holding `UnstructuredText` and `UnstructuredBinary` respectively.
    ///
    /// If you need a user defined type along with these two variants, you can use `MultimodalCustom<T>` where `T` is your custom type.
    ///
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = BasicModality>,
    {
        let advanced = MultimodalAdvanced::new(items);

        Multimodal { value: advanced }
    }

    pub fn items(&self) -> &Vec<BasicModality> {
        &self.value.items
    }
}

impl Schema for Multimodal {
    fn get_type() -> StructuredSchema {
        MultimodalAdvanced::<BasicModality>::get_type()
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        self.value.to_schema_value()
    }

    fn from_schema_value(value: SchemaValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced = MultimodalAdvanced::<BasicModality>::from_schema_value(value, schema)?;
        Ok(Multimodal { value: advanced })
    }
}

pub enum BasicModality {
    Text(UnstructuredText),
    Binary(UnstructuredBinary<String>),
}

impl BasicModality {
    pub fn text(text: String) -> BasicModality {
        BasicModality::Text(UnstructuredText::from_inline_any(text))
    }

    pub fn binary<MT: ToString>(data: Vec<u8>, mime_type: MT) -> BasicModality {
        BasicModality::Binary(UnstructuredBinary::from_inline(data, mime_type.to_string()))
    }
}

impl MultimodalSchema for BasicModality {
    fn get_multimodal_schema() -> Vec<(String, SchemaGraph)> {
        vec![
            (
                "Text".to_string(),
                <UnstructuredText>::get_type()
                    .get_schema_graph()
                    .expect("internal error: unable to get schema graph for UnstructuredText"),
            ),
            (
                "Binary".to_string(),
                UnstructuredBinary::<String>::get_type()
                    .get_schema_graph()
                    .expect("internal error: unable to get schema graph for UnstructuredBinary"),
            ),
        ]
    }

    fn get_name(&self) -> String {
        match self {
            BasicModality::Text(_) => "Text".to_string(),
            BasicModality::Binary(_) => "Binary".to_string(),
        }
    }

    fn to_schema_value(self) -> Result<(String, SchemaValue), String>
    where
        Self: Sized,
    {
        match self {
            BasicModality::Text(text) => Ok(("Text".to_string(), text.to_schema_value()?)),
            BasicModality::Binary(binary) => Ok(("Binary".to_string(), binary.to_schema_value()?)),
        }
    }

    fn from_schema_value(name: String, value: SchemaValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        match name.as_str() {
            "Text" => {
                let text =
                    UnstructuredText::from_schema_value(value, <UnstructuredText>::get_type())?;
                Ok(BasicModality::Text(text))
            }
            "Binary" => {
                let binary = UnstructuredBinary::<String>::from_schema_value(
                    value,
                    <UnstructuredBinary<String>>::get_type(),
                )?;
                Ok(BasicModality::Binary(binary))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }
}

pub struct MultimodalCustom<T: Schema> {
    value: MultimodalAdvanced<CustomModality<T>>,
}

impl<T: Schema> MultimodalCustom<T> {
    /// Create a Multimodal input data for agent functions with basic types: Text and Binary.
    ///
    /// # Example
    /// ```ignore
    /// use golem_rust::agentic::*;
    /// use golem_rust::{FromSchema, IntoSchema, MultimodalSchema};
    ///
    /// // Define a custom type
    /// #[derive(IntoSchema, FromSchema)]
    /// struct MyCustomType {
    ///   x: String,
    ///   y: i32,
    /// }
    ///
    /// // Create a multimodal dataset with text, binary and custom inputs
    /// let multimodal_data: MultimodalCustom<MyCustomType> = MultimodalCustom::new([
    ///     CustomModality::text("foo".to_string()),
    ///     CustomModality::binary(vec![1, 2, 3], "image/png"),
    ///     CustomModality::Custom(MyCustomType { x: "bar".to_string(), y: 42 }),
    /// ]);
    /// // Function that shows how an agent might receive multimodal input
    /// fn my_agent_method(input: MultimodalCustom<MyCustomType>) {
    ///     // handle the multimodal input here
    /// }
    /// my_agent_method(multimodal_data);
    /// ```
    /// The dynamic representation of this type would have three variants: "text", "binary" and "custom"
    /// holding `UnstructuredText`, `UnstructuredBinary`, `CustomType` respectively.
    ///
    pub fn new<I>(items: I) -> Self
    where
        I: IntoIterator<Item = CustomModality<T>>,
    {
        MultimodalCustom {
            value: MultimodalAdvanced::new(items),
        }
    }

    pub fn items(&self) -> &Vec<CustomModality<T>> {
        &self.value.items
    }
}

impl<T: Schema> Schema for MultimodalCustom<T> {
    fn get_type() -> StructuredSchema {
        MultimodalAdvanced::<CustomModality<T>>::get_type()
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        self.value.to_schema_value()
    }

    fn from_schema_value(value: SchemaValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced = MultimodalAdvanced::<CustomModality<T>>::from_schema_value(value, schema)?;

        Ok(MultimodalCustom { value: advanced })
    }
}

pub enum CustomModality<T: Schema> {
    Basic(BasicModality),
    Custom(T),
}

impl<T: Schema> CustomModality<T> {
    pub fn text(text: String) -> CustomModality<T> {
        CustomModality::Basic(BasicModality::text(text))
    }

    pub fn binary<MT: ToString>(data: Vec<u8>, mime_type: MT) -> CustomModality<T> {
        CustomModality::Basic(BasicModality::binary(data, mime_type.to_string()))
    }

    pub fn custom(value: T) -> CustomModality<T> {
        CustomModality::Custom(value)
    }
}

impl<T: Schema> MultimodalSchema for CustomModality<T> {
    fn get_multimodal_schema() -> Vec<(String, SchemaGraph)> {
        let mut schema = BasicModality::get_multimodal_schema();

        schema.push((
            "Custom".to_string(),
            T::get_type()
                .get_schema_graph()
                .expect("internal error: unable to get schema graph for Custom modality"),
        ));
        schema
    }

    fn get_name(&self) -> String {
        match self {
            CustomModality::Basic(basic) => basic.get_name(),
            CustomModality::Custom(_) => "Custom".to_string(),
        }
    }

    fn to_schema_value(self) -> Result<(String, SchemaValue), String>
    where
        Self: Sized,
    {
        match self {
            CustomModality::Basic(basic) => basic.to_schema_value(),
            CustomModality::Custom(custom) => Ok(("Custom".to_string(), custom.to_schema_value()?)),
        }
    }

    fn from_schema_value(name: String, value: SchemaValue) -> Result<Self, String>
    where
        Self: Sized,
    {
        match name.as_str() {
            "Text" | "Binary" => {
                let basic = BasicModality::from_schema_value(name, value)?;
                Ok(CustomModality::Basic(basic))
            }
            "Custom" => {
                let custom = T::from_schema_value(value, T::get_type())?;
                Ok(CustomModality::Custom(custom))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }
}
