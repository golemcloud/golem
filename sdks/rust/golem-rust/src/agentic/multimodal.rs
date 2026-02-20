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

use crate::agentic::{
    MultimodalSchema, Schema, StructuredSchema, StructuredValue, UnstructuredBinary,
    UnstructuredText,
};
use crate::golem_agentic::golem::agent::common::{DataValue, ElementSchema, ElementValue};
use golem_wasm::{Value, WitValue};

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

    pub fn get_schema() -> Vec<(String, ElementSchema)> {
        T::get_multimodal_schema()
    }

    // With Multimodal schema we get name and element schema
    pub fn to_name_and_element_values(self) -> Result<Vec<(String, ElementValue)>, String> {
        let items = self.items;

        let mut elements = Vec::new();

        for item in items {
            let serialized = <T as MultimodalSchema>::to_element_value(item)?;
            elements.push(serialized);
        }

        Ok(elements)
    }

    pub fn from_data_value(data: DataValue) -> Result<Self, String> {
        match data {
            DataValue::Multimodal(elements) => Self::from_element_values(elements),
            _ => Err("Expected Multimodal DataValue".to_string()),
        }
    }

    pub fn from_element_values(
        elems: Vec<(String, ElementValue)>,
    ) -> Result<MultimodalAdvanced<T>, String> {
        let mut items = Vec::new();

        for elem in elems {
            let item = <T as MultimodalSchema>::from_element_value(elem)?;
            items.push(item);
        }

        Ok(MultimodalAdvanced { items })
    }

    pub fn convert_to_wit_value(self) -> Result<WitValue, String> {
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

                Ok(MultimodalAdvanced { items })
            }
            _ => Err("Expected List value for Multimodal".to_string()),
        }
    }
}

