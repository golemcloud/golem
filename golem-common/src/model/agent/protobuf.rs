use crate::model::agent::{
    AgentConstructor, AgentDependency, AgentMethod, AgentType, BinaryDescriptor, BinaryType,
    DataSchema, ElementSchema, NamedElementSchema, NamedElementSchemas, TextDescriptor, TextType,
};
use golem_api_grpc::proto::golem::component::data_schema;
use golem_api_grpc::proto::golem::component::element_schema;

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
