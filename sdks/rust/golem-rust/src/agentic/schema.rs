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

use crate::golem_agentic::golem::agent::common::Principal;
use crate::schema::{
    BinaryRestrictions, BinaryValuePayload, FromSchema, IntoSchema, MetadataEnvelope, Role,
    SchemaGraph, SchemaType, SchemaValue, TextRestrictions, TextValuePayload, UrlRestrictions,
    VariantCaseType, VariantValuePayload,
};

pub trait Schema {
    fn get_type() -> StructuredSchema;
    fn to_schema_value(self) -> Result<SchemaValue, String>
    where
        Self: Sized;
    fn from_schema_value(value: SchemaValue, schema: StructuredSchema) -> Result<Self, String>
    where
        Self: Sized;

    fn from_principal(_principal: Principal) -> Result<Self, String>
    where
        Self: Sized,
    {
        Err("Principal can only be injected into Principal parameters".to_string())
    }
}

#[derive(Debug)]
pub enum StructuredSchema {
    AutoInject(AutoInjectedParamType),
    Default(SchemaGraph),
}

#[derive(Debug, Clone)]
pub enum AutoInjectedParamType {
    Principal,
}

impl StructuredSchema {
    pub fn get_schema_graph(self) -> Option<SchemaGraph> {
        match self {
            StructuredSchema::Default(schema) => Some(schema),
            StructuredSchema::AutoInject(_) => None,
        }
    }
}

impl Schema for Principal {
    fn get_type() -> StructuredSchema {
        StructuredSchema::AutoInject(AutoInjectedParamType::Principal)
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        Err("Principal is auto-injected and cannot be converted to SchemaValue".to_string())
    }

    fn from_schema_value(_value: SchemaValue, _schema: StructuredSchema) -> Result<Self, String> {
        Err("Principal is auto-injected and cannot be converted from SchemaValue".to_string())
    }

    fn from_principal(principal: Principal) -> Result<Self, String> {
        Ok(principal)
    }
}

impl<T: IntoSchema + FromSchema> Schema for T {
    fn get_type() -> StructuredSchema {
        StructuredSchema::Default(
            crate::schema::try_into_schema_graph::<T>()
                .expect("failed to build schema graph for agentic type"),
        )
    }

    fn to_schema_value(self) -> Result<SchemaValue, String> {
        Ok(self.to_value())
    }

    fn from_schema_value(value: SchemaValue, schema: StructuredSchema) -> Result<Self, String> {
        match schema {
            StructuredSchema::Default(_) => T::from_value(&value).map_err(|err| err.to_string()),
            _ => Err(format!("Expected ComponentModel schema, got: {schema:?}")),
        }
    }
}

mod host_api_schema {
    use super::*;
    use crate::bindings::golem::api::host;
    use crate::golem_schema::{AgentId, ComponentId, EnvironmentId, Uuid};
    use crate::schema::wit::wire;
    use crate::schema::{FromSchemaError, SchemaBuilder, SchemaType, TypeId};

    fn uuid_to_model(value: wire::Uuid) -> Uuid {
        Uuid::from_u64_pair(value.high_bits, value.low_bits)
    }

    fn uuid_from_model(value: Uuid) -> wire::Uuid {
        let (high_bits, low_bits) = value.as_u64_pair();
        wire::Uuid {
            high_bits,
            low_bits,
        }
    }

    fn component_id_to_model(value: wire::ComponentId) -> ComponentId {
        ComponentId {
            uuid: uuid_to_model(value.uuid),
        }
    }

    fn component_id_from_model(value: ComponentId) -> wire::ComponentId {
        wire::ComponentId {
            uuid: uuid_from_model(value.uuid),
        }
    }

    fn agent_id_to_model(value: wire::AgentId) -> AgentId {
        AgentId {
            component_id: component_id_to_model(value.component_id),
            agent_id: value.agent_id,
        }
    }

    fn agent_id_from_model(value: AgentId) -> wire::AgentId {
        wire::AgentId {
            component_id: component_id_from_model(value.component_id),
            agent_id: value.agent_id,
        }
    }

    fn environment_id_to_model(value: host::EnvironmentId) -> EnvironmentId {
        EnvironmentId {
            uuid: uuid_to_model(value.uuid),
        }
    }

    fn environment_id_from_model(value: EnvironmentId) -> host::EnvironmentId {
        host::EnvironmentId {
            uuid: uuid_from_model(value.uuid),
        }
    }

