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

#[cfg(test)]
test_r::enable!();

pub use uuid::Uuid;
pub use wasip2;
pub use wstd;

pub use golem_schema;
pub use golem_schema::schema;
pub use golem_schema::schema::{
    FromSchema, IntoSchema, IntoTypedSchemaValue, Schema, SchemaGraph, SchemaType, SchemaValue,
    TypedSchemaValue,
};
pub use golem_schema::{AgentId, ComponentId, EnvironmentId, PromiseId};

pub fn encode_schema_graph(
    graph: &SchemaGraph,
) -> Result<schema::wit::wire::SchemaGraph, schema::wit::EncodeError> {
    schema::wit::encode_graph(graph)
}

pub fn decode_schema_graph(
    graph: &schema::wit::wire::SchemaGraph,
) -> Result<SchemaGraph, schema::wit::DecodeError> {
    schema::wit::decode_graph(graph)
}

pub fn encode_schema_value(value: &SchemaValue) -> schema::wit::wire::SchemaValueTree {
    schema::wit::encode_value(value)
}

pub fn decode_schema_value(
    value: &schema::wit::wire::SchemaValueTree,
) -> Result<SchemaValue, schema::wit::DecodeError> {
    schema::wit::decode_value(value)
}

pub fn encode_typed_schema_value(
    value: &TypedSchemaValue,
) -> Result<schema::wit::wire::TypedSchemaValue, schema::wit::EncodeError> {
    schema::wit::encode_typed(value)
}

pub fn decode_typed_schema_value(
    value: &schema::wit::wire::TypedSchemaValue,
) -> Result<TypedSchemaValue, schema::wit::DecodeError> {
    schema::wit::decode_typed(value)
}

mod raw_bindings {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/poll@0.2.3": wasip2::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,
        }
    });
}

pub mod bindings {
    pub use crate::raw_bindings::wasi;

    pub mod golem {
        pub mod api {
            pub(crate) use crate::raw_bindings::golem::api::host;
            pub use crate::raw_bindings::golem::api::{context, oplog, retry};
        }

        pub mod durability {
            pub(crate) use crate::raw_bindings::golem::durability::durability;
        }

        pub mod quota {
            pub(crate) use crate::raw_bindings::golem::quota::types;
        }

        pub mod agent {
            pub use crate::raw_bindings::golem::agent::host;
        }

        pub use crate::raw_bindings::golem::{rdbms, websocket};
    }
}

#[cfg(feature = "export_load_snapshot")]
pub mod load_snapshot {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust-load-snapshot",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/poll@0.2.3": wasip2::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/retry@1.5.0": crate::bindings::golem::api::retry,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:quota/types@1.5.0": crate::bindings::golem::quota::types,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_load_snapshot_impl as export_load_snapshot;
}

#[cfg(feature = "export_save_snapshot")]
pub mod save_snapshot {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-rust-save-snapshot",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/poll@0.2.3": wasip2::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/retry@1.5.0": crate::bindings::golem::api::retry,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:quota/types@1.5.0": crate::bindings::golem::quota::types,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_save_snapshot_impl as export_save_snapshot;
}

#[cfg(feature = "export_golem_agentic")]
pub mod golem_agentic {
    use wit_bindgen::generate;

