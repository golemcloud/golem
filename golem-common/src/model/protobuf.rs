// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use super::{
    AgentInstanceDescription, AgentInstanceKey, ExportedResourceInstanceDescription,
    ExportedResourceInstanceKey, WorkerResourceDescription, WorkerResourceKey,
    WorkerWasiConfigVarsFilter,
};
use crate::model::oplog::{OplogIndex, WorkerResourceId};
use crate::model::{
    AccountId, ComponentFilePath, ComponentFilePermissions, ComponentFileSystemNode,
    ComponentFileSystemNodeDetails, ComponentType, FilterComparator, IdempotencyKey,
    InitialComponentFile, InitialComponentFileKey, LogLevel, NumberOfShards, Pod, PromiseId,
    RoutingTable, RoutingTableEntry, ScanCursor, ShardId, StringFilterComparator, TargetWorkerId,
    Timestamp, WorkerCreatedAtFilter, WorkerEnvFilter, WorkerEvent, WorkerFilter, WorkerId,
    WorkerNameFilter, WorkerNotFilter, WorkerStatus, WorkerStatusFilter, WorkerVersionFilter,
};
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::{
    Pod as GrpcPod, RoutingTable as GrpcRoutingTable, RoutingTableEntry as GrpcRoutingTableEntry,
};
use golem_api_grpc::proto::golem::worker::Cursor;
use std::ops::Add;
use std::time::{Duration, SystemTime};

impl From<Timestamp> for prost_types::Timestamp {
    fn from(value: Timestamp) -> Self {
        let d = value
            .0
            .duration_since(iso8601_timestamp::Timestamp::UNIX_EPOCH);
        Self {
            seconds: d.whole_seconds(),
            nanos: d.subsec_nanoseconds(),
        }
    }
}

impl From<prost_types::Timestamp> for Timestamp {
    fn from(value: prost_types::Timestamp) -> Self {
        Timestamp(
            iso8601_timestamp::Timestamp::UNIX_EPOCH
                .add(Duration::new(value.seconds as u64, value.nanos as u32)),
        )
    }
}

impl From<WorkerId> for golem_api_grpc::proto::golem::worker::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            name: value.worker_name,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerId> for WorkerId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value.component_id.unwrap().try_into()?,
            worker_name: value.name,
        })
    }
}

impl TryFrom<golem::worker::TargetWorkerId> for TargetWorkerId {
    type Error = String;

    fn try_from(value: golem::worker::TargetWorkerId) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value
                .component_id
                .ok_or("Missing component_id")?
                .try_into()?,
            worker_name: value.name,
        })
    }
}

impl From<TargetWorkerId> for golem::worker::TargetWorkerId {
    fn from(value: TargetWorkerId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            name: value.worker_name,
        }
    }
}

impl From<PromiseId> for golem_api_grpc::proto::golem::worker::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            oplog_idx: value.oplog_idx.into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::PromiseId,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            oplog_idx: OplogIndex::from_u64(value.oplog_idx),
        })
    }
}

impl From<ShardId> for golem_api_grpc::proto::golem::shardmanager::ShardId {
    fn from(value: ShardId) -> golem_api_grpc::proto::golem::shardmanager::ShardId {
        golem_api_grpc::proto::golem::shardmanager::ShardId { value: value.value }
    }
}

impl From<golem_api_grpc::proto::golem::shardmanager::ShardId> for ShardId {
    fn from(proto: golem_api_grpc::proto::golem::shardmanager::ShardId) -> Self {
        Self { value: proto.value }
    }
}

impl From<GrpcPod> for Pod {
    fn from(value: GrpcPod) -> Self {
        Self {
            host: value.host,
            port: value.port as u16,
        }
    }
}

impl From<GrpcRoutingTableEntry> for RoutingTableEntry {
    fn from(value: GrpcRoutingTableEntry) -> Self {
        Self {
            shard_id: value.shard_id.unwrap().into(),
            pod: value.pod.unwrap().into(),
        }
    }
}

