use super::{
    AgentConstructor, AgentDependency, AgentHttpAuthDetails, AgentInvocationMode, AgentMethod,
    AgentMode, AgentPrincipal, AgentType, AgentTypeName, BinaryDescriptor, BinaryReference,
    BinaryReferenceValue, BinarySource, BinaryType, ComponentModelElementSchema,
    ComponentModelElementValue, CorsOptions, CustomHttpMethod, DataSchema, DataValue,
    ElementSchema, ElementValue, ElementValues, GolemUserPrincipal, HeaderVariable,
    HttpEndpointDetails, HttpMethod, HttpMountDetails, LiteralSegment, NamedElementSchema,
    NamedElementSchemas, NamedElementValue, NamedElementValues, OidcPrincipal, PathSegment,
    PathVariable, Principal, QueryVariable, RegisteredAgentType, RegisteredAgentTypeImplementer,
    Snapshotting, SnapshottingConfig, SnapshottingEveryNInvocation, SnapshottingPeriodic,
    SystemVariable, SystemVariableSegment, TextDescriptor, TextReference, TextReferenceValue,
    TextSource, TextType, UnstructuredBinaryElementValue, UnstructuredTextElementValue,
    UntypedDataValue, UntypedElementValue, UntypedNamedElementValue, Url,
};
use crate::model::Empty;
use golem_api_grpc::proto::golem::component::data_schema;
use golem_api_grpc::proto::golem::component::element_schema;
use golem_api_grpc::proto::golem::component::{
    binary_reference, data_value, element_value, text_reference, untyped_data_value,
    untyped_element_value,
};

impl From<golem_api_grpc::proto::golem::component::AgentMode> for AgentMode {
    fn from(value: golem_api_grpc::proto::golem::component::AgentMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::component::AgentMode::Durable => AgentMode::Durable,
            golem_api_grpc::proto::golem::component::AgentMode::Ephemeral => AgentMode::Ephemeral,
        }
    }
}

impl From<AgentMode> for golem_api_grpc::proto::golem::component::AgentMode {
    fn from(value: AgentMode) -> Self {
        match value {
            AgentMode::Durable => golem_api_grpc::proto::golem::component::AgentMode::Durable,
            AgentMode::Ephemeral => golem_api_grpc::proto::golem::component::AgentMode::Ephemeral,
        }
    }
}

// worker_service proto AgentInvocationMode conversions

impl From<golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode> for AgentInvocationMode {
    fn from(value: golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode::Await => {
                AgentInvocationMode::Await
            }
            golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode::Schedule => {
                AgentInvocationMode::Schedule
            }
        }
    }
}

impl From<AgentInvocationMode> for golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode {
    fn from(value: AgentInvocationMode) -> Self {
        match value {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode::Await
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::v1::AgentInvocationMode::Schedule
            }
        }
    }
}

// workerexecutor proto AgentInvocationMode conversions

impl From<golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode>
    for AgentInvocationMode
{
    fn from(value: golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode) -> Self {
        match value {
            golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Await => {
                AgentInvocationMode::Await
            }
            golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Schedule => {
                AgentInvocationMode::Schedule
            }
        }
    }
}

