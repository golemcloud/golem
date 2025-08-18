use crate::model::agent::{
    AgentConstructor, AgentDependency, AgentMethod, AgentType, BinaryDescriptor, BinaryReference,
    BinarySource, BinaryType, DataSchema, DataValue, ElementSchema, ElementValue, ElementValues,
    NamedElementSchema, NamedElementSchemas, NamedElementValue, NamedElementValues, TextDescriptor,
    TextReference, TextSource, TextType, Url,
};
use golem_api_grpc::proto::golem::component::data_schema;
use golem_api_grpc::proto::golem::component::element_schema;
use golem_api_grpc::proto::golem::component::{
    binary_reference, data_value, element_value, text_reference,
};

impl TryFrom<golem_api_grpc::proto::golem::component::AgentType> for AgentType {
    type Error = String;

    fn try_from(
        proto: golem_api_grpc::proto::golem::component::AgentType,
    ) -> Result<Self, Self::Error> {
        Ok(AgentType {
            type_name: proto.type_name,
            description: proto.description,
            constructor: proto
                .constructor
                .ok_or_else(|| "Missing field: constructor".to_string())?
                .try_into()?,
            methods: proto
                .methods
                .into_iter()
                .map(AgentMethod::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            dependencies: proto
                .dependencies
                .into_iter()
                .map(AgentDependency::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<AgentType> for golem_api_grpc::proto::golem::component::AgentType {
    fn from(value: AgentType) -> Self {
        golem_api_grpc::proto::golem::component::AgentType {
            type_name: value.type_name,
            description: value.description,
            constructor: Some(value.constructor.into()),
            methods: value
                .methods
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::AgentMethod::from)
                .collect(),
            dependencies: value
                .dependencies
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::AgentDependency::from)
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentConstructor> for AgentConstructor {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentConstructor,
    ) -> Result<Self, Self::Error> {
        Ok(AgentConstructor {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: input_schema".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentConstructor> for golem_api_grpc::proto::golem::component::AgentConstructor {
    fn from(value: AgentConstructor) -> Self {
        golem_api_grpc::proto::golem::component::AgentConstructor {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentMethod> for AgentMethod {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentMethod,
    ) -> Result<Self, Self::Error> {
        Ok(AgentMethod {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: input_schema".to_string())?
                .try_into()?,
            output_schema: value
                .output_schema
                .ok_or_else(|| "Missing field: output_schema".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentMethod> for golem_api_grpc::proto::golem::component::AgentMethod {
    fn from(value: AgentMethod) -> Self {
        golem_api_grpc::proto::golem::component::AgentMethod {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
            output_schema: Some(value.output_schema.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentDependency> for AgentDependency {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentDependency,
    ) -> Result<Self, Self::Error> {
        Ok(AgentDependency {
            type_name: value.type_name,
            description: value.description,
            constructor: value
                .constructor
                .ok_or_else(|| "Missing field: constructor".to_string())?
                .try_into()?,
            methods: value
                .methods
                .into_iter()
                .map(AgentMethod::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<AgentDependency> for golem_api_grpc::proto::golem::component::AgentDependency {
    fn from(value: AgentDependency) -> Self {
        golem_api_grpc::proto::golem::component::AgentDependency {
            type_name: value.type_name,
            description: value.description,
            constructor: Some(value.constructor.into()),
            methods: value
                .methods
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::AgentMethod::from)
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DataSchema> for DataSchema {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::DataSchema,
    ) -> Result<Self, Self::Error> {
        match value.schema {
            None => Err("Missing field: schema".to_string()),
            Some(schema) => match schema {
                data_schema::Schema::Tuple(tuple) => Ok(DataSchema::Tuple(NamedElementSchemas {
                    elements: tuple
                        .elements
                        .into_iter()
                        .map(NamedElementSchema::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                })),
                data_schema::Schema::Multimodal(multimodal) => {
                    Ok(DataSchema::Multimodal(NamedElementSchemas {
                        elements: multimodal
                            .elements
                            .into_iter()
                            .map(NamedElementSchema::try_from)
                            .collect::<Result<Vec<_>, _>>()?,
                    }))
                }
            },
        }
    }
}

impl From<DataSchema> for golem_api_grpc::proto::golem::component::DataSchema {
    fn from(value: DataSchema) -> Self {
        match value {
            DataSchema::Tuple(named_elements) => golem_api_grpc::proto::golem::component::DataSchema {
                schema: Some(data_schema::Schema::Tuple(
                    golem_api_grpc::proto::golem::component::TupleSchema {
                        elements: named_elements.elements.into_iter().map(golem_api_grpc::proto::golem::component::NamedElementSchema::from).collect(),
                    }
                )),
            },
            DataSchema::Multimodal(named_elements) => golem_api_grpc::proto::golem::component::DataSchema {
                schema: Some(data_schema::Schema::Multimodal(
                    golem_api_grpc::proto::golem::component::MultimodalSchema {
                        elements: named_elements.elements.into_iter().map(golem_api_grpc::proto::golem::component::NamedElementSchema::from).collect(),
                    }
                )),
            },
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::NamedElementSchema> for NamedElementSchema {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::NamedElementSchema,
    ) -> Result<Self, Self::Error> {
        Ok(NamedElementSchema {
            name: value.name,
            schema: value
                .schema
                .ok_or_else(|| "Missing field: schema".to_string())?
                .try_into()?,
        })
    }
}

impl From<NamedElementSchema> for golem_api_grpc::proto::golem::component::NamedElementSchema {
    fn from(value: NamedElementSchema) -> Self {
        golem_api_grpc::proto::golem::component::NamedElementSchema {
            name: value.name,
            schema: Some(value.schema.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ElementSchema> for ElementSchema {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ElementSchema,
    ) -> Result<Self, Self::Error> {
        match value.schema {
            None => Err("Missing field: schema".to_string()),
            Some(schema) => match schema {
                element_schema::Schema::ComponentModel(wit_type) => {
                    Ok(ElementSchema::ComponentModel((&wit_type).try_into()?))
                }
                element_schema::Schema::UnstructuredText(text_descriptor) => {
                    Ok(ElementSchema::UnstructuredText(text_descriptor.try_into()?))
                }
                element_schema::Schema::UnstructuredBinary(binary_descriptor) => Ok(
                    ElementSchema::UnstructuredBinary(binary_descriptor.try_into()?),
                ),
            },
        }
    }
}

impl From<ElementSchema> for golem_api_grpc::proto::golem::component::ElementSchema {
    fn from(value: ElementSchema) -> Self {
        match value {
            ElementSchema::ComponentModel(wit_type) => {
                golem_api_grpc::proto::golem::component::ElementSchema {
                    schema: Some(element_schema::Schema::ComponentModel((&wit_type).into())),
                }
            }
            ElementSchema::UnstructuredText(text_descriptor) => {
                golem_api_grpc::proto::golem::component::ElementSchema {
                    schema: Some(element_schema::Schema::UnstructuredText(
                        text_descriptor.into(),
                    )),
                }
            }
            ElementSchema::UnstructuredBinary(binary_descriptor) => {
                golem_api_grpc::proto::golem::component::ElementSchema {
                    schema: Some(element_schema::Schema::UnstructuredBinary(
                        binary_descriptor.into(),
                    )),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::TextDescriptor> for TextDescriptor {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::TextDescriptor,
    ) -> Result<Self, Self::Error> {
        let restrictions = value
            .restrictions
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TextDescriptor {
            restrictions: if restrictions.is_empty() {
                None
            } else {
                Some(restrictions)
            },
        })
    }
}

impl From<TextDescriptor> for golem_api_grpc::proto::golem::component::TextDescriptor {
    fn from(value: TextDescriptor) -> Self {
        golem_api_grpc::proto::golem::component::TextDescriptor {
            restrictions: value
                .restrictions
                .unwrap_or_default()
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::TextType::from)
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::TextType> for TextType {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::TextType,
    ) -> Result<Self, Self::Error> {
        Ok(TextType {
            language_code: value.language_code,
        })
    }
}

impl From<TextType> for golem_api_grpc::proto::golem::component::TextType {
    fn from(value: TextType) -> Self {
        golem_api_grpc::proto::golem::component::TextType {
            language_code: value.language_code,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::BinaryDescriptor> for BinaryDescriptor {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::BinaryDescriptor,
    ) -> Result<Self, Self::Error> {
        let restrictions = value
            .restrictions
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(BinaryDescriptor {
            restrictions: if restrictions.is_empty() {
                None
            } else {
                Some(restrictions)
            },
        })
    }
}

impl From<BinaryDescriptor> for golem_api_grpc::proto::golem::component::BinaryDescriptor {
    fn from(value: BinaryDescriptor) -> Self {
        golem_api_grpc::proto::golem::component::BinaryDescriptor {
            restrictions: value
                .restrictions
                .unwrap_or_default()
                .into_iter()
                .map(golem_api_grpc::proto::golem::component::BinaryType::from)
                .collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::BinaryType> for BinaryType {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::BinaryType,
    ) -> Result<Self, Self::Error> {
        Ok(BinaryType {
            mime_type: value.mime_type,
        })
    }
}

impl From<BinaryType> for golem_api_grpc::proto::golem::component::BinaryType {
    fn from(value: BinaryType) -> Self {
        golem_api_grpc::proto::golem::component::BinaryType {
            mime_type: value.mime_type,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::TextSource> for TextSource {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::TextSource,
    ) -> Result<Self, Self::Error> {
        Ok(TextSource {
            data: value.data,
            text_type: match value.text_type {
                None => None,
                Some(tt) => Some(tt.try_into()?),
            },
        })
    }
}

impl From<TextSource> for golem_api_grpc::proto::golem::component::TextSource {
    fn from(value: TextSource) -> Self {
        golem_api_grpc::proto::golem::component::TextSource {
            data: value.data,
            text_type: value.text_type.map(|tt| tt.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::TextReference> for TextReference {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::TextReference,
    ) -> Result<Self, Self::Error> {
        match value.text {
            None => Err("Missing field: text".to_string()),
            Some(text) => match text {
                text_reference::Text::Url(url) => Ok(TextReference::Url(Url { value: url })),
                text_reference::Text::Inline(inline) => {
                    Ok(TextReference::Inline(inline.try_into()?))
                }
            },
        }
    }
}

impl From<TextReference> for golem_api_grpc::proto::golem::component::TextReference {
    fn from(value: TextReference) -> Self {
        match value {
            TextReference::Url(url) => golem_api_grpc::proto::golem::component::TextReference {
                text: Some(text_reference::Text::Url(url.value)),
            },
            TextReference::Inline(inline) => {
                golem_api_grpc::proto::golem::component::TextReference {
                    text: Some(text_reference::Text::Inline(inline.into())),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::BinarySource> for BinarySource {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::BinarySource,
    ) -> Result<Self, Self::Error> {
        Ok(BinarySource {
            data: value.data,
            binary_type: value
                .binary_type
                .ok_or_else(|| "Missing field: binary_type".to_string())?
                .try_into()?,
        })
    }
}

impl From<BinarySource> for golem_api_grpc::proto::golem::component::BinarySource {
    fn from(value: BinarySource) -> Self {
        golem_api_grpc::proto::golem::component::BinarySource {
            data: value.data,
            binary_type: Some(value.binary_type.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::BinaryReference> for BinaryReference {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::BinaryReference,
    ) -> Result<Self, Self::Error> {
        match value.binary {
            None => Err("Missing field: binary".to_string()),
            Some(binary) => match binary {
                binary_reference::Binary::Url(url) => Ok(BinaryReference::Url(Url { value: url })),
                binary_reference::Binary::Inline(inline) => {
                    Ok(BinaryReference::Inline(inline.try_into()?))
                }
            },
        }
    }
}

impl From<BinaryReference> for golem_api_grpc::proto::golem::component::BinaryReference {
    fn from(value: BinaryReference) -> Self {
        match value {
            BinaryReference::Url(url) => golem_api_grpc::proto::golem::component::BinaryReference {
                binary: Some(binary_reference::Binary::Url(url.value)),
            },
            BinaryReference::Inline(inline) => {
                golem_api_grpc::proto::golem::component::BinaryReference {
                    binary: Some(binary_reference::Binary::Inline(inline.into())),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::ElementValue> for ElementValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::ElementValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Missing field: value".to_string()),
            Some(v) => match v {
                element_value::Value::ComponentModel(val) => {
                    Ok(ElementValue::ComponentModel(val.try_into()?))
                }
                element_value::Value::UnstructuredText(text_ref) => {
                    Ok(ElementValue::UnstructuredText(text_ref.try_into()?))
                }
                element_value::Value::UnstructuredBinary(bin_ref) => {
                    Ok(ElementValue::UnstructuredBinary(bin_ref.try_into()?))
                }
            },
        }
    }
}

impl From<ElementValue> for golem_api_grpc::proto::golem::component::ElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(val) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::ComponentModel(val.into())),
                }
            }
            ElementValue::UnstructuredText(text_ref) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::UnstructuredText(text_ref.into())),
                }
            }
            ElementValue::UnstructuredBinary(bin_ref) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::UnstructuredBinary(bin_ref.into())),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::NamedElementValue> for NamedElementValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::NamedElementValue,
    ) -> Result<Self, Self::Error> {
        Ok(NamedElementValue {
            name: value.name,
            value: value
                .value
                .ok_or_else(|| "Missing field: value".to_string())?
                .try_into()?,
        })
    }
}

impl From<NamedElementValue> for golem_api_grpc::proto::golem::component::NamedElementValue {
    fn from(value: NamedElementValue) -> Self {
        golem_api_grpc::proto::golem::component::NamedElementValue {
            name: value.name,
            value: Some(value.value.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::DataValue> for DataValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::DataValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Missing field: value".to_string()),
            Some(v) => match v {
                data_value::Value::Tuple(tuple) => Ok(DataValue::Tuple(ElementValues {
                    elements: tuple
                        .elements
                        .into_iter()
                        .map(ElementValue::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                })),
                data_value::Value::Multimodal(mm) => {
                    Ok(DataValue::Multimodal(NamedElementValues {
                        elements: mm
                            .elements
                            .into_iter()
                            .map(NamedElementValue::try_from)
                            .collect::<Result<Vec<_>, _>>()?,
                    }))
                }
            },
        }
    }
}

impl From<DataValue> for golem_api_grpc::proto::golem::component::DataValue {
    fn from(value: DataValue) -> Self {
        match value {
            DataValue::Tuple(elements) => golem_api_grpc::proto::golem::component::DataValue {
                value: Some(data_value::Value::Tuple(
                    golem_api_grpc::proto::golem::component::TupleValue {
                        elements: elements
                            .elements
                            .into_iter()
                            .map(golem_api_grpc::proto::golem::component::ElementValue::from)
                            .collect(),
                    },
                )),
            },
            DataValue::Multimodal(elements) => golem_api_grpc::proto::golem::component::DataValue {
                value: Some(data_value::Value::Multimodal(
                    golem_api_grpc::proto::golem::component::MultimodalValue {
                        elements: elements
                            .elements
                            .into_iter()
                            .map(golem_api_grpc::proto::golem::component::NamedElementValue::from)
                            .collect(),
                    },
                )),
            },
        }
    }
}
