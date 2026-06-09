// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Conversions between the canonical `golem-common` agent model and the
//! untyped/structural representations used at the service and storage seams.
//!
//! The WIT-side conversions (model <-> `golem:agent` bindings) now live with
//! the single agent bindgen in [`crate::schema::agent::wit`]; this file keeps
//! only the model-internal conversions and the `golem_wasm` core-type bridges
//! that other (non-agent) consumers still depend on.

use crate::model::agent::{
    BinaryDescriptor, BinaryReference, BinaryReferenceValue, BinarySource, BinaryType,
    ComponentModelElementValue, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementValue, NamedElementValues, TextDescriptor, TextReference, TextReferenceValue,
    TextSource, TextType, UnstructuredBinaryElementValue, UnstructuredTextElementValue,
    UntypedDataValue, UntypedElementValue, UntypedNamedElementValue,
};
use golem_wasm::ValueAndType;
use golem_wasm::analysis::AnalysedType;

impl DataValue {
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
                        .enumerate()
                        .map(|(idx, (value, schema))| {
                            ElementValue::try_from_untyped(value.value, schema.schema).map(|v| {
                                NamedElementValue {
                                    name: value.name,
                                    value: v,
                                    schema_index: idx as u32,
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

impl From<ElementValue> for UntypedElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                UntypedElementValue::ComponentModel(value.value)
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
                UntypedElementValue::UnstructuredText(TextReferenceValue { value })
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
                UntypedElementValue::UnstructuredBinary(BinaryReferenceValue { value })
            }
        }
    }
}

impl From<NamedElementValue> for UntypedNamedElementValue {
    fn from(value: NamedElementValue) -> Self {
        UntypedNamedElementValue {
            name: value.name,
            value: value.value.into(),
        }
    }
}

impl From<DataValue> for UntypedDataValue {
    fn from(value: DataValue) -> Self {
        match value {
            DataValue::Tuple(elements) => {
                UntypedDataValue::Tuple(elements.elements.into_iter().map(Into::into).collect())
            }
            DataValue::Multimodal(elements) => UntypedDataValue::Multimodal(
                elements.elements.into_iter().map(Into::into).collect(),
            ),
        }
    }
}

impl ElementValue {
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
                Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                    value: ValueAndType::new(value, typ),
                }))
            }
            (
                UntypedElementValue::UnstructuredText(text_ref),
                ElementSchema::UnstructuredText(descriptor),
            ) => Ok(ElementValue::UnstructuredText(
                UnstructuredTextElementValue {
                    value: text_ref.value,
                    descriptor,
                },
            )),
            (
                UntypedElementValue::UnstructuredBinary(binary_ref),
                ElementSchema::UnstructuredBinary(descriptor),
            ) => Ok(ElementValue::UnstructuredBinary(
                UnstructuredBinaryElementValue {
                    value: binary_ref.value,
                    descriptor,
                },
            )),
            _ => Err("Element value does not match schema".to_string()),
        }
    }
}

// -----------------------------------------------------------------------------
// Oplog seam: forward conversions from the canonical base-model agent value and
// schema types into the `golem:core@1.5.0` wire bindings used by the public
// oplog WIT (`golem:api/oplog`). These are required because the oplog WIT still
// references `golem:core/types@1.5.0.{data-value, data-schema}` while the agent
// WIT itself has been cut over to `golem:core/types@2.0.0`.
// -----------------------------------------------------------------------------

use golem_wasm::golem_core_1_5_x::types as gc15;

impl From<DataSchema> for gc15::DataSchema {
    fn from(value: DataSchema) -> Self {
        match value {
            DataSchema::Tuple(tuple) => gc15::DataSchema::Tuple(
                tuple
                    .elements
                    .into_iter()
                    .map(|named| (named.name, named.schema.into()))
                    .collect(),
            ),
            DataSchema::Multimodal(multimodal) => gc15::DataSchema::Multimodal(
                multimodal
                    .elements
                    .into_iter()
                    .map(|named| (named.name, named.schema.into()))
                    .collect(),
            ),
        }
    }
}

impl From<DataValue> for gc15::DataValue {
    fn from(value: DataValue) -> Self {
        match value {
            DataValue::Tuple(tuple) => gc15::DataValue::Tuple(
                tuple.elements.into_iter().map(ElementValue::into).collect(),
            ),
            DataValue::Multimodal(multimodal) => gc15::DataValue::Multimodal(
                multimodal
                    .elements
                    .into_iter()
                    .map(|v| (v.name, ElementValue::into(v.value)))
                    .collect(),
            ),
        }
    }
}

impl From<ElementSchema> for gc15::ElementSchema {
    fn from(value: ElementSchema) -> Self {
        match value {
            ElementSchema::ComponentModel(component_model_element_schema) => {
                gc15::ElementSchema::ComponentModel(
                    component_model_element_schema.element_type.into(),
                )
            }
            ElementSchema::UnstructuredText(text) => {
                gc15::ElementSchema::UnstructuredText(text.into())
            }
            ElementSchema::UnstructuredBinary(binary) => {
                gc15::ElementSchema::UnstructuredBinary(binary.into())
            }
        }
    }
}

impl From<ElementValue> for gc15::ElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                gc15::ElementValue::ComponentModel(value.into())
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
                gc15::ElementValue::UnstructuredText(value.into())
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
                gc15::ElementValue::UnstructuredBinary(value.into())
            }
        }
    }
}

impl From<BinaryDescriptor> for gc15::BinaryDescriptor {
    fn from(value: BinaryDescriptor) -> Self {
        Self {
            restrictions: value
                .restrictions
                .map(|r| r.into_iter().map(gc15::BinaryType::from).collect()),
        }
    }
}

impl From<BinaryReference> for gc15::BinaryReference {
    fn from(value: BinaryReference) -> Self {
        match value {
            BinaryReference::Url(url) => gc15::BinaryReference::Url(url.value),
            BinaryReference::Inline(source) => gc15::BinaryReference::Inline(source.into()),
        }
    }
}

impl From<BinarySource> for gc15::BinarySource {
    fn from(value: BinarySource) -> Self {
        Self {
            data: value.data,
            binary_type: value.binary_type.into(),
        }
    }
}

impl From<BinaryType> for gc15::BinaryType {
    fn from(value: BinaryType) -> Self {
        Self {
            mime_type: value.mime_type,
        }
    }
}

impl From<TextDescriptor> for gc15::TextDescriptor {
    fn from(value: TextDescriptor) -> Self {
        Self {
            restrictions: value
                .restrictions
                .map(|r| r.into_iter().map(gc15::TextType::from).collect()),
        }
    }
}

impl From<TextReference> for gc15::TextReference {
    fn from(value: TextReference) -> Self {
        match value {
            TextReference::Url(url) => gc15::TextReference::Url(url.value),
            TextReference::Inline(source) => gc15::TextReference::Inline(source.into()),
        }
    }
}

impl From<TextSource> for gc15::TextSource {
    fn from(value: TextSource) -> Self {
        Self {
            data: value.data,
            text_type: value.text_type.map(gc15::TextType::from),
        }
    }
}

impl From<TextType> for gc15::TextType {
    fn from(value: TextType) -> Self {
        Self {
            language_code: value.language_code,
        }
    }
}