impl From<GrpcRoutingTable> for RoutingTable {
    fn from(value: GrpcRoutingTable) -> Self {
        Self {
            number_of_shards: NumberOfShards {
                value: value.number_of_shards as usize,
            },
            shard_assignments: value
                .shard_assignments
                .into_iter()
                .map(RoutingTableEntry::from)
                .map(|routing_table_entry| (routing_table_entry.shard_id, routing_table_entry.pod))
                .collect(),
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::IdempotencyKey> for IdempotencyKey {
    fn from(proto: golem_api_grpc::proto::golem::worker::IdempotencyKey) -> Self {
        Self { value: proto.value }
    }
}

impl From<IdempotencyKey> for golem_api_grpc::proto::golem::worker::IdempotencyKey {
    fn from(value: IdempotencyKey) -> Self {
        Self { value: value.value }
    }
}

impl From<WorkerStatus> for golem_api_grpc::proto::golem::worker::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem_api_grpc::proto::golem::worker::WorkerStatus::Running,
            WorkerStatus::Idle => golem_api_grpc::proto::golem::worker::WorkerStatus::Idle,
            WorkerStatus::Suspended => {
                golem_api_grpc::proto::golem::worker::WorkerStatus::Suspended
            }
            WorkerStatus::Interrupted => {
                golem_api_grpc::proto::golem::worker::WorkerStatus::Interrupted
            }
            WorkerStatus::Retrying => golem_api_grpc::proto::golem::worker::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem_api_grpc::proto::golem::worker::WorkerStatus::Failed,
            WorkerStatus::Exited => golem_api_grpc::proto::golem::worker::WorkerStatus::Exited,
        }
    }
}

impl From<golem_api_grpc::proto::golem::common::AccountId> for AccountId {
    fn from(proto: golem_api_grpc::proto::golem::common::AccountId) -> Self {
        Self { value: proto.name }
    }
}

impl From<AccountId> for golem_api_grpc::proto::golem::common::AccountId {
    fn from(value: AccountId) -> Self {
        golem_api_grpc::proto::golem::common::AccountId { name: value.value }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerFilter> for WorkerFilter {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerFilter,
    ) -> Result<Self, Self::Error> {
        match value.filter {
            Some(filter) => match filter {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Name(filter) => Ok(
                    WorkerFilter::new_name(filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Version(filter) => Ok(
                    WorkerFilter::new_version(filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Status(filter) => {
                    Ok(WorkerFilter::new_status(
                        filter.comparator.try_into()?,
                        filter.value.try_into()?,
                    ))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::CreatedAt(filter) => {
                    let value = filter
                        .value
                        .map(|t| t.into())
                        .ok_or_else(|| "Missing value".to_string())?;
                    Ok(WorkerFilter::new_created_at(
                        filter.comparator.try_into()?,
                        value,
                    ))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Env(filter) => Ok(
                    WorkerFilter::new_env(filter.name, filter.comparator.try_into()?, filter.value),
                ),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::WasiConfigVars(
                    filter,
                ) => Ok(WorkerFilter::new_wasi_config_vars(
                    filter.name,
                    filter.comparator.try_into()?,
                    filter.value,
                )),
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Not(filter) => {
                    let filter = *filter.filter.ok_or_else(|| "Missing filter".to_string())?;
                    Ok(WorkerFilter::new_not(filter.try_into()?))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::And(
                    golem_api_grpc::proto::golem::worker::WorkerAndFilter { filters },
                ) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<WorkerFilter>,
                        String,
                    >>(
                    )?;

                    Ok(WorkerFilter::new_and(filters))
                }
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Or(
                    golem_api_grpc::proto::golem::worker::WorkerOrFilter { filters },
                ) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<WorkerFilter>,
                        String,
                    >>(
                    )?;

                    Ok(WorkerFilter::new_or(filters))
                }
            },
            None => Err("Missing filter".to_string()),
        }
    }
}

impl From<WorkerFilter> for golem_api_grpc::proto::golem::worker::WorkerFilter {
    fn from(value: WorkerFilter) -> Self {
        let filter = match value {
            WorkerFilter::Name(WorkerNameFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Name(
                    golem_api_grpc::proto::golem::worker::WorkerNameFilter {
                        comparator: comparator.into(),
                        value,
                    },
                )
            }
            WorkerFilter::Version(WorkerVersionFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Version(
                    golem_api_grpc::proto::golem::worker::WorkerVersionFilter {
                        comparator: comparator.into(),
                        value,
                    },
                )
            }
            WorkerFilter::Env(WorkerEnvFilter {
                name,
                comparator,
                value,
            }) => golem_api_grpc::proto::golem::worker::worker_filter::Filter::Env(
                golem_api_grpc::proto::golem::worker::WorkerEnvFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                },
            ),
            WorkerFilter::WasiConfigVars(WorkerWasiConfigVarsFilter {
                name,
                comparator,
                value,
            }) => golem_api_grpc::proto::golem::worker::worker_filter::Filter::WasiConfigVars(
                golem_api_grpc::proto::golem::worker::WorkerWasiConfigVarsFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                },
            ),
            WorkerFilter::Status(WorkerStatusFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Status(
                    golem_api_grpc::proto::golem::worker::WorkerStatusFilter {
                        comparator: comparator.into(),
                        value: value.into(),
                    },
                )
            }
            WorkerFilter::CreatedAt(WorkerCreatedAtFilter { comparator, value }) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::CreatedAt(
                    golem_api_grpc::proto::golem::worker::WorkerCreatedAtFilter {
                        value: Some(value.into()),
                        comparator: comparator.into(),
                    },
                )
            }
            WorkerFilter::Not(WorkerNotFilter { filter }) => {
                let f: golem_api_grpc::proto::golem::worker::WorkerFilter = (*filter).into();
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Not(Box::new(
                    golem_api_grpc::proto::golem::worker::WorkerNotFilter {
                        filter: Some(Box::new(f)),
                    },
                ))
            }
            WorkerFilter::And(filter) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::And(
                    golem_api_grpc::proto::golem::worker::WorkerAndFilter {
                        filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                    },
                )
            }
            WorkerFilter::Or(filter) => {
                golem_api_grpc::proto::golem::worker::worker_filter::Filter::Or(
                    golem_api_grpc::proto::golem::worker::WorkerOrFilter {
                        filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                    },
                )
            }
        };

        golem_api_grpc::proto::golem::worker::WorkerFilter {
            filter: Some(filter),
        }
    }
}

impl From<StringFilterComparator> for golem_api_grpc::proto::golem::common::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringEqual
            }
            StringFilterComparator::NotEqual => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringNotEqual
            }
            StringFilterComparator::Like => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringLike
            }
            StringFilterComparator::NotLike => {
                golem_api_grpc::proto::golem::common::StringFilterComparator::StringNotLike
            }
        }
    }
}