impl From<AgentInvocationMode>
    for golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode
{
    fn from(value: AgentInvocationMode) -> Self {
        match value {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Await
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::workerexecutor::v1::AgentInvocationMode::Schedule
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentType> for AgentType {
    type Error = String;

    fn try_from(
        proto: golem_api_grpc::proto::golem::component::AgentType,
    ) -> Result<Self, Self::Error> {
        Ok(AgentType {
            mode: proto.mode().into(),
            type_name: AgentTypeName(proto.type_name),
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
            http_mount: proto.http_mount.map(TryInto::try_into).transpose()?,
            snapshotting: proto
                .snapshotting
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or(Snapshotting::Disabled(Empty {})),
        })
    }
}

impl From<AgentType> for golem_api_grpc::proto::golem::component::AgentType {
    fn from(value: AgentType) -> Self {
        golem_api_grpc::proto::golem::component::AgentType {
            mode: golem_api_grpc::proto::golem::component::AgentMode::from(value.mode) as i32,
            type_name: value.type_name.0,
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
            http_mount: value.http_mount.map(Into::into),
            snapshotting: Some(value.snapshotting.into()),
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
            http_endpoint: value
                .http_endpoint
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
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
            http_endpoint: value.http_endpoint.into_iter().map(Into::into).collect(),
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
                    Ok(ElementSchema::ComponentModel(ComponentModelElementSchema {
                        element_type: (&wit_type).try_into()?,
                    }))
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
            ElementSchema::ComponentModel(component_model_element_schema) => {
                golem_api_grpc::proto::golem::component::ElementSchema {
                    schema: Some(element_schema::Schema::ComponentModel(
                        (&component_model_element_schema.element_type).into(),
                    )),
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
                    Ok(ElementValue::ComponentModel(ComponentModelElementValue {
                        value: val.try_into()?,
                    }))
                }
                element_value::Value::UnstructuredText(text_ref) => Ok(
                    ElementValue::UnstructuredText(UnstructuredTextElementValue {
                        value: text_ref.try_into()?,
                        descriptor: TextDescriptor::default(),
                    }),
                ),
                element_value::Value::UnstructuredBinary(bin_ref) => Ok(
                    ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue {
                        value: bin_ref.try_into()?,
                        descriptor: BinaryDescriptor::default(),
                    }),
                ),
            },
        }
    }
}

impl From<ElementValue> for golem_api_grpc::proto::golem::component::ElementValue {
    fn from(value: ElementValue) -> Self {
        match value {
            ElementValue::ComponentModel(ComponentModelElementValue { value }) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::ComponentModel(value.into())),
                }
            }
            ElementValue::UnstructuredText(UnstructuredTextElementValue { value, .. }) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::UnstructuredText(value.into())),
                }
            }
            ElementValue::UnstructuredBinary(UnstructuredBinaryElementValue { value, .. }) => {
                golem_api_grpc::proto::golem::component::ElementValue {
                    value: Some(element_value::Value::UnstructuredBinary(value.into())),
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

impl TryFrom<golem_api_grpc::proto::golem::component::UntypedElementValue> for UntypedElementValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::UntypedElementValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Missing field: value".to_string()),
            Some(v) => match v {
                untyped_element_value::Value::ComponentModel(val) => {
                    Ok(UntypedElementValue::ComponentModel(val.try_into()?))
                }
                untyped_element_value::Value::UnstructuredText(text_ref) => {
                    Ok(UntypedElementValue::UnstructuredText(TextReferenceValue {
                        value: text_ref.try_into()?,
                    }))
                }
                untyped_element_value::Value::UnstructuredBinary(bin_ref) => Ok(
                    UntypedElementValue::UnstructuredBinary(BinaryReferenceValue {
                        value: bin_ref.try_into()?,
                    }),
                ),
            },
        }
    }
}