    macro_rules! impl_schema_conversion_via_model {
        ($ty:ty, $model:ty, $to_model:expr, $from_model:expr) => {
            impl IntoSchema for $ty {
                fn type_id() -> TypeId {
                    <$model>::type_id()
                }

                fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
                    <$model>::register_in(builder)
                }

                fn to_value(&self) -> SchemaValue {
                    ($to_model)(self.clone()).to_value()
                }
            }

            impl FromSchema for $ty {
                fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
                    <$model>::from_value(value).map($from_model)
                }
            }
        };
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    enum FilterComparatorModel {
        Equal,
        NotEqual,
        GreaterEqual,
        Greater,
        LessEqual,
        Less,
    }

    impl From<host::FilterComparator> for FilterComparatorModel {
        fn from(value: host::FilterComparator) -> Self {
            match value {
                host::FilterComparator::Equal => Self::Equal,
                host::FilterComparator::NotEqual => Self::NotEqual,
                host::FilterComparator::GreaterEqual => Self::GreaterEqual,
                host::FilterComparator::Greater => Self::Greater,
                host::FilterComparator::LessEqual => Self::LessEqual,
                host::FilterComparator::Less => Self::Less,
            }
        }
    }

    impl From<FilterComparatorModel> for host::FilterComparator {
        fn from(value: FilterComparatorModel) -> Self {
            match value {
                FilterComparatorModel::Equal => Self::Equal,
                FilterComparatorModel::NotEqual => Self::NotEqual,
                FilterComparatorModel::GreaterEqual => Self::GreaterEqual,
                FilterComparatorModel::Greater => Self::Greater,
                FilterComparatorModel::LessEqual => Self::LessEqual,
                FilterComparatorModel::Less => Self::Less,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    enum StringFilterComparatorModel {
        Equal,
        NotEqual,
        Like,
        NotLike,
        StartsWith,
    }

    impl From<host::StringFilterComparator> for StringFilterComparatorModel {
        fn from(value: host::StringFilterComparator) -> Self {
            match value {
                host::StringFilterComparator::Equal => Self::Equal,
                host::StringFilterComparator::NotEqual => Self::NotEqual,
                host::StringFilterComparator::Like => Self::Like,
                host::StringFilterComparator::NotLike => Self::NotLike,
                host::StringFilterComparator::StartsWith => Self::StartsWith,
            }
        }
    }

    impl From<StringFilterComparatorModel> for host::StringFilterComparator {
        fn from(value: StringFilterComparatorModel) -> Self {
            match value {
                StringFilterComparatorModel::Equal => Self::Equal,
                StringFilterComparatorModel::NotEqual => Self::NotEqual,
                StringFilterComparatorModel::Like => Self::Like,
                StringFilterComparatorModel::NotLike => Self::NotLike,
                StringFilterComparatorModel::StartsWith => Self::StartsWith,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    enum AgentStatusModel {
        Running,
        Idle,
        Suspended,
        Interrupted,
        Retrying,
        Failed,
        Exited,
    }

    impl From<host::AgentStatus> for AgentStatusModel {
        fn from(value: host::AgentStatus) -> Self {
            match value {
                host::AgentStatus::Running => Self::Running,
                host::AgentStatus::Idle => Self::Idle,
                host::AgentStatus::Suspended => Self::Suspended,
                host::AgentStatus::Interrupted => Self::Interrupted,
                host::AgentStatus::Retrying => Self::Retrying,
                host::AgentStatus::Failed => Self::Failed,
                host::AgentStatus::Exited => Self::Exited,
            }
        }
    }

    impl From<AgentStatusModel> for host::AgentStatus {
        fn from(value: AgentStatusModel) -> Self {
            match value {
                AgentStatusModel::Running => Self::Running,
                AgentStatusModel::Idle => Self::Idle,
                AgentStatusModel::Suspended => Self::Suspended,
                AgentStatusModel::Interrupted => Self::Interrupted,
                AgentStatusModel::Retrying => Self::Retrying,
                AgentStatusModel::Failed => Self::Failed,
                AgentStatusModel::Exited => Self::Exited,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    enum UpdateModeModel {
        Automatic,
        SnapshotBased,
    }

    fn update_mode_to_model(value: host::UpdateMode) -> UpdateModeModel {
        match value {
            host::UpdateMode::Automatic => UpdateModeModel::Automatic,
            host::UpdateMode::SnapshotBased => UpdateModeModel::SnapshotBased,
        }
    }

    fn update_mode_from_model(value: UpdateModeModel) -> host::UpdateMode {
        match value {
            UpdateModeModel::Automatic => host::UpdateMode::Automatic,
            UpdateModeModel::SnapshotBased => host::UpdateMode::SnapshotBased,
        }
    }

    impl_schema_conversion_via_model!(
        host::UpdateMode,
        UpdateModeModel,
        update_mode_to_model,
        update_mode_from_model
    );

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentNameFilterModel {
        comparator: StringFilterComparatorModel,
        value: String,
    }

    impl From<host::AgentNameFilter> for AgentNameFilterModel {
        fn from(value: host::AgentNameFilter) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    impl From<AgentNameFilterModel> for host::AgentNameFilter {
        fn from(value: AgentNameFilterModel) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentStatusFilterModel {
        comparator: FilterComparatorModel,
        value: AgentStatusModel,
    }

    impl From<host::AgentStatusFilter> for AgentStatusFilterModel {
        fn from(value: host::AgentStatusFilter) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value.into(),
            }
        }
    }

    impl From<AgentStatusFilterModel> for host::AgentStatusFilter {
        fn from(value: AgentStatusFilterModel) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value.into(),
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentVersionFilterModel {
        comparator: FilterComparatorModel,
        value: u64,
    }

    impl From<host::AgentVersionFilter> for AgentVersionFilterModel {
        fn from(value: host::AgentVersionFilter) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    impl From<AgentVersionFilterModel> for host::AgentVersionFilter {
        fn from(value: AgentVersionFilterModel) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentCreatedAtFilterModel {
        comparator: FilterComparatorModel,
        value: u64,
    }

    impl From<host::AgentCreatedAtFilter> for AgentCreatedAtFilterModel {
        fn from(value: host::AgentCreatedAtFilter) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    impl From<AgentCreatedAtFilterModel> for host::AgentCreatedAtFilter {
        fn from(value: AgentCreatedAtFilterModel) -> Self {
            Self {
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentEnvFilterModel {
        name: String,
        comparator: StringFilterComparatorModel,
        value: String,
    }

    impl From<host::AgentEnvFilter> for AgentEnvFilterModel {
        fn from(value: host::AgentEnvFilter) -> Self {
            Self {
                name: value.name,
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    impl From<AgentEnvFilterModel> for host::AgentEnvFilter {
        fn from(value: AgentEnvFilterModel) -> Self {
            Self {
                name: value.name,
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentConfigVarsFilterModel {
        name: String,
        comparator: StringFilterComparatorModel,
        value: String,
    }

    impl From<host::AgentConfigVarsFilter> for AgentConfigVarsFilterModel {
        fn from(value: host::AgentConfigVarsFilter) -> Self {
            Self {
                name: value.name,
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    impl From<AgentConfigVarsFilterModel> for host::AgentConfigVarsFilter {
        fn from(value: AgentConfigVarsFilterModel) -> Self {
            Self {
                name: value.name,
                comparator: value.comparator.into(),
                value: value.value,
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    enum AgentPropertyFilterModel {
        Name(AgentNameFilterModel),
        Status(AgentStatusFilterModel),
        Version(AgentVersionFilterModel),
        CreatedAt(AgentCreatedAtFilterModel),
        Env(AgentEnvFilterModel),
        Config(AgentConfigVarsFilterModel),
    }

    impl From<host::AgentPropertyFilter> for AgentPropertyFilterModel {
        fn from(value: host::AgentPropertyFilter) -> Self {
            match value {
                host::AgentPropertyFilter::Name(value) => Self::Name(value.into()),
                host::AgentPropertyFilter::Status(value) => Self::Status(value.into()),
                host::AgentPropertyFilter::Version(value) => Self::Version(value.into()),
                host::AgentPropertyFilter::CreatedAt(value) => Self::CreatedAt(value.into()),
                host::AgentPropertyFilter::Env(value) => Self::Env(value.into()),
                host::AgentPropertyFilter::Config(value) => Self::Config(value.into()),
            }
        }
    }

    impl From<AgentPropertyFilterModel> for host::AgentPropertyFilter {
        fn from(value: AgentPropertyFilterModel) -> Self {
            match value {
                AgentPropertyFilterModel::Name(value) => Self::Name(value.into()),
                AgentPropertyFilterModel::Status(value) => Self::Status(value.into()),
                AgentPropertyFilterModel::Version(value) => Self::Version(value.into()),
                AgentPropertyFilterModel::CreatedAt(value) => Self::CreatedAt(value.into()),
                AgentPropertyFilterModel::Env(value) => Self::Env(value.into()),
                AgentPropertyFilterModel::Config(value) => Self::Config(value.into()),
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentAllFilterModel {
        filters: Vec<AgentPropertyFilterModel>,
    }

    impl From<host::AgentAllFilter> for AgentAllFilterModel {
        fn from(value: host::AgentAllFilter) -> Self {
            Self {
                filters: value.filters.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<AgentAllFilterModel> for host::AgentAllFilter {
        fn from(value: AgentAllFilterModel) -> Self {
            Self {
                filters: value.filters.into_iter().map(Into::into).collect(),
            }
        }
    }

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentAnyFilterModel {
        filters: Vec<AgentAllFilterModel>,
    }

    fn agent_any_filter_to_model(value: host::AgentAnyFilter) -> AgentAnyFilterModel {
        AgentAnyFilterModel {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }

    fn agent_any_filter_from_model(value: AgentAnyFilterModel) -> host::AgentAnyFilter {
        host::AgentAnyFilter {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }

    impl_schema_conversion_via_model!(
        host::AgentAnyFilter,
        AgentAnyFilterModel,
        agent_any_filter_to_model,
        agent_any_filter_from_model
    );

    #[derive(Clone, Debug, IntoSchema, FromSchema)]
    struct AgentMetadataModel {
        agent_id: AgentId,
        args: Vec<String>,
        env: Vec<(String, String)>,
        config: Vec<(String, String)>,
        status: AgentStatusModel,
        component_revision: u64,
        retry_count: u64,
        environment_id: EnvironmentId,
    }

    fn agent_metadata_to_model(value: host::AgentMetadata) -> AgentMetadataModel {
        AgentMetadataModel {
            agent_id: agent_id_to_model(value.agent_id),
            args: value.args,
            env: value.env,
            config: value.config,
            status: value.status.into(),
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            environment_id: environment_id_to_model(value.environment_id),
        }
    }

    fn agent_metadata_from_model(value: AgentMetadataModel) -> host::AgentMetadata {
        host::AgentMetadata {
            agent_id: agent_id_from_model(value.agent_id),
            args: value.args,
            env: value.env,
            config: value.config,
            status: value.status.into(),
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            environment_id: environment_id_from_model(value.environment_id),
        }
    }

    impl_schema_conversion_via_model!(
        host::AgentMetadata,
        AgentMetadataModel,
        agent_metadata_to_model,
        agent_metadata_from_model
    );
}

pub trait MultimodalSchema {
    fn get_multimodal_schema() -> Vec<(String, SchemaGraph)>;

    fn get_name(&self) -> String;

    fn to_schema_value(self) -> Result<(String, SchemaValue), String>
    where
        Self: Sized;

    fn from_schema_value(name: String, value: SchemaValue) -> Result<Self, String>
    where
        Self: Sized;
}

pub fn schema_graph_root(schema: &SchemaGraph) -> SchemaType {
    schema.root.clone()
}

pub fn unstructured_text_inline_value(text: String, language: Option<String>) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: 0,
        payload: Some(Box::new(SchemaValue::Text(TextValuePayload {
            text,
            language,
        }))),
    })
}

pub fn unstructured_text_url_value(url: String) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: 1,
        payload: Some(Box::new(SchemaValue::Url { url })),
    })
}

pub fn unstructured_binary_inline_value(bytes: Vec<u8>, mime_type: Option<String>) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: 0,
        payload: Some(Box::new(SchemaValue::Binary(BinaryValuePayload {
            bytes,
            mime_type,
        }))),
    })
}

pub fn unstructured_binary_url_value(url: String) -> SchemaValue {
    SchemaValue::Variant(VariantValuePayload {
        case: 1,
        payload: Some(Box::new(SchemaValue::Url { url })),
    })
}

pub fn unstructured_text_schema_graph(restrictions: Option<Vec<String>>) -> SchemaGraph {
    SchemaGraph::anonymous(unstructured_text_schema_type(restrictions))
}

pub fn unstructured_binary_schema_graph(restrictions: Option<Vec<String>>) -> SchemaGraph {
    SchemaGraph::anonymous(unstructured_binary_schema_type(restrictions))
}

pub fn multimodal_schema_graph(fields: &[(String, SchemaGraph)]) -> SchemaGraph {
    let merged = crate::schema::merge_agent_graphs(
        fields
            .iter()
            .map(|(_, schema)| schema.clone())
            .collect::<Vec<_>>(),
    )
    .expect("failed to merge multimodal schema graphs");

    SchemaGraph {
        defs: merged.defs,
        root: multimodal_schema_type(fields),
    }
}

pub fn unstructured_text_schema_type(restrictions: Option<Vec<String>>) -> SchemaType {
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "inline".to_string(),
            payload: Some(SchemaType::Text {
                restrictions: TextRestrictions {
                    languages: restrictions,
                    min_length: None,
                    max_length: None,
                    regex: None,
                },
                metadata: MetadataEnvelope::default(),
            }),
            metadata: MetadataEnvelope::default(),
        },
        VariantCaseType {
            name: "url".to_string(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    ty.metadata_mut().role = Some(Role::UnstructuredText);
    ty
}

pub fn unstructured_binary_schema_type(restrictions: Option<Vec<String>>) -> SchemaType {
    let mut ty = SchemaType::variant(vec![
        VariantCaseType {
            name: "inline".to_string(),
            payload: Some(SchemaType::Binary {
                restrictions: BinaryRestrictions {
                    mime_types: restrictions,
                    min_bytes: None,
                    max_bytes: None,
                },
                metadata: MetadataEnvelope::default(),
            }),
            metadata: MetadataEnvelope::default(),
        },
        VariantCaseType {
            name: "url".to_string(),
            payload: Some(SchemaType::url(UrlRestrictions::default())),
            metadata: MetadataEnvelope::default(),
        },
    ]);
    ty.metadata_mut().role = Some(Role::UnstructuredBinary);
    ty
}

pub fn multimodal_schema_type(fields: &[(String, SchemaGraph)]) -> SchemaType {
    let mut schema = SchemaType::list(SchemaType::variant(
        fields
            .iter()
            .map(|(name, schema)| VariantCaseType {
                name: name.clone(),
                payload: Some(schema_graph_root(schema)),
                metadata: MetadataEnvelope::default(),
            })
            .collect(),
    ));
    schema.metadata_mut().role = Some(Role::Multimodal);
    schema
}

/// Rejects quota-token capability values appearing anywhere inside an agent
/// constructor parameter tree.
///
/// Constructor parameters define an agent's deterministic, stable identity (they
/// are encoded into its agent-id). A quota token is an opaque, single-use
/// capability handle and must never participate in identity, so it is rejected
/// here — before the constructor value is ever encoded — so that the handle is
/// not silently consumed and the user gets a clear error.
///
/// This is an internal helper used by generated remote-client constructors.
#[doc(hidden)]
pub fn __reject_quota_tokens_in_agent_constructor(value: &SchemaValue) -> Result<(), String> {
    use crate::schema::ResultValuePayload;

    fn contains_quota_token(value: &SchemaValue) -> bool {
        match value {
            SchemaValue::QuotaToken(_) => true,
            SchemaValue::Record { fields } => fields.iter().any(contains_quota_token),
            SchemaValue::Tuple { elements }
            | SchemaValue::List { elements }
            | SchemaValue::FixedList { elements } => elements.iter().any(contains_quota_token),
            SchemaValue::Variant(payload) => {
                payload.payload.as_deref().is_some_and(contains_quota_token)
            }
            SchemaValue::Map { entries } => entries
                .iter()
                .any(|(k, v)| contains_quota_token(k) || contains_quota_token(v)),
            SchemaValue::Option { inner } => inner.as_deref().is_some_and(contains_quota_token),
            SchemaValue::Result(payload) => match payload {
                ResultValuePayload::Ok { value } | ResultValuePayload::Err { value } => {
                    value.as_deref().is_some_and(contains_quota_token)
                }
            },
            SchemaValue::Union(payload) => contains_quota_token(&payload.body),
            _ => false,
        }
    }

    if contains_quota_token(value) {
        Err(
            "quota tokens are not allowed in agent constructor parameters because constructor \
             parameters define the agent's deterministic identity; pass quota tokens to methods \
             instead"
                .to_string(),
        )
    } else {
        Ok(())
    }
}
