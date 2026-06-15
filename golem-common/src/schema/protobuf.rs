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

//! Conversions between common agent schema types and their protobuf mirror.

use crate::base_model::agent::{AgentConfigSource, AgentTypeName, Snapshotting};
use crate::model::Empty as ModelEmpty;
use crate::schema::agent::{
    AgentConfigDeclarationSchema, AgentConstructorSchema, AgentDependencySchema, AgentMethodSchema,
    AgentTypeSchema, AutoInjectedKind, FieldSource, InputSchema, NamedField, OutputSchema,
    RegisteredAgentTypeSchema,
};
use crate::schema::graph::SchemaGraph;
use crate::schema::metadata::MetadataEnvelope;
use golem_api_grpc::proto::golem::common::Empty as ProtoEmpty;
use golem_api_grpc::proto::golem::schema as proto;

fn optional_meta(meta: MetadataEnvelope) -> Option<proto::MetadataEnvelope> {
    if meta.is_empty() {
        None
    } else {
        Some(meta.into())
    }
}

fn meta_from_proto(meta: Option<proto::MetadataEnvelope>) -> Result<MetadataEnvelope, String> {
    match meta {
        None => Ok(MetadataEnvelope::default()),
        Some(m) => m.try_into(),
    }
}

impl From<InputSchema> for proto::InputSchema {
    fn from(value: InputSchema) -> Self {
        match value {
            InputSchema::Parameters(fields) => Self {
                parameters: fields.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl TryFrom<proto::InputSchema> for InputSchema {
    type Error = String;

    fn try_from(value: proto::InputSchema) -> Result<Self, Self::Error> {
        Ok(InputSchema::Parameters(
            value
                .parameters
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        ))
    }
}

impl From<OutputSchema> for proto::OutputSchema {
    fn from(value: OutputSchema) -> Self {
        use proto::output_schema::Output;
        let output = match value {
            OutputSchema::Unit => Output::Unit(ProtoEmpty {}),
            OutputSchema::Single(ty) => Output::Single((*ty).into()),
        };
        Self {
            output: Some(output),
        }
    }
}

impl TryFrom<proto::OutputSchema> for OutputSchema {
    type Error = String;

    fn try_from(value: proto::OutputSchema) -> Result<Self, Self::Error> {
        use proto::output_schema::Output;
        match value.output {
            Some(Output::Unit(_)) => Ok(OutputSchema::Unit),
            Some(Output::Single(ty)) => Ok(OutputSchema::Single(Box::new(ty.try_into()?))),
            None => Err("Missing field: OutputSchema.output".to_string()),
        }
    }
}

impl From<NamedField> for proto::NamedField {
    fn from(value: NamedField) -> Self {
        Self {
            name: value.name,
            source: Some(value.source.into()),
            schema: Some(value.schema.into()),
            metadata: optional_meta(value.metadata),
        }
    }
}

impl TryFrom<proto::NamedField> for NamedField {
    type Error = String;

    fn try_from(value: proto::NamedField) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            source: value
                .source
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_default(),
            schema: value
                .schema
                .ok_or_else(|| "Missing field: NamedField.schema".to_string())?
                .try_into()?,
            metadata: meta_from_proto(value.metadata)?,
        })
    }
}

impl From<FieldSource> for proto::FieldSource {
    fn from(value: FieldSource) -> Self {
        use proto::field_source::Source;
        let source = match value {
            FieldSource::UserSupplied => Source::UserSupplied(ProtoEmpty {}),
            FieldSource::AutoInjected(kind) => {
                Source::AutoInjected(proto::AutoInjectedKind::from(kind) as i32)
            }
        };
        Self {
            source: Some(source),
        }
    }
}

impl TryFrom<proto::FieldSource> for FieldSource {
    type Error = String;

    fn try_from(value: proto::FieldSource) -> Result<Self, Self::Error> {
        use proto::field_source::Source;
        match value.source {
            Some(Source::UserSupplied(_)) => Ok(FieldSource::UserSupplied),
            Some(Source::AutoInjected(kind)) => {
                let kind = proto::AutoInjectedKind::try_from(kind)
                    .map_err(|_| format!("Invalid AutoInjectedKind: {kind}"))?;
                Ok(FieldSource::AutoInjected(kind.try_into()?))
            }
            None => Err("Missing field: FieldSource.source".to_string()),
        }
    }
}

impl From<AutoInjectedKind> for proto::AutoInjectedKind {
    fn from(value: AutoInjectedKind) -> Self {
        match value {
            AutoInjectedKind::Principal => proto::AutoInjectedKind::Principal,
        }
    }
}

impl TryFrom<proto::AutoInjectedKind> for AutoInjectedKind {
    type Error = String;

    fn try_from(value: proto::AutoInjectedKind) -> Result<Self, Self::Error> {
        match value {
            proto::AutoInjectedKind::Principal => Ok(AutoInjectedKind::Principal),
            proto::AutoInjectedKind::Unspecified => Err("Unspecified AutoInjectedKind".to_string()),
        }
    }
}

// --- agent type layer --------------------------------------------------------

impl From<AgentConstructorSchema> for proto::AgentConstructorSchema {
    fn from(value: AgentConstructorSchema) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
        }
    }
}

impl TryFrom<proto::AgentConstructorSchema> for AgentConstructorSchema {
    type Error = String;