impl<T: MultimodalSchema> Schema for MultimodalAdvanced<T> {
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        StructuredSchema::Multimodal(T::get_multimodal_schema())
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        let data_value = self.to_name_and_element_values()?;
        Ok(StructuredValue::Multimodal(data_value))
    }

    fn from_structured_value(
        value: StructuredValue,
        _schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            StructuredValue::Multimodal(elements) => Self::from_element_values(elements),
            _ => Err("Expected Multimodal StructuredValue".to_string()),
        }
    }

    fn from_wit_value(wit_value: WitValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        match schema {
            StructuredSchema::Multimodal(_) => Self::from_wit_value(wit_value),
            _ => Err("Expected Multimodal StructuredSchema".to_string()),
        }
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        self.convert_to_wit_value()
    }
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
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        MultimodalAdvanced::<BasicModality>::get_type()
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        self.value.to_structured_value()
    }

    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced = MultimodalAdvanced::<BasicModality>::from_structured_value(value, schema)?;
        Ok(Multimodal { value: advanced })
    }

    fn from_wit_value(wit_value: WitValue, _schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced = MultimodalAdvanced::<BasicModality>::from_wit_value(wit_value)?;

        Ok(Multimodal { value: advanced })
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        self.value.to_wit_value()
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
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)> {
        vec![
            (
                "Text".to_string(),
                <UnstructuredText>::get_type()
                    .get_element_schema()
                    .expect("internal error: unable to get element schema for UnstructuredText"),
            ),
            (
                "Binary".to_string(),
                UnstructuredBinary::<String>::get_type()
                    .get_element_schema()
                    .expect("internal error: unable to get element schema for UnstructuredBinary"),
            ),
        ]
    }

    fn get_name(&self) -> String {
        match self {
            BasicModality::Text(_) => "Text".to_string(),
            BasicModality::Binary(_) => "Binary".to_string(),
        }
    }

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized,
    {
        match self {
            BasicModality::Text(text) => {
                let elem_value = text.to_structured_value()?;
                Ok((
                    "Text".to_string(),
                    elem_value
                        .get_element_value()
                        .expect("internal error: unable to get element value for Text"),
                ))
            }
            BasicModality::Binary(binary) => {
                let elem_value = binary.to_structured_value()?;
                Ok((
                    "Binary".to_string(),
                    elem_value
                        .get_element_value()
                        .expect("internal error: unable to get element value for Binary"),
                ))
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
                let text = UnstructuredText::from_structured_value(
                    StructuredValue::Default(value),
                    schema,
                )?;
                Ok(BasicModality::Text(text))
            }
            "Binary" => {
                let schema = <UnstructuredBinary<String>>::get_type();
                let binary = UnstructuredBinary::<String>::from_structured_value(
                    StructuredValue::Default(value),
                    schema,
                )?;
                Ok(BasicModality::Binary(binary))
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
                Ok(BasicModality::Text(text))
            }
            "Binary" => {
                let binary = UnstructuredBinary::<String>::from_wit_value(value)?;
                Ok(BasicModality::Binary(binary))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        match self {
            BasicModality::Text(text) => text.to_wit_value(),
            BasicModality::Binary(binary) => binary.to_wit_value(),
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
    /// use golem_rust::MultimodalSchema;
    /// use golem_rust::Schema;
    ///
    /// // Define a custom type
    /// #[derive(Schema)]
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
    const IS_COMPONENT_MODEL_SCHEMA: bool = false;

    fn get_type() -> StructuredSchema {
        MultimodalAdvanced::<CustomModality<T>>::get_type()
    }

    fn to_structured_value(self) -> Result<StructuredValue, String> {
        self.value.to_structured_value()
    }

    fn from_structured_value(
        value: StructuredValue,
        schema: StructuredSchema,
    ) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced =
            MultimodalAdvanced::<CustomModality<T>>::from_structured_value(value, schema)?;

        Ok(MultimodalCustom { value: advanced })
    }

    fn from_wit_value(wit_value: WitValue, _schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized,
    {
        let advanced = MultimodalAdvanced::<CustomModality<T>>::from_wit_value(wit_value)?;

        Ok(MultimodalCustom { value: advanced })
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        self.value.to_wit_value()
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
    fn get_multimodal_schema() -> Vec<(String, ElementSchema)> {
        let mut schema = BasicModality::get_multimodal_schema();

        schema.push((
            "Custom".to_string(),
            T::get_type()
                .get_element_schema()
                .expect("internal error: unable to get element schema for Custom modality"),
        ));
        schema
    }

    fn get_name(&self) -> String {
        match self {
            CustomModality::Basic(basic) => basic.get_name(),
            CustomModality::Custom(_) => "Custom".to_string(),
        }
    }

    fn to_element_value(self) -> Result<(String, ElementValue), String>
    where
        Self: Sized,
    {
        match self {
            CustomModality::Basic(basic) => basic.to_element_value(),
            CustomModality::Custom(custom) => {
                let elem_value = custom.to_structured_value()?;
                Ok((
                    "Custom".to_string(),
                    elem_value
                        .get_element_value()
                        .expect("internal error: unable to get element value for Custom modality"),
                ))
            }
        }
    }

    fn from_element_value(elem: (String, ElementValue)) -> Result<Self, String>
    where
        Self: Sized,
    {
        let (name, value) = elem;

        match name.as_str() {
            "Text" | "Binary" => {
                let basic = BasicModality::from_element_value((name, value))?;
                Ok(CustomModality::Basic(basic))
            }
            "Custom" => {
                let schema = T::get_type();
                let custom = T::from_structured_value(StructuredValue::Default(value), schema)?;
                Ok(CustomModality::Custom(custom))
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
            "Text" | "Binary" => {
                let basic = BasicModality::from_wit_value((name, value))?;
                Ok(CustomModality::Basic(basic))
            }
            "Custom" => {
                let schema = T::get_type();
                let custom = T::from_wit_value(value, schema)?;
                Ok(CustomModality::Custom(custom))
            }
            _ => Err(format!("Unknown modality name: {}", name)),
        }
    }

    fn to_wit_value(self) -> Result<WitValue, String>
    where
        Self: Sized,
    {
        match self {
            CustomModality::Basic(basic) => basic.to_wit_value(),
            CustomModality::Custom(custom) => custom.to_wit_value(),
        }
    }
}