impl From<FilterComparator> for golem_api_grpc::proto::golem::common::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => {
                golem_api_grpc::proto::golem::common::FilterComparator::Equal
            }
            FilterComparator::NotEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::NotEqual
            }
            FilterComparator::Less => golem_api_grpc::proto::golem::common::FilterComparator::Less,
            FilterComparator::LessEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::LessEqual
            }
            FilterComparator::Greater => {
                golem_api_grpc::proto::golem::common::FilterComparator::Greater
            }
            FilterComparator::GreaterEqual => {
                golem_api_grpc::proto::golem::common::FilterComparator::GreaterEqual
            }
        }
    }
}

impl From<Cursor> for ScanCursor {
    fn from(value: Cursor) -> Self {
        Self {
            cursor: value.cursor,
            layer: value.layer as usize,
        }
    }
}

impl From<ScanCursor> for Cursor {
    fn from(value: ScanCursor) -> Self {
        Self {
            cursor: value.cursor,
            layer: value.layer as u64,
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::Level> for LogLevel {
    fn from(value: golem_api_grpc::proto::golem::worker::Level) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::Level::Trace => LogLevel::Trace,
            golem_api_grpc::proto::golem::worker::Level::Debug => LogLevel::Debug,
            golem_api_grpc::proto::golem::worker::Level::Info => LogLevel::Info,
            golem_api_grpc::proto::golem::worker::Level::Warn => LogLevel::Warn,
            golem_api_grpc::proto::golem::worker::Level::Error => LogLevel::Error,
            golem_api_grpc::proto::golem::worker::Level::Critical => LogLevel::Critical,
        }
    }
}

impl From<LogLevel> for golem_api_grpc::proto::golem::worker::Level {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => golem_api_grpc::proto::golem::worker::Level::Trace,
            LogLevel::Debug => golem_api_grpc::proto::golem::worker::Level::Debug,
            LogLevel::Info => golem_api_grpc::proto::golem::worker::Level::Info,
            LogLevel::Warn => golem_api_grpc::proto::golem::worker::Level::Warn,
            LogLevel::Error => golem_api_grpc::proto::golem::worker::Level::Error,
            LogLevel::Critical => golem_api_grpc::proto::golem::worker::Level::Critical,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::LogEvent> for WorkerEvent {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::LogEvent,
    ) -> Result<Self, Self::Error> {
        match value.event {
            Some(event) => match event {
                golem_api_grpc::proto::golem::worker::log_event::Event::Stdout(event) => {
                    Ok(WorkerEvent::StdOut {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        bytes: event.message.into_bytes(),
                    })
                }
                golem_api_grpc::proto::golem::worker::log_event::Event::Stderr(event) => {
                    Ok(WorkerEvent::StdErr {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        bytes: event.message.into_bytes(),
                    })
                }
                golem_api_grpc::proto::golem::worker::log_event::Event::Log(event) => {
                    Ok(WorkerEvent::Log {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        level: event.level().into(),
                        context: event.context,
                        message: event.message,
                    })
                }
                golem_api_grpc::proto::golem::worker::log_event::Event::InvocationStarted(
                    event,
                ) => Ok(WorkerEvent::InvocationStart {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    function: event.function,
                    idempotency_key: event
                        .idempotency_key
                        .ok_or("Missing idempotency key")?
                        .into(),
                }),
                golem_api_grpc::proto::golem::worker::log_event::Event::InvocationFinished(
                    event,
                ) => Ok(WorkerEvent::InvocationFinished {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    function: event.function,
                    idempotency_key: event
                        .idempotency_key
                        .ok_or("Missing idempotency key")?
                        .into(),
                }),
                golem_api_grpc::proto::golem::worker::log_event::Event::ClientLagged(event) => {
                    Ok(WorkerEvent::ClientLagged {
                        number_of_missed_messages: event.number_of_missed_messages,
                    })
                }
            },
            None => Err("Missing event".to_string()),
        }
    }
}

impl TryFrom<WorkerEvent> for golem_api_grpc::proto::golem::worker::LogEvent {
    type Error = String;

