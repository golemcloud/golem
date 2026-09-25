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
use crate::schema::wit::{direct, wire};

#[doc(hidden)]
pub trait MultimodalWire {
    fn append_modality_cases(builder: &mut direct::WireSchemaBuilder)
    -> Vec<wire::VariantCaseType>;

    fn contains_stream(_: &mut std::collections::HashSet<&'static str>) -> bool {
        false
    }
}

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

impl<T: direct::IntoWire> direct::IntoWire for MultimodalAdvanced<T> {
    fn preflight(&self, resources: &mut direct::WirePreflight) -> Result<(), direct::WireError> {
        self.items.preflight(resources)
    }

    async fn prepare_wire(&self) -> Result<(), direct::WireError> {
        self.items.prepare_wire().await
    }

    fn write_wire(
        &self,
        writer: &mut direct::WireWriter,
    ) -> Result<wire::ValueNodeIndex, direct::WireError> {
        self.items.write_wire(writer)
    }
}

impl<T: direct::FromWire> direct::FromWire for MultimodalAdvanced<T> {
    fn read_wire(
        reader: &mut direct::WireReader,
        index: wire::ValueNodeIndex,
    ) -> Result<Self, direct::WireError> {
        Ok(Self {
            items: Vec::<T>::read_wire(reader, index)?,
        })
    }
}

impl<T: MultimodalWire> direct::WireSchema for MultimodalAdvanced<T> {
    fn contains_stream(seen: &mut std::collections::HashSet<&'static str>) -> bool {
        T::contains_stream(seen)
    }

    fn append_schema(builder: &mut direct::WireSchemaBuilder) -> wire::TypeNodeIndex {
        let cases = T::append_modality_cases(builder);
        let element = builder.push(wire::SchemaTypeBody::VariantType(cases));
        let mut metadata = direct::empty_metadata();
        metadata.role = Some(wire::Role::Multimodal);
        builder.push_with_metadata(wire::SchemaTypeBody::ListType(element), metadata)
    }
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

macro_rules! wire_wrapper {
    ($wrapper:ty, $inner:ty $(, $t:ident)?) => {
        impl<$($t: Schema + direct::IntoWire)?> direct::IntoWire for $wrapper {
            fn preflight(&self, resources: &mut direct::WirePreflight) -> Result<(), direct::WireError> {
                direct::IntoWire::preflight(&self.value, resources)
            }

            async fn prepare_wire(&self) -> Result<(), direct::WireError> {
                direct::IntoWire::prepare_wire(&self.value).await
            }

            fn write_wire(&self, writer: &mut direct::WireWriter) -> Result<wire::ValueNodeIndex, direct::WireError> {
                direct::IntoWire::write_wire(&self.value, writer)
            }
        }

        impl<$($t: Schema + direct::FromWire)?> direct::FromWire for $wrapper {
            fn read_wire(reader: &mut direct::WireReader, index: wire::ValueNodeIndex) -> Result<Self, direct::WireError> {
                Ok(Self { value: <$inner as direct::FromWire>::read_wire(reader, index)? })
            }
        }

        impl<$($t: Schema + direct::WireSchema)?> direct::WireSchema for $wrapper {
            fn contains_stream(seen: &mut std::collections::HashSet<&'static str>) -> bool {
                <$inner as direct::WireSchema>::contains_stream(seen)
            }

            fn append_schema(builder: &mut direct::WireSchemaBuilder) -> wire::TypeNodeIndex {
                <$inner as direct::WireSchema>::append_schema(builder)
            }
        }
    };
}

wire_wrapper!(Multimodal, MultimodalAdvanced<BasicModality>);
wire_wrapper!(
    MultimodalCustom<T>,
    MultimodalAdvanced<CustomModality<T>>,
    T
);

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

#[derive(crate::FromWire, crate::IntoWire)]
pub enum BasicModality {
    Text(UnstructuredText),
    Binary(UnstructuredBinary<String>),
}

impl MultimodalWire for BasicModality {
    fn append_modality_cases(
        builder: &mut direct::WireSchemaBuilder,
    ) -> Vec<wire::VariantCaseType> {
        vec![
            wire::VariantCaseType {
                name: "Text".to_string(),
                payload: Some(<UnstructuredText as direct::WireSchema>::append_schema(
                    builder,
                )),
                metadata: direct::empty_metadata(),
            },
            wire::VariantCaseType {
                name: "Binary".to_string(),
                payload: Some(
                    <UnstructuredBinary<String> as direct::WireSchema>::append_schema(builder),
                ),
                metadata: direct::empty_metadata(),
            },
        ]
    }
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

impl<T: Schema + direct::WireSchema> MultimodalWire for CustomModality<T> {
    fn contains_stream(seen: &mut std::collections::HashSet<&'static str>) -> bool {
        <T as direct::WireSchema>::contains_stream(seen)
    }

    fn append_modality_cases(
        builder: &mut direct::WireSchemaBuilder,
    ) -> Vec<wire::VariantCaseType> {
        let mut cases = BasicModality::append_modality_cases(builder);
        cases.push(wire::VariantCaseType {
            name: "Custom".to_string(),
            payload: Some(T::append_schema(builder)),
            metadata: direct::empty_metadata(),
        });
        cases
    }
}

impl<T: Schema + direct::IntoWire> direct::IntoWire for CustomModality<T> {
    fn preflight(&self, resources: &mut direct::WirePreflight) -> Result<(), direct::WireError> {
        match self {
            Self::Basic(value) => value.preflight(resources),
            Self::Custom(value) => value.preflight(resources),
        }
    }

    async fn prepare_wire(&self) -> Result<(), direct::WireError> {
        match self {
            Self::Basic(value) => value.prepare_wire().await,
            Self::Custom(value) => value.prepare_wire().await,
        }
    }

    fn write_wire(
        &self,
        writer: &mut direct::WireWriter,
    ) -> Result<wire::ValueNodeIndex, direct::WireError> {
        match self {
            Self::Basic(value) => value.write_wire(writer),
            Self::Custom(value) => {
                let payload = value.write_wire(writer)?;
                Ok(writer.push(wire::SchemaValueNode::VariantValue(
                    wire::VariantValuePayload {
                        case: 2,
                        payload: Some(payload),
                    },
                )))
            }
        }
    }
}

impl<T: Schema + direct::FromWire> direct::FromWire for CustomModality<T> {
    fn read_wire(
        reader: &mut direct::WireReader,
        index: wire::ValueNodeIndex,
    ) -> Result<Self, direct::WireError> {
        let wire::SchemaValueNode::VariantValue(variant) = reader.take(index)? else {
            return Err(direct::WireError::Shape("modality variant"));
        };
        let payload = variant
            .payload
            .ok_or(direct::WireError::Shape("modality payload"))?;
        match variant.case {
            0 => Ok(Self::Basic(BasicModality::Text(
                UnstructuredText::read_wire(reader, payload)?,
            ))),
            1 => Ok(Self::Basic(BasicModality::Binary(UnstructuredBinary::<
                String,
            >::read_wire(
                reader, payload
            )?))),
            2 => Ok(Self::Custom(T::read_wire(reader, payload)?)),
            _ => Err(direct::WireError::Shape("modality case")),
        }
    }
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