    generate!({
        path: "wit",
        world: "golem-agentic",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,

        with: {
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/poll@0.2.3": wasip2::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/retry@1.5.0": crate::bindings::golem::api::retry,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:quota/types@1.5.0": crate::bindings::golem::quota::types,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_agentic_impl as export_golem_agentic;
}

#[cfg(feature = "export_golem_agentic")]
pub use ctor;

#[cfg(feature = "export_golem_agentic")]
pub use async_trait;

#[cfg(feature = "export_golem_agentic")]
pub use serde;

#[cfg(feature = "export_golem_agentic")]
pub use serde_json;

#[cfg(feature = "export_oplog_processor")]
pub mod oplog_processor {
    use wit_bindgen::generate;

    pub mod host {
        pub use crate::raw_bindings::golem::api::host::AgentMetadata;
    }

    generate!({
        path: "wit",
        world: "golem-rust-oplog-processor",
        generate_all,
        generate_unused_types: true,
        pub_export_macro: true,
        with: {
            "golem:core/types@2.0.0": golem_schema::schema::wit::wire,
            "wasi:io/poll@0.2.3": wasip2::io::poll,
            "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,

            "golem:api/host@1.5.0": crate::bindings::golem::api::host,
            "golem:api/retry@1.5.0": crate::bindings::golem::api::retry,
            "golem:api/oplog@1.5.0": crate::bindings::golem::api::oplog,
            "golem:api/context@1.5.0": crate::bindings::golem::api::context,
            "golem:durability/durability@1.5.0": crate::bindings::golem::durability::durability,
            "golem:quota/types@1.5.0": crate::bindings::golem::quota::types,
            "golem:rdbms/mysql@1.5.0": crate::bindings::golem::rdbms::mysql,
            "golem:rdbms/postgres@1.5.0": crate::bindings::golem::rdbms::postgres,
            "golem:rdbms/types@1.5.0": crate::bindings::golem::rdbms::types,
            "wasi:blobstore/blobstore": crate::bindings::wasi::blobstore::blobstore,
            "wasi:blobstore/container": crate::bindings::wasi::blobstore::container,
            "wasi:blobstore/types": crate::bindings::wasi::blobstore::types,
            "wasi:keyvalue/eventual-batch@0.1.0": crate::bindings::wasi::keyvalue::eventual_batch,
            "wasi:keyvalue/eventual@0.1.0": crate::bindings::wasi::keyvalue::eventual,
            "wasi:keyvalue/types@0.1.0": crate::bindings::wasi::keyvalue::types,
            "wasi:keyvalue/wasi-keyvalue-error@0.1.0": crate::bindings::wasi::keyvalue::wasi_keyvalue_error,
            "wasi:logging/logging": crate::bindings::wasi::logging::logging,
        }
    });

    pub use __export_golem_rust_oplog_processor_impl as export_oplog_processor;
}

#[cfg(feature = "export_golem_agentic")]
pub mod agentic;

#[cfg(feature = "durability")]
pub mod durability;

#[cfg(feature = "json")]
mod json;

#[cfg(feature = "json")]
pub use json::*;

mod checkpoint;
pub mod quota;
mod transaction;

use std::future::Future;

use bindings::golem::api::host::{
    self as host_api, get_idempotence_mode, get_oplog_persistence_level, mark_begin_operation,
    mark_end_operation, set_idempotence_mode, set_oplog_persistence_level,
};

pub type OplogIndex = u64;

pub mod websocket;
pub use checkpoint::*;
pub use quota::*;
pub mod retry;
pub use transaction::*;
pub use websocket::{WebSocketCloseInfo, WebSocketError, WebSocketMessage, WebsocketConnection};

#[cfg(feature = "macro")]
pub use golem_rust_macro::*;

fn schema_uuid_to_wire(value: Uuid) -> schema::wit::wire::Uuid {
    let (high_bits, low_bits) = value.as_u64_pair();
    schema::wit::wire::Uuid {
        high_bits,
        low_bits,
    }
}

fn wire_uuid_to_schema(value: schema::wit::wire::Uuid) -> Uuid {
    Uuid::from_u64_pair(value.high_bits, value.low_bits)
}

fn schema_component_id_to_wire(value: ComponentId) -> schema::wit::wire::ComponentId {
    schema::wit::wire::ComponentId {
        uuid: schema_uuid_to_wire(value.uuid),
    }
}

fn wire_component_id_to_schema(value: schema::wit::wire::ComponentId) -> ComponentId {
    ComponentId {
        uuid: wire_uuid_to_schema(value.uuid),
    }
}

fn schema_agent_id_to_wire(value: AgentId) -> schema::wit::wire::AgentId {
    schema::wit::wire::AgentId {
        component_id: schema_component_id_to_wire(value.component_id),
        agent_id: value.agent_id,
    }
}

fn wire_agent_id_to_schema(value: schema::wit::wire::AgentId) -> AgentId {
    AgentId {
        component_id: wire_component_id_to_schema(value.component_id),
        agent_id: value.agent_id,
    }
}

fn schema_promise_id_to_wire(value: &PromiseId) -> schema::wit::wire::PromiseId {
    schema::wit::wire::PromiseId {
        agent_id: schema_agent_id_to_wire(value.agent_id.clone()),
        oplog_idx: value.oplog_idx,
    }
}

fn wire_promise_id_to_schema(value: schema::wit::wire::PromiseId) -> PromiseId {
    PromiseId {
        agent_id: wire_agent_id_to_schema(value.agent_id),
        oplog_idx: value.oplog_idx,
    }
}

fn schema_environment_id_to_host(value: EnvironmentId) -> host_api::EnvironmentId {
    host_api::EnvironmentId {
        uuid: schema_uuid_to_wire(value.uuid),
    }
}

fn host_environment_id_to_schema(value: host_api::EnvironmentId) -> EnvironmentId {
    EnvironmentId {
        uuid: wire_uuid_to_schema(value.uuid),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

impl From<host_api::PersistenceLevel> for PersistenceLevel {
    fn from(value: host_api::PersistenceLevel) -> Self {
        match value {
            host_api::PersistenceLevel::PersistNothing => Self::PersistNothing,
            host_api::PersistenceLevel::PersistRemoteSideEffects => Self::PersistRemoteSideEffects,
            host_api::PersistenceLevel::Smart => Self::Smart,
        }
    }
}

impl From<PersistenceLevel> for host_api::PersistenceLevel {
    fn from(value: PersistenceLevel) -> Self {
        match value {
            PersistenceLevel::PersistNothing => Self::PersistNothing,
            PersistenceLevel::PersistRemoteSideEffects => Self::PersistRemoteSideEffects,
            PersistenceLevel::Smart => Self::Smart,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum UpdateMode {
    Automatic,
    SnapshotBased,
}

impl From<host_api::UpdateMode> for UpdateMode {
    fn from(value: host_api::UpdateMode) -> Self {
        match value {
            host_api::UpdateMode::Automatic => Self::Automatic,
            host_api::UpdateMode::SnapshotBased => Self::SnapshotBased,
        }
    }
}

impl From<UpdateMode> for host_api::UpdateMode {
    fn from(value: UpdateMode) -> Self {
        match value {
            UpdateMode::Automatic => Self::Automatic,
            UpdateMode::SnapshotBased => Self::SnapshotBased,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum FilterComparator {
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
    LessEqual,
    Less,
}

impl From<host_api::FilterComparator> for FilterComparator {
    fn from(value: host_api::FilterComparator) -> Self {
        match value {
            host_api::FilterComparator::Equal => Self::Equal,
            host_api::FilterComparator::NotEqual => Self::NotEqual,
            host_api::FilterComparator::GreaterEqual => Self::GreaterEqual,
            host_api::FilterComparator::Greater => Self::Greater,
            host_api::FilterComparator::LessEqual => Self::LessEqual,
            host_api::FilterComparator::Less => Self::Less,
        }
    }
}

impl From<FilterComparator> for host_api::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => Self::Equal,
            FilterComparator::NotEqual => Self::NotEqual,
            FilterComparator::GreaterEqual => Self::GreaterEqual,
            FilterComparator::Greater => Self::Greater,
            FilterComparator::LessEqual => Self::LessEqual,
            FilterComparator::Less => Self::Less,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum StringFilterComparator {
    Equal,
    NotEqual,
    Like,
    NotLike,
    StartsWith,
}

impl From<host_api::StringFilterComparator> for StringFilterComparator {
    fn from(value: host_api::StringFilterComparator) -> Self {
        match value {
            host_api::StringFilterComparator::Equal => Self::Equal,
            host_api::StringFilterComparator::NotEqual => Self::NotEqual,
            host_api::StringFilterComparator::Like => Self::Like,
            host_api::StringFilterComparator::NotLike => Self::NotLike,
            host_api::StringFilterComparator::StartsWith => Self::StartsWith,
        }
    }
}

impl From<StringFilterComparator> for host_api::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => Self::Equal,
            StringFilterComparator::NotEqual => Self::NotEqual,
            StringFilterComparator::Like => Self::Like,
            StringFilterComparator::NotLike => Self::NotLike,
            StringFilterComparator::StartsWith => Self::StartsWith,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum AgentStatus {
    Running,
    Idle,
    Suspended,
    Interrupted,
    Retrying,
    Failed,
    Exited,
}

impl From<host_api::AgentStatus> for AgentStatus {
    fn from(value: host_api::AgentStatus) -> Self {
        match value {
            host_api::AgentStatus::Running => Self::Running,
            host_api::AgentStatus::Idle => Self::Idle,
            host_api::AgentStatus::Suspended => Self::Suspended,
            host_api::AgentStatus::Interrupted => Self::Interrupted,
            host_api::AgentStatus::Retrying => Self::Retrying,
            host_api::AgentStatus::Failed => Self::Failed,
            host_api::AgentStatus::Exited => Self::Exited,
        }
    }
}

impl From<AgentStatus> for host_api::AgentStatus {
    fn from(value: AgentStatus) -> Self {
        match value {
            AgentStatus::Running => Self::Running,
            AgentStatus::Idle => Self::Idle,
            AgentStatus::Suspended => Self::Suspended,
            AgentStatus::Interrupted => Self::Interrupted,
            AgentStatus::Retrying => Self::Retrying,
            AgentStatus::Failed => Self::Failed,
            AgentStatus::Exited => Self::Exited,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentNameFilter {
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl From<host_api::AgentNameFilter> for AgentNameFilter {
    fn from(value: host_api::AgentNameFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

impl From<AgentNameFilter> for host_api::AgentNameFilter {
    fn from(value: AgentNameFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentStatusFilter {
    pub comparator: FilterComparator,
    pub value: AgentStatus,
}

impl From<host_api::AgentStatusFilter> for AgentStatusFilter {
    fn from(value: host_api::AgentStatusFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: Into::into(value.value),
        }
    }
}

impl From<AgentStatusFilter> for host_api::AgentStatusFilter {
    fn from(value: AgentStatusFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: Into::into(value.value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentVersionFilter {
    pub comparator: FilterComparator,
    pub value: u64,
}

impl From<host_api::AgentVersionFilter> for AgentVersionFilter {
    fn from(value: host_api::AgentVersionFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

impl From<AgentVersionFilter> for host_api::AgentVersionFilter {
    fn from(value: AgentVersionFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentCreatedAtFilter {
    pub comparator: FilterComparator,
    pub value: u64,
}

impl From<host_api::AgentCreatedAtFilter> for AgentCreatedAtFilter {
    fn from(value: host_api::AgentCreatedAtFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

impl From<AgentCreatedAtFilter> for host_api::AgentCreatedAtFilter {
    fn from(value: AgentCreatedAtFilter) -> Self {
        Self {
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentEnvFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl From<host_api::AgentEnvFilter> for AgentEnvFilter {
    fn from(value: host_api::AgentEnvFilter) -> Self {
        Self {
            name: value.name,
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

impl From<AgentEnvFilter> for host_api::AgentEnvFilter {
    fn from(value: AgentEnvFilter) -> Self {
        Self {
            name: value.name,
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentConfigVarsFilter {
    pub name: String,
    pub comparator: StringFilterComparator,
    pub value: String,
}

impl From<host_api::AgentConfigVarsFilter> for AgentConfigVarsFilter {
    fn from(value: host_api::AgentConfigVarsFilter) -> Self {
        Self {
            name: value.name,
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

impl From<AgentConfigVarsFilter> for host_api::AgentConfigVarsFilter {
    fn from(value: AgentConfigVarsFilter) -> Self {
        Self {
            name: value.name,
            comparator: Into::into(value.comparator),
            value: value.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum AgentPropertyFilter {
    Name(AgentNameFilter),
    Status(AgentStatusFilter),
    Version(AgentVersionFilter),
    CreatedAt(AgentCreatedAtFilter),
    Env(AgentEnvFilter),
    Config(AgentConfigVarsFilter),
}

impl From<host_api::AgentPropertyFilter> for AgentPropertyFilter {
    fn from(value: host_api::AgentPropertyFilter) -> Self {
        match value {
            host_api::AgentPropertyFilter::Name(value) => Self::Name(Into::into(value)),
            host_api::AgentPropertyFilter::Status(value) => Self::Status(Into::into(value)),
            host_api::AgentPropertyFilter::Version(value) => Self::Version(Into::into(value)),
            host_api::AgentPropertyFilter::CreatedAt(value) => Self::CreatedAt(Into::into(value)),
            host_api::AgentPropertyFilter::Env(value) => Self::Env(Into::into(value)),
            host_api::AgentPropertyFilter::Config(value) => Self::Config(Into::into(value)),
        }
    }
}

impl From<AgentPropertyFilter> for host_api::AgentPropertyFilter {
    fn from(value: AgentPropertyFilter) -> Self {
        match value {
            AgentPropertyFilter::Name(value) => Self::Name(Into::into(value)),
            AgentPropertyFilter::Status(value) => Self::Status(Into::into(value)),
            AgentPropertyFilter::Version(value) => Self::Version(Into::into(value)),
            AgentPropertyFilter::CreatedAt(value) => Self::CreatedAt(Into::into(value)),
            AgentPropertyFilter::Env(value) => Self::Env(Into::into(value)),
            AgentPropertyFilter::Config(value) => Self::Config(Into::into(value)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentAllFilter {
    pub filters: Vec<AgentPropertyFilter>,
}

impl From<host_api::AgentAllFilter> for AgentAllFilter {
    fn from(value: host_api::AgentAllFilter) -> Self {
        Self {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<AgentAllFilter> for host_api::AgentAllFilter {
    fn from(value: AgentAllFilter) -> Self {
        Self {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentAnyFilter {
    pub filters: Vec<AgentAllFilter>,
}

impl From<host_api::AgentAnyFilter> for AgentAnyFilter {
    fn from(value: host_api::AgentAnyFilter) -> Self {
        Self {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<AgentAnyFilter> for host_api::AgentAnyFilter {
    fn from(value: AgentAnyFilter) -> Self {
        Self {
            filters: value.filters.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct AgentMetadata {
    pub agent_id: AgentId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub config: Vec<(String, String)>,
    pub status: AgentStatus,
    pub component_revision: u64,
    pub retry_count: u64,
    pub environment_id: EnvironmentId,
}

impl From<host_api::AgentMetadata> for AgentMetadata {
    fn from(value: host_api::AgentMetadata) -> Self {
        Self {
            agent_id: wire_agent_id_to_schema(value.agent_id),
            args: value.args,
            env: value.env,
            config: value.config,
            status: Into::into(value.status),
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            environment_id: host_environment_id_to_schema(value.environment_id),
        }
    }
}

impl From<AgentMetadata> for host_api::AgentMetadata {
    fn from(value: AgentMetadata) -> Self {
        Self {
            agent_id: schema_agent_id_to_wire(value.agent_id),
            args: value.args,
            env: value.env,
            config: value.config,
            status: Into::into(value.status),
            component_revision: value.component_revision,
            retry_count: value.retry_count,
            environment_id: schema_environment_id_to_host(value.environment_id),
        }
    }
}

pub struct GetAgents {
    raw: host_api::GetAgents,
}

impl GetAgents {
    pub fn new(component_id: ComponentId, filter: Option<&AgentAnyFilter>, precise: bool) -> Self {
        let raw_filter = filter.cloned().map(host_api::AgentAnyFilter::from);
        Self {
            raw: host_api::GetAgents::new(
                schema_component_id_to_wire(component_id),
                raw_filter.as_ref(),
                precise,
            ),
        }
    }

    pub fn get_next(&self) -> Option<Vec<AgentMetadata>> {
        self.raw
            .get_next()
            .map(|values| values.into_iter().map(Into::into).collect())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub struct ForkDetails {
    pub forked_phantom_id: Uuid,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum ForkResult {
    Original(ForkDetails),
    Forked(ForkDetails),
}

impl From<host_api::ForkResult> for ForkResult {
    fn from(value: host_api::ForkResult) -> Self {
        match value {
            host_api::ForkResult::Original(details) => Self::Original(ForkDetails {
                forked_phantom_id: wire_uuid_to_schema(details.forked_phantom_id),
            }),
            host_api::ForkResult::Forked(details) => Self::Forked(ForkDetails {
                forked_phantom_id: wire_uuid_to_schema(details.forked_phantom_id),
            }),
        }
    }
}

pub struct PromiseResult {
    raw: host_api::GetPromiseResult,
}

impl PromiseResult {
    pub fn subscribe(&self) -> wasip2::io::poll::Pollable {
        self.raw.subscribe()
    }

    pub fn get(&self) -> Option<Vec<u8>> {
        self.raw.get()
    }
}

pub fn create_promise() -> PromiseId {
    wire_promise_id_to_schema(host_api::create_promise())
}

pub fn get_promise(promise_id: &PromiseId) -> PromiseResult {
    PromiseResult {
        raw: host_api::get_promise(&schema_promise_id_to_wire(promise_id)),
    }
}

pub fn complete_promise(promise_id: &PromiseId, data: &[u8]) -> bool {
    host_api::complete_promise(&schema_promise_id_to_wire(promise_id), data)
}

pub fn get_oplog_index() -> OplogIndex {
    host_api::get_oplog_index()
}

pub fn set_oplog_index(oplog_idx: OplogIndex) {
    host_api::set_oplog_index(oplog_idx)
}

pub fn oplog_commit(replicas: u8) {
    host_api::oplog_commit(replicas)
}

pub fn get_self_metadata() -> AgentMetadata {
    Into::into(host_api::get_self_metadata())
}

pub fn get_agent_metadata(agent_id: &AgentId) -> Option<AgentMetadata> {
    host_api::get_agent_metadata(&schema_agent_id_to_wire(agent_id.clone())).map(Into::into)
}

pub fn update_agent(agent_id: &AgentId, target_revision: u64, mode: UpdateMode) {
    host_api::update_agent(
        &schema_agent_id_to_wire(agent_id.clone()),
        target_revision,
        Into::into(mode),
    )
}

pub fn resolve_component_id(component_reference: &str) -> Option<ComponentId> {
    host_api::resolve_component_id(component_reference).map(wire_component_id_to_schema)
}

pub fn resolve_agent_id(component_reference: &str, agent_name: &str) -> Option<AgentId> {
    host_api::resolve_agent_id(component_reference, agent_name).map(wire_agent_id_to_schema)
}

pub fn resolve_agent_id_strict(component_reference: &str, agent_name: &str) -> Option<AgentId> {
    host_api::resolve_agent_id_strict(component_reference, agent_name).map(wire_agent_id_to_schema)
}

pub fn fork() -> ForkResult {
    Into::into(host_api::fork())
}

/// Awaits a promise blocking the execution of the agent. The agent is going to be
/// suspended until the promise is completed.
///
/// Use `await_promise` for an async version of this function, allowing to interleave
/// awaiting of the promise with other operations.
pub fn blocking_await_promise(promise_id: &PromiseId) -> Vec<u8> {
    let promise = get_promise(promise_id);
    let pollable = promise.subscribe();
    pollable.block();
    promise.get().unwrap()
}

/// Awaits a promise.
///
/// If only promises or timeouts are awaited simultaneously, the agent is going to be
/// suspended until any of them completes.
pub async fn await_promise(promise_id: &PromiseId) -> Vec<u8> {
    let promise = get_promise(promise_id);
    let pollable = promise.subscribe();
    wstd::io::AsyncPollable::new(pollable).wait_for().await;
    promise.get().unwrap()
}

pub struct PersistenceLevelGuard {
    original_level: host_api::PersistenceLevel,
}

impl Drop for PersistenceLevelGuard {
    fn drop(&mut self) {
        set_oplog_persistence_level(self.original_level);
    }
}

/// Temporarily sets the oplog persistence level to the given value.
///
/// When the returned guard is dropped, the original persistence level is restored.
#[must_use]
pub fn use_persistence_level(level: PersistenceLevel) -> PersistenceLevelGuard {
    let original_level = get_oplog_persistence_level();
    set_oplog_persistence_level(Into::into(level));
    PersistenceLevelGuard { original_level }
}

/// Executes the given function with the oplog persistence level set to the given value.
pub fn with_persistence_level<R>(level: PersistenceLevel, f: impl FnOnce() -> R) -> R {
    let _guard = use_persistence_level(level);
    f()
}

/// Executes the given async function with the oplog persistence level set to the given value.
pub async fn with_persistence_level_async<R, F: Future<Output = R>>(
    level: PersistenceLevel,
    f: impl FnOnce() -> F,
) -> R {
    let _guard = use_persistence_level(level);
    f().await
}

pub struct IdempotenceModeGuard {
    original: bool,
}

impl Drop for IdempotenceModeGuard {
    fn drop(&mut self) {
        set_idempotence_mode(self.original);
    }
}

/// Temporarily sets the idempotence mode to the given value.
///
/// When the returned guard is dropped, the original idempotence mode is restored.
#[must_use]
pub fn use_idempotence_mode(mode: bool) -> IdempotenceModeGuard {
    let original = get_idempotence_mode();
    set_idempotence_mode(mode);
    IdempotenceModeGuard { original }
}

/// Executes the given function with the idempotence mode set to the given value.
pub fn with_idempotence_mode<R>(mode: bool, f: impl FnOnce() -> R) -> R {
    let _guard = use_idempotence_mode(mode);
    f()
}

/// Executes the given async function with the idempotence mode set to the given value.
pub async fn with_idempotence_mode_async<R, F: Future<Output = R>>(
    mode: bool,
    f: impl FnOnce() -> F,
) -> R {
    let _guard = use_idempotence_mode(mode);
    f().await
}

/// Generates an idempotency key. This operation will never be replayed —
/// i.e. not only is this key generated, but it is persisted and committed, such that the key can be used in third-party systems (e.g. payment processing)
/// to introduce idempotence.
pub fn generate_idempotency_key() -> uuid::Uuid {
    Into::into(host_api::generate_idempotency_key())
}

pub struct AtomicOperationGuard {
    begin: OplogIndex,
}

impl Drop for AtomicOperationGuard {
    fn drop(&mut self) {
        // If we're unwinding from a panic, leave the atomic region open so the
        // worker recovery + replay-time fallback in `mark_begin_operation`
        // re-executes the block from the begin marker. WASM panics are wasm
        // traps, so the executor will recover the worker.
        if !std::thread::panicking() {
            mark_end_operation(self.begin);
        }
    }
}

/// Marks a block as an atomic operation
///
/// When the returned guard is dropped, the operation gets committed —
/// unless the current thread is panicking, in which case the region is left
/// open so worker recovery re-executes the block.
#[must_use]
pub fn mark_atomic_operation() -> AtomicOperationGuard {
    let begin = mark_begin_operation();
    AtomicOperationGuard { begin }
}

/// Executes the given function as an atomic operation.
///
/// On panic the region is left open and the worker recovers + re-executes
/// the block. Use [`atomically_result`] when the body returns a `Result` so
/// that error returns also force a trap rather than silently committing the
/// region.
pub fn atomically<T>(f: impl FnOnce() -> T) -> T {
    let _guard = mark_atomic_operation();
    f()
}

/// Executes the given async function as an atomic operation.
///
/// On panic the region is left open and the worker recovers + re-executes
/// the block.
pub async fn atomically_async<T, F: Future<Output = T>>(f: impl FnOnce() -> F) -> T {
    let _guard = mark_atomic_operation();
    f().await
}

/// Executes the given fallible function as an atomic operation.
///
/// On `Ok` the region is committed.
/// On `Err` the SDK calls the host `trap` function, which surfaces as an
/// uncatchable wasm trap so the failure cannot be observed by user code.
/// The atomic region is intentionally left open so the existing replay-time
/// fallback in `mark_begin_operation` deletes the partial inner side effects
/// and re-executes the block.
pub fn atomically_result<T, E>(f: impl FnOnce() -> Result<T, E>) -> Result<T, E>
where
    E: core::fmt::Display,
{
    let guard = mark_atomic_operation();
    match f() {
        Ok(v) => Ok(v),
        Err(e) => {
            // Skip the Drop closing the region — the trap is the terminator.
            core::mem::forget(guard);
            host_api::trap(&format!("atomic block failed: {e}"));
            unreachable!("trap host call must not return")
        }
    }
}

/// Async version of [`atomically_result`].
pub async fn atomically_result_async<T, E, F>(f: impl FnOnce() -> F) -> Result<T, E>
where
    E: core::fmt::Display,
    F: Future<Output = Result<T, E>>,
{
    let guard = mark_atomic_operation();
    match f().await {
        Ok(v) => Ok(v),
        Err(e) => {
            core::mem::forget(guard);
            host_api::trap(&format!("atomic block failed: {e}"));
            unreachable!("trap host call must not return")
        }
    }
}