    fn try_from(value: WorkerEvent) -> Result<Self, Self::Error> {
        match value {
            WorkerEvent::StdOut { timestamp, bytes } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::Stdout(
                    golem::worker::StdOutLog {
                        message: String::from_utf8_lossy(&bytes).to_string(),
                        timestamp: Some(timestamp.into()),
                    },
                )),
            }),
            WorkerEvent::StdErr { timestamp, bytes } => Ok(golem::worker::LogEvent {
                event: Some(
                    golem_api_grpc::proto::golem::worker::log_event::Event::Stderr(
                        golem::worker::StdErrLog {
                            message: String::from_utf8_lossy(&bytes).to_string(),
                            timestamp: Some(timestamp.into()),
                        },
                    ),
                ),
            }),
            WorkerEvent::Log {
                timestamp,
                level,
                context,
                message,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::Log(golem::worker::Log {
                    level: match level {
                        LogLevel::Trace => golem::worker::Level::Trace.into(),
                        LogLevel::Debug => golem::worker::Level::Debug.into(),
                        LogLevel::Info => golem::worker::Level::Info.into(),
                        LogLevel::Warn => golem::worker::Level::Warn.into(),
                        LogLevel::Error => golem::worker::Level::Error.into(),
                        LogLevel::Critical => golem::worker::Level::Critical.into(),
                    },
                    context,
                    message,
                    timestamp: Some(timestamp.into()),
                })),
            }),
            WorkerEvent::InvocationStart {
                timestamp,
                function,
                idempotency_key,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::InvocationStarted(
                    golem::worker::InvocationStarted {
                        function,
                        idempotency_key: Some(idempotency_key.into()),
                        timestamp: Some(timestamp.into()),
                    },
                )),
            }),
            WorkerEvent::InvocationFinished {
                timestamp,
                function,
                idempotency_key,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::InvocationFinished(
                    golem::worker::InvocationFinished {
                        function,
                        idempotency_key: Some(idempotency_key.into()),
                        timestamp: Some(timestamp.into()),
                    },
                )),
            }),
            WorkerEvent::ClientLagged {
                number_of_missed_messages,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::ClientLagged(
                    golem::worker::ClientLagged {
                        number_of_missed_messages,
                    },
                )),
            }),
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::ComponentType> for ComponentType {
    fn from(value: golem_api_grpc::proto::golem::component::ComponentType) -> Self {
        match value {
            golem_api_grpc::proto::golem::component::ComponentType::Durable => {
                ComponentType::Durable
            }
            golem_api_grpc::proto::golem::component::ComponentType::Ephemeral => {
                ComponentType::Ephemeral
            }
        }
    }
}