    fn try_from(value: proto::AgentConstructorSchema) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: AgentConstructorSchema.input_schema".to_string())?
                .try_into()?,
        })
    }
}

impl From<AgentMethodSchema> for proto::AgentMethodSchema {
    fn from(value: AgentMethodSchema) -> Self {
        Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: Some(value.input_schema.into()),
            output_schema: Some(value.output_schema.into()),
            http_endpoint: value.http_endpoint.into_iter().map(Into::into).collect(),
            read_only: value.read_only.map(Into::into),
        }
    }
}

impl TryFrom<proto::AgentMethodSchema> for AgentMethodSchema {
    type Error = String;

    fn try_from(value: proto::AgentMethodSchema) -> Result<Self, Self::Error> {
        Ok(Self {
            name: value.name,
            description: value.description,
            prompt_hint: value.prompt_hint,
            input_schema: value
                .input_schema
                .ok_or_else(|| "Missing field: AgentMethodSchema.input_schema".to_string())?
                .try_into()?,
            output_schema: value
                .output_schema
                .ok_or_else(|| "Missing field: AgentMethodSchema.output_schema".to_string())?
                .try_into()?,
            http_endpoint: value
                .http_endpoint
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            read_only: value.read_only.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<AgentDependencySchema> for proto::AgentDependencySchema {
    fn from(value: AgentDependencySchema) -> Self {
        Self {
            type_name: value.type_name,
            description: value.description,
            schema: Some(value.schema.into()),
            constructor: Some(value.constructor.into()),
            methods: value.methods.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::AgentDependencySchema> for AgentDependencySchema {
    type Error = String;

    fn try_from(value: proto::AgentDependencySchema) -> Result<Self, Self::Error> {
        Ok(Self {
            type_name: value.type_name,
            description: value.description,
            schema: value
                .schema
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_else(SchemaGraph::empty),
            constructor: value
                .constructor
                .ok_or_else(|| "Missing field: AgentDependencySchema.constructor".to_string())?
                .try_into()?,
            methods: value
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<AgentTypeSchema> for proto::AgentTypeSchema {
    fn from(value: AgentTypeSchema) -> Self {
        Self {
            type_name: value.type_name.0,
            description: value.description,
            source_language: value.source_language,
            schema: Some(value.schema.into()),
            constructor: Some(value.constructor.into()),
            methods: value.methods.into_iter().map(Into::into).collect(),
            dependencies: value.dependencies.into_iter().map(Into::into).collect(),
            mode: golem_api_grpc::proto::golem::component::AgentMode::from(value.mode) as i32,
            http_mount: value.http_mount.map(Into::into),
            snapshotting: Some(value.snapshotting.into()),
            config: value.config.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<proto::AgentTypeSchema> for AgentTypeSchema {
    type Error = String;

    fn try_from(value: proto::AgentTypeSchema) -> Result<Self, Self::Error> {
        let mode = value.mode().into();
        Ok(Self {
            type_name: AgentTypeName(value.type_name),
            description: value.description,
            source_language: value.source_language,
            schema: value
                .schema
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or_else(SchemaGraph::empty),
            constructor: value
                .constructor
                .ok_or_else(|| "Missing field: AgentTypeSchema.constructor".to_string())?
                .try_into()?,
            methods: value
                .methods
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            dependencies: value
                .dependencies
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            mode,
            http_mount: value.http_mount.map(TryInto::try_into).transpose()?,
            snapshotting: value
                .snapshotting
                .map(TryInto::try_into)
                .transpose()?
                .unwrap_or(Snapshotting::Disabled(ModelEmpty {})),
            config: value
                .config
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        })
    }
}

impl From<AgentConfigDeclarationSchema> for proto::AgentConfigDeclarationSchema {
    fn from(value: AgentConfigDeclarationSchema) -> Self {
        Self {
            source: golem_api_grpc::proto::golem::component::AgentConfigSource::from(value.source)
                as i32,
            path: value.path,
            value_type: Some(value.value_type.into()),
        }
    }
}

impl TryFrom<proto::AgentConfigDeclarationSchema> for AgentConfigDeclarationSchema {
    type Error = String;

    fn try_from(value: proto::AgentConfigDeclarationSchema) -> Result<Self, Self::Error> {
        let source = AgentConfigSource::try_from(value.source())?;
        Ok(Self {
            source,
            path: value.path,
            value_type: value
                .value_type
                .ok_or_else(|| {
                    "Missing field: AgentConfigDeclarationSchema.value_type".to_string()
                })?
                .try_into()?,
        })
    }
}

// --- registered agent type schema (golem.registry package) ------------------

impl From<RegisteredAgentTypeSchema>
    for golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema
{
    fn from(value: RegisteredAgentTypeSchema) -> Self {
        Self {
            agent_type: Some(value.agent_type.into()),
            implemented_by: Some(value.implemented_by.into()),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema>
    for RegisteredAgentTypeSchema
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::registry::RegisteredAgentTypeSchema,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_type: value
                .agent_type
                .ok_or_else(|| "Missing field: RegisteredAgentTypeSchema.agent_type".to_string())?
                .try_into()?,
            implemented_by: value
                .implemented_by
                .ok_or_else(|| {
                    "Missing field: RegisteredAgentTypeSchema.implemented_by".to_string()
                })?
                .try_into()?,
        })
    }
}