impl From<UntypedElementValue> for golem_api_grpc::proto::golem::component::UntypedElementValue {
    fn from(value: UntypedElementValue) -> Self {
        match value {
            UntypedElementValue::ComponentModel(val) => {
                golem_api_grpc::proto::golem::component::UntypedElementValue {
                    value: Some(untyped_element_value::Value::ComponentModel(val.into())),
                }
            }
            UntypedElementValue::UnstructuredText(text_ref) => {
                golem_api_grpc::proto::golem::component::UntypedElementValue {
                    value: Some(untyped_element_value::Value::UnstructuredText(
                        text_ref.value.into(),
                    )),
                }
            }
            UntypedElementValue::UnstructuredBinary(bin_ref) => {
                golem_api_grpc::proto::golem::component::UntypedElementValue {
                    value: Some(untyped_element_value::Value::UnstructuredBinary(
                        bin_ref.value.into(),
                    )),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::UntypedNamedElementValue>
    for UntypedNamedElementValue
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::UntypedNamedElementValue,
    ) -> Result<Self, Self::Error> {
        Ok(UntypedNamedElementValue {
            name: value.name,
            value: value
                .value
                .ok_or_else(|| "Missing field: value".to_string())?
                .try_into()?,
        })
    }
}

impl From<UntypedNamedElementValue>
    for golem_api_grpc::proto::golem::component::UntypedNamedElementValue
{
    fn from(value: UntypedNamedElementValue) -> Self {
        golem_api_grpc::proto::golem::component::UntypedNamedElementValue {
            name: value.name,
            value: Some(value.value.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::UntypedDataValue> for UntypedDataValue {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::UntypedDataValue,
    ) -> Result<Self, Self::Error> {
        match value.value {
            None => Err("Missing field: value".to_string()),
            Some(v) => match v {
                untyped_data_value::Value::Tuple(tuple) => Ok(UntypedDataValue::Tuple(
                    tuple
                        .elements
                        .into_iter()
                        .map(UntypedElementValue::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                )),
                untyped_data_value::Value::Multimodal(mm) => Ok(UntypedDataValue::Multimodal(
                    mm.elements
                        .into_iter()
                        .map(UntypedNamedElementValue::try_from)
                        .collect::<Result<Vec<_>, _>>()?,
                )),
            },
        }
    }
}

impl From<UntypedDataValue> for golem_api_grpc::proto::golem::component::UntypedDataValue {
    fn from(value: UntypedDataValue) -> Self {
        match value {
            UntypedDataValue::Tuple(elements) => {
                golem_api_grpc::proto::golem::component::UntypedDataValue {
                    value: Some(untyped_data_value::Value::Tuple(
                        golem_api_grpc::proto::golem::component::UntypedTupleValue {
                            elements: elements
                                .into_iter()
                                .map(
                                    golem_api_grpc::proto::golem::component::UntypedElementValue::from,
                                )
                                .collect(),
                        },
                    )),
                }
            }
            UntypedDataValue::Multimodal(elements) => {
                golem_api_grpc::proto::golem::component::UntypedDataValue {
                    value: Some(untyped_data_value::Value::Multimodal(
                        golem_api_grpc::proto::golem::component::UntypedMultimodalValue {
                            elements: elements
                                .into_iter()
                                .map(
                                    golem_api_grpc::proto::golem::component::UntypedNamedElementValue::from,
                                )
                                .collect(),
                        },
                    )),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer>
    for RegisteredAgentTypeImplementer
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer,
    ) -> Result<Self, Self::Error> {
        Ok(RegisteredAgentTypeImplementer {
            component_id: value
                .component_id
                .ok_or_else(|| "Missing component_id field".to_string())?
                .try_into()?,
            component_revision: value.component_revision.try_into()?,
        })
    }
}

impl From<RegisteredAgentTypeImplementer>
    for golem_api_grpc::proto::golem::registry::RegisteredAgentTypeImplementer
{
    fn from(value: RegisteredAgentTypeImplementer) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            component_revision: value.component_revision.into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentType> for RegisteredAgentType {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentType,
    ) -> Result<Self, Self::Error> {
        Ok(RegisteredAgentType {
            agent_type: value
                .agent_type
                .ok_or_else(|| "Missing agent_type field".to_string())?
                .try_into()?,
            implemented_by: value
                .implemented_by
                .ok_or_else(|| "Missing implemented_by field".to_string())?
                .try_into()?,
        })
    }
}

impl From<RegisteredAgentType> for golem_api_grpc::proto::golem::registry::RegisteredAgentType {
    fn from(value: RegisteredAgentType) -> Self {
        Self {
            agent_type: Some(value.agent_type.into()),
            implemented_by: Some(value.implemented_by.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpMountDetails> for HttpMountDetails {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpMountDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            path_prefix: value
                .path_prefix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            auth_details: value.auth_details.map(TryInto::try_into).transpose()?,
            phantom_agent: value.phantom_agent,
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
            webhook_suffix: value
                .webhook_suffix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<HttpMountDetails> for golem_api_grpc::proto::golem::component::HttpMountDetails {
    fn from(value: HttpMountDetails) -> Self {
        Self {
            path_prefix: value.path_prefix.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            phantom_agent: value.phantom_agent,
            cors_options: Some(value.cors_options.into()),
            webhook_suffix: value.webhook_suffix.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpEndpointDetails> for HttpEndpointDetails {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpEndpointDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            http_method: value
                .http_method
                .ok_or_else(|| "Missing field: http_method".to_string())?
                .try_into()?,
            path_suffix: value
                .path_suffix
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            header_vars: value
                .header_vars
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            query_vars: value
                .query_vars
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            auth_details: value.auth_details.map(TryInto::try_into).transpose()?,
            cors_options: value
                .cors_options
                .ok_or_else(|| "Missing field: cors_options".to_string())?
                .try_into()?,
        })
    }
}

impl From<HttpEndpointDetails> for golem_api_grpc::proto::golem::component::HttpEndpointDetails {
    fn from(value: HttpEndpointDetails) -> Self {
        Self {
            http_method: Some(value.http_method.into()),
            path_suffix: value.path_suffix.into_iter().map(Into::into).collect(),
            header_vars: value.header_vars.into_iter().map(Into::into).collect(),
            query_vars: value.query_vars.into_iter().map(Into::into).collect(),
            auth_details: value.auth_details.map(Into::into),
            cors_options: Some(value.cors_options.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HttpMethod> for HttpMethod {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HttpMethod,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::http_method::Value;
        use golem_api_grpc::proto::golem::component::StandardHttpMethod;

        match value
            .value
            .ok_or_else(|| "Missing oneof: value".to_string())?
        {
            Value::Standard(inner) => {
                let typed =
                    golem_api_grpc::proto::golem::component::StandardHttpMethod::try_from(inner)
                        .unwrap_or_default();
                match typed {
                    StandardHttpMethod::Get => Ok(Self::Get(Empty {})),
                    StandardHttpMethod::Head => Ok(Self::Head(Empty {})),
                    StandardHttpMethod::Post => Ok(Self::Post(Empty {})),
                    StandardHttpMethod::Put => Ok(Self::Put(Empty {})),
                    StandardHttpMethod::Delete => Ok(Self::Delete(Empty {})),
                    StandardHttpMethod::Connect => Ok(Self::Connect(Empty {})),
                    StandardHttpMethod::Options => Ok(Self::Options(Empty {})),
                    StandardHttpMethod::Trace => Ok(Self::Trace(Empty {})),
                    StandardHttpMethod::Patch => Ok(Self::Patch(Empty {})),
                    StandardHttpMethod::Unspecified => {
                        Err("Unknown http method variant".to_string())
                    }
                }
            }
            Value::Custom(c) => Ok(HttpMethod::Custom(CustomHttpMethod { value: c })),
        }
    }
}

impl From<HttpMethod> for golem_api_grpc::proto::golem::component::HttpMethod {
    fn from(value: HttpMethod) -> Self {
        use golem_api_grpc::proto::golem::component::http_method::Value;
        use golem_api_grpc::proto::golem::component::StandardHttpMethod;

        Self {
            value: Some(match value {
                HttpMethod::Get(_) => Value::Standard(StandardHttpMethod::Get.into()),
                HttpMethod::Head(_) => Value::Standard(StandardHttpMethod::Head.into()),
                HttpMethod::Post(_) => Value::Standard(StandardHttpMethod::Post.into()),
                HttpMethod::Put(_) => Value::Standard(StandardHttpMethod::Put.into()),
                HttpMethod::Delete(_) => Value::Standard(StandardHttpMethod::Delete.into()),
                HttpMethod::Connect(_) => Value::Standard(StandardHttpMethod::Connect.into()),
                HttpMethod::Options(_) => Value::Standard(StandardHttpMethod::Options.into()),
                HttpMethod::Trace(_) => Value::Standard(StandardHttpMethod::Trace.into()),
                HttpMethod::Patch(_) => Value::Standard(StandardHttpMethod::Patch.into()),
                HttpMethod::Custom(c) => Value::Custom(c.value),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::CorsOptions> for CorsOptions {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::CorsOptions,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            allowed_patterns: value.allowed_patterns,
        })
    }
}

impl From<CorsOptions> for golem_api_grpc::proto::golem::component::CorsOptions {
    fn from(value: CorsOptions) -> Self {
        Self {
            allowed_patterns: value.allowed_patterns,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PathSegment> for PathSegment {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PathSegment,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::path_segment::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Literal(v) => Ok(Self::Literal(v.try_into()?)),
            Value::SystemVariable(v) => Ok(Self::SystemVariable(v.try_into()?)),
            Value::PathVariable(v) => Ok(Self::PathVariable(v.try_into()?)),
            Value::RemainingPathVariable(v) => Ok(Self::RemainingPathVariable(v.try_into()?)),
        }
    }
}

impl From<PathSegment> for golem_api_grpc::proto::golem::component::PathSegment {
    fn from(value: PathSegment) -> Self {
        use golem_api_grpc::proto::golem::component::path_segment::Value;

        Self {
            value: Some(match value {
                PathSegment::Literal(v) => Value::Literal(v.into()),
                PathSegment::SystemVariable(v) => Value::SystemVariable(v.into()),
                PathSegment::PathVariable(v) => Value::PathVariable(v.into()),
                PathSegment::RemainingPathVariable(v) => Value::RemainingPathVariable(v.into()),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::LiteralSegment> for LiteralSegment {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::LiteralSegment,
    ) -> Result<Self, Self::Error> {
        Ok(Self { value: value.value })
    }
}

impl From<LiteralSegment> for golem_api_grpc::proto::golem::component::LiteralSegment {
    fn from(value: LiteralSegment) -> Self {
        Self { value: value.value }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SystemVariableSegment>
    for SystemVariableSegment
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SystemVariableSegment,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            value: value.value().try_into()?,
        })
    }
}

impl From<SystemVariableSegment>
    for golem_api_grpc::proto::golem::component::SystemVariableSegment
{
    fn from(value: SystemVariableSegment) -> Self {
        Self {
            value: golem_api_grpc::proto::golem::component::SystemVariable::from(value.value)
                .into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SystemVariable> for SystemVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SystemVariable,
    ) -> Result<Self, Self::Error> {
        match value {
            golem_api_grpc::proto::golem::component::SystemVariable::AgentType => {
                Ok(Self::AgentType)
            }
            golem_api_grpc::proto::golem::component::SystemVariable::AgentVersion => {
                Ok(Self::AgentVersion)
            }
            golem_api_grpc::proto::golem::component::SystemVariable::Unspecified => {
                Err("Unknown SystemVariable variant".to_string())
            }
        }
    }
}

impl From<SystemVariable> for golem_api_grpc::proto::golem::component::SystemVariable {
    fn from(value: SystemVariable) -> Self {
        match value {
            SystemVariable::AgentType => Self::AgentType,
            SystemVariable::AgentVersion => Self::AgentVersion,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::PathVariable> for PathVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::PathVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            variable_name: value.variable_name,
        })
    }
}

impl From<PathVariable> for golem_api_grpc::proto::golem::component::PathVariable {
    fn from(value: PathVariable) -> Self {
        Self {
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::HeaderVariable> for HeaderVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::HeaderVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        })
    }
}

impl From<HeaderVariable> for golem_api_grpc::proto::golem::component::HeaderVariable {
    fn from(value: HeaderVariable) -> Self {
        Self {
            header_name: value.header_name,
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::QueryVariable> for QueryVariable {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::QueryVariable,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        })
    }
}

impl From<QueryVariable> for golem_api_grpc::proto::golem::component::QueryVariable {
    fn from(value: QueryVariable) -> Self {
        Self {
            query_param_name: value.query_param_name,
            variable_name: value.variable_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentHttpAuthDetails>
    for AgentHttpAuthDetails
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentHttpAuthDetails,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            required: value.required,
        })
    }
}

impl From<AgentHttpAuthDetails> for golem_api_grpc::proto::golem::component::AgentHttpAuthDetails {
    fn from(value: AgentHttpAuthDetails) -> Self {
        Self {
            required: value.required,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Principal> for Principal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Principal,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::principal::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Oidc(v) => Ok(Self::Oidc(v.try_into()?)),
            Value::Agent(v) => Ok(Self::Agent(v.try_into()?)),
            Value::GolemUser(v) => Ok(Self::GolemUser(v.try_into()?)),
            Value::Anonymous(_) => Ok(Self::Anonymous(Empty {})),
        }
    }
}

impl From<Principal> for golem_api_grpc::proto::golem::component::Principal {
    fn from(value: Principal) -> Self {
        use golem_api_grpc::proto::golem::component::principal::Value;

        Self {
            value: Some(match value {
                Principal::Oidc(v) => Value::Oidc(v.into()),
                Principal::Agent(v) => Value::Agent(v.into()),
                Principal::GolemUser(v) => Value::GolemUser(v.into()),
                Principal::Anonymous(_) => {
                    Value::Anonymous(golem_api_grpc::proto::golem::common::Empty {})
                }
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::OidcPrincipal> for OidcPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::OidcPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        })
    }
}

impl From<OidcPrincipal> for golem_api_grpc::proto::golem::component::OidcPrincipal {
    fn from(value: OidcPrincipal) -> Self {
        Self {
            sub: value.sub,
            issuer: value.issuer,
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: value.claims,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::AgentPrincipal> for AgentPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::AgentPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_id: value
                .agent_id
                .ok_or_else(|| "Missing field: agent_id".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentPrincipal> for golem_api_grpc::proto::golem::component::AgentPrincipal {
    fn from(value: AgentPrincipal) -> Self {
        Self {
            agent_id: Some(value.agent_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::GolemUserPrincipal> for GolemUserPrincipal {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::GolemUserPrincipal,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            account_id: value
                .account_id
                .ok_or_else(|| "Missing field: account_id".to_string())?
                .try_into()?,
        })
    }
}

impl From<GolemUserPrincipal> for golem_api_grpc::proto::golem::component::GolemUserPrincipal {
    fn from(value: GolemUserPrincipal) -> Self {
        Self {
            account_id: Some(value.account_id.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::Snapshotting> for Snapshotting {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::Snapshotting,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::snapshotting::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Disabled(_) => Ok(Self::Disabled(Empty {})),
            Value::Enabled(config) => Ok(Self::Enabled(config.try_into()?)),
        }
    }
}

impl From<Snapshotting> for golem_api_grpc::proto::golem::component::Snapshotting {
    fn from(value: Snapshotting) -> Self {
        use golem_api_grpc::proto::golem::component::snapshotting::Value;

        Self {
            value: Some(match value {
                Snapshotting::Disabled(_) => {
                    Value::Disabled(golem_api_grpc::proto::golem::common::Empty {})
                }
                Snapshotting::Enabled(config) => Value::Enabled(config.into()),
            }),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::SnapshottingConfig> for SnapshottingConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::SnapshottingConfig,
    ) -> Result<Self, Self::Error> {
        use golem_api_grpc::proto::golem::component::snapshotting_config::Value;

        match value
            .value
            .ok_or_else(|| "Missing field: value".to_string())?
        {
            Value::Default(_) => Ok(Self::Default(Empty {})),
            Value::PeriodicNanos(nanos) => Ok(Self::Periodic(SnapshottingPeriodic {
                duration_nanos: nanos,
            })),
            Value::EveryNInvocation(n) => {
                Ok(Self::EveryNInvocation(SnapshottingEveryNInvocation {
                    count: n as u16,
                }))
            }
        }
    }
}

impl From<SnapshottingConfig> for golem_api_grpc::proto::golem::component::SnapshottingConfig {
    fn from(value: SnapshottingConfig) -> Self {
        use golem_api_grpc::proto::golem::component::snapshotting_config::Value;

        Self {
            value: Some(match value {
                SnapshottingConfig::Default(_) => {
                    Value::Default(golem_api_grpc::proto::golem::common::Empty {})
                }
                SnapshottingConfig::Periodic(periodic) => {
                    Value::PeriodicNanos(periodic.duration_nanos)
                }
                SnapshottingConfig::EveryNInvocation(every_n) => {
                    Value::EveryNInvocation(every_n.count as u32)
                }
            }),
        }
    }
}