impl From<ComponentType> for golem_api_grpc::proto::golem::component::ComponentType {
    fn from(value: ComponentType) -> Self {
        match value {
            ComponentType::Durable => {
                golem_api_grpc::proto::golem::component::ComponentType::Durable
            }
            ComponentType::Ephemeral => {
                golem_api_grpc::proto::golem::component::ComponentType::Ephemeral
            }
        }
    }
}

impl From<golem_api_grpc::proto::golem::component::ComponentFilePermissions>
    for ComponentFilePermissions
{
    fn from(value: golem_api_grpc::proto::golem::component::ComponentFilePermissions) -> Self {
        match value {
            golem_api_grpc::proto::golem::component::ComponentFilePermissions::ReadOnly => {
                ComponentFilePermissions::ReadOnly
            }
            golem_api_grpc::proto::golem::component::ComponentFilePermissions::ReadWrite => {
                ComponentFilePermissions::ReadWrite
            }
        }
    }
}

impl From<ComponentFilePermissions>
    for golem_api_grpc::proto::golem::component::ComponentFilePermissions
{
    fn from(value: ComponentFilePermissions) -> Self {
        match value {
            ComponentFilePermissions::ReadOnly => {
                golem_api_grpc::proto::golem::component::ComponentFilePermissions::ReadOnly
            }
            ComponentFilePermissions::ReadWrite => {
                golem_api_grpc::proto::golem::component::ComponentFilePermissions::ReadWrite
            }
        }
    }
}

impl From<InitialComponentFile> for golem_api_grpc::proto::golem::component::InitialComponentFile {
    fn from(value: InitialComponentFile) -> Self {
        let permissions: golem_api_grpc::proto::golem::component::ComponentFilePermissions =
            value.permissions.into();
        Self {
            key: value.key.0,
            path: value.path.to_string(),
            permissions: permissions.into(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::component::InitialComponentFile>
    for InitialComponentFile
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::component::InitialComponentFile,
    ) -> Result<Self, Self::Error> {
        let permissions: golem_api_grpc::proto::golem::component::ComponentFilePermissions = value
            .permissions
            .try_into()
            .map_err(|e| format!("Failed converting permissions {e}"))?;
        let permissions: ComponentFilePermissions = permissions.into();
        let path = ComponentFilePath::from_abs_str(&value.path).map_err(|e| e.to_string())?;
        let key = InitialComponentFileKey(value.key);
        Ok(Self {
            key,
            path,
            permissions,
        })
    }
}

impl From<ComponentFileSystemNode> for golem_api_grpc::proto::golem::worker::FileSystemNode {
    fn from(value: ComponentFileSystemNode) -> Self {
        let last_modified = value
            .last_modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        match value.details {
            ComponentFileSystemNodeDetails::File { permissions, size } =>
                golem_api_grpc::proto::golem::worker::FileSystemNode {
                    value: Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::File(
                        golem_api_grpc::proto::golem::worker::FileFileSystemNode {
                            name: value.name,
                            last_modified,
                            size,
                            permissions:
                            golem_api_grpc::proto::golem::component::ComponentFilePermissions::from(permissions).into(),
                        }
                    ))
                },
            ComponentFileSystemNodeDetails::Directory =>
                golem_api_grpc::proto::golem::worker::FileSystemNode {
                    value: Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::Directory(
                        golem_api_grpc::proto::golem::worker::DirectoryFileSystemNode {
                            name: value.name,
                            last_modified,
                        }
                    ))
                }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::FileSystemNode> for ComponentFileSystemNode {
    type Error = anyhow::Error;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::FileSystemNode,
    ) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::Directory(
                golem_api_grpc::proto::golem::worker::DirectoryFileSystemNode {
                    name,
                    last_modified,
                },
            )) => Ok(ComponentFileSystemNode {
                name,
                last_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(last_modified),
                details: ComponentFileSystemNodeDetails::Directory,
            }),
            Some(golem_api_grpc::proto::golem::worker::file_system_node::Value::File(
                golem_api_grpc::proto::golem::worker::FileFileSystemNode {
                    name,
                    last_modified,
                    size,
                    permissions,
                },
            )) => Ok(ComponentFileSystemNode {
                name,
                last_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(last_modified),
                details: ComponentFileSystemNodeDetails::File {
                    permissions:
                        golem_api_grpc::proto::golem::component::ComponentFilePermissions::try_from(
                            permissions,
                        )?
                        .into(),
                    size,
                },
            }),
            None => Err(anyhow::anyhow!("Missing value")),
        }
    }
}

pub fn to_protobuf_resource_description(
    key: WorkerResourceKey,
    description: WorkerResourceDescription,
) -> Result<golem::worker::ResourceDescription, String> {
    match (key, description) {
        (
            WorkerResourceKey::ExportedResourceInstanceKey(key),
            WorkerResourceDescription::ExportedResourceInstance(description),
        ) => Ok(golem::worker::ResourceDescription {
            description: Some(
                golem::worker::resource_description::Description::ExportedResourceInstance(
                    golem::worker::ExportedResourceInstanceDescription {
                        created_at: Some(description.created_at.into()),
                        resource_id: key.resource_id.0,
                        resource_owner: description.resource_owner,
                        resource_name: description.resource_name,
                        is_indexed: description.resource_params.is_some(),
                        resource_params: description.resource_params.unwrap_or_default(),
                    },
                ),
            ),
        }),
        (
            WorkerResourceKey::AgentInstanceKey(key),
            WorkerResourceDescription::AgentInstance(description),
        ) => Ok(golem::worker::ResourceDescription {
            description: Some(
                golem::worker::resource_description::Description::AgentInstance(
                    golem::worker::AgentInstanceDescription {
                        created_at: Some(description.created_at.into()),
                        agent_type: key.agent_type,
                        agent_id: key.agent_id,
                        agent_parameters: Some(description.agent_parameters.into()),
                    },
                ),
            ),
        }),
        _ => Err("Invalid key/description combination".to_string()),
    }
}

pub fn from_protobuf_resource_description(
    description: golem::worker::ResourceDescription,
) -> Result<(WorkerResourceKey, WorkerResourceDescription), String> {
    match description.description {
        Some(golem::worker::resource_description::Description::ExportedResourceInstance(
            golem::worker::ExportedResourceInstanceDescription {
                created_at,
                resource_id,
                resource_owner,
                resource_name,
                is_indexed,
                resource_params,
            },
        )) => Ok((
            WorkerResourceKey::ExportedResourceInstanceKey(ExportedResourceInstanceKey {
                resource_id: WorkerResourceId(resource_id),
            }),
            WorkerResourceDescription::ExportedResourceInstance(
                ExportedResourceInstanceDescription {
                    created_at: created_at.ok_or("Missing created_at")?.into(),
                    resource_owner,
                    resource_name,
                    resource_params: if is_indexed {
                        Some(resource_params)
                    } else {
                        None
                    },
                },
            ),
        )),
        Some(golem::worker::resource_description::Description::AgentInstance(
            golem::worker::AgentInstanceDescription {
                created_at,
                agent_type,
                agent_id,
                agent_parameters,
            },
        )) => Ok((
            WorkerResourceKey::AgentInstanceKey(AgentInstanceKey {
                agent_type,
                agent_id,
            }),
            WorkerResourceDescription::AgentInstance(AgentInstanceDescription {
                created_at: created_at.ok_or("Missing created_at")?.into(),
                agent_parameters: agent_parameters
                    .ok_or("Missing agent_parameters")?
                    .try_into()?,
            }),
        )),
        None => Err("Missing description".to_string()),
    }
}
