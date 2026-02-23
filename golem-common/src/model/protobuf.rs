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

use super::{WorkerResourceDescription, WorkerWasiConfigVarsFilter, diff};
use crate::model::component::{ComponentFileContentHash, ComponentFilePath, InitialComponentFile};
use crate::model::oplog::{OplogIndex, WorkerResourceId};
use crate::model::{
    ComponentFilePermissions, FilterComparator, IdempotencyKey, LogLevel, NumberOfShards, Pod,
    PromiseId, RoutingTable, RoutingTableEntry, ScanCursor, ShardId, StringFilterComparator,
    Timestamp, WorkerCreatedAtFilter, WorkerEnvFilter, WorkerEvent, WorkerFilter, WorkerId,
    WorkerNameFilter, WorkerNotFilter, WorkerRevisionFilter, WorkerStatus, WorkerStatusFilter,
};
use applying::Apply;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::{
    Pod as GrpcPod, RoutingTable as GrpcRoutingTable, RoutingTableEntry as GrpcRoutingTableEntry,
};
use golem_api_grpc::proto::golem::worker::Cursor;
use std::ops::Add;
use std::time::Duration;

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

impl From<diff::Hash> for golem::common::Hash {
    fn from(value: diff::Hash) -> Self {
        Self {
            hash_bytes: value
                .into_blake3()
                .as_bytes()
                .iter()
                .map(|b| *b as u32)
                .collect(),
        }
    }
}

impl TryFrom<golem::common::Hash> for diff::Hash {
    type Error = String;

    fn try_from(value: golem::common::Hash) -> Result<Self, Self::Error> {
        let hash = value
            .hash_bytes
            .into_iter()
            .map(|b| b as u8)
            .collect::<Vec<_>>()
            .apply(|bs| blake3::Hash::from_slice(&bs))
            .map_err(|e| format!("Invalid content hash bytes: {e}"))?;

        Ok(diff::Hash::from(hash))
    }
}

impl From<WorkerId> for golem::worker::WorkerId {
    fn from(value: WorkerId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            name: value.worker_name,
        }
    }
}

impl TryFrom<golem::worker::WorkerId> for WorkerId {
    type Error = String;

    fn try_from(value: golem::worker::WorkerId) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value.component_id.unwrap().try_into()?,
            worker_name: value.name,
        })
    }
}

impl From<PromiseId> for golem::worker::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            worker_id: Some(value.worker_id.into()),
            oplog_idx: value.oplog_idx.into(),
        }
    }
}

impl TryFrom<golem::worker::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(value: golem::worker::PromiseId) -> Result<Self, Self::Error> {
        Ok(Self {
            worker_id: value.worker_id.ok_or("Missing worker_id")?.try_into()?,
            oplog_idx: OplogIndex::from_u64(value.oplog_idx),
        })
    }
}

impl From<ShardId> for golem::shardmanager::ShardId {
    fn from(value: ShardId) -> golem::shardmanager::ShardId {
        golem::shardmanager::ShardId { value: value.value }
    }
}

impl From<golem::shardmanager::ShardId> for ShardId {
    fn from(proto: golem::shardmanager::ShardId) -> Self {
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

impl From<golem::worker::IdempotencyKey> for IdempotencyKey {
    fn from(proto: golem::worker::IdempotencyKey) -> Self {
        Self { value: proto.value }
    }
}

impl From<IdempotencyKey> for golem::worker::IdempotencyKey {
    fn from(value: IdempotencyKey) -> Self {
        Self { value: value.value }
    }
}

impl From<WorkerStatus> for golem::worker::WorkerStatus {
    fn from(value: WorkerStatus) -> Self {
        match value {
            WorkerStatus::Running => golem::worker::WorkerStatus::Running,
            WorkerStatus::Idle => golem::worker::WorkerStatus::Idle,
            WorkerStatus::Suspended => golem::worker::WorkerStatus::Suspended,
            WorkerStatus::Interrupted => golem::worker::WorkerStatus::Interrupted,
            WorkerStatus::Retrying => golem::worker::WorkerStatus::Retrying,
            WorkerStatus::Failed => golem::worker::WorkerStatus::Failed,
            WorkerStatus::Exited => golem::worker::WorkerStatus::Exited,
        }
    }
}

impl TryFrom<golem::worker::WorkerFilter> for WorkerFilter {
    type Error = String;

    fn try_from(value: golem::worker::WorkerFilter) -> Result<Self, Self::Error> {
        match value.filter {
            Some(filter) => match filter {
                golem::worker::worker_filter::Filter::Name(filter) => Ok(WorkerFilter::new_name(
                    filter.comparator.try_into()?,
                    filter.value,
                )),
                golem::worker::worker_filter::Filter::Revision(filter) => {
                    Ok(WorkerFilter::new_revision(
                        filter.comparator.try_into()?,
                        filter.value.try_into()?,
                    ))
                }
                golem::worker::worker_filter::Filter::Status(filter) => {
                    Ok(WorkerFilter::new_status(
                        filter.comparator.try_into()?,
                        filter.value.try_into()?,
                    ))
                }
                golem::worker::worker_filter::Filter::CreatedAt(filter) => {
                    let value = filter
                        .value
                        .map(|t| t.into())
                        .ok_or_else(|| "Missing value".to_string())?;
                    Ok(WorkerFilter::new_created_at(
                        filter.comparator.try_into()?,
                        value,
                    ))
                }
                golem::worker::worker_filter::Filter::Env(filter) => Ok(WorkerFilter::new_env(
                    filter.name,
                    filter.comparator.try_into()?,
                    filter.value,
                )),
                golem::worker::worker_filter::Filter::WasiConfigVars(filter) => {
                    Ok(WorkerFilter::new_wasi_config_vars(
                        filter.name,
                        filter.comparator.try_into()?,
                        filter.value,
                    ))
                }
                golem::worker::worker_filter::Filter::Not(filter) => {
                    let filter = *filter.filter.ok_or_else(|| "Missing filter".to_string())?;
                    Ok(WorkerFilter::new_not(filter.try_into()?))
                }
                golem::worker::worker_filter::Filter::And(golem::worker::WorkerAndFilter {
                    filters,
                }) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<WorkerFilter>,
                        String,
                    >>(
                    )?;

                    Ok(WorkerFilter::new_and(filters))
                }
                golem::worker::worker_filter::Filter::Or(golem::worker::WorkerOrFilter {
                    filters,
                }) => {
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

impl From<WorkerFilter> for golem::worker::WorkerFilter {
    fn from(value: WorkerFilter) -> Self {
        let filter = match value {
            WorkerFilter::Name(WorkerNameFilter { comparator, value }) => {
                golem::worker::worker_filter::Filter::Name(golem::worker::WorkerNameFilter {
                    comparator: comparator.into(),
                    value,
                })
            }
            WorkerFilter::Revision(WorkerRevisionFilter { comparator, value }) => {
                golem::worker::worker_filter::Filter::Revision(
                    golem::worker::WorkerRevisionFilter {
                        comparator: comparator.into(),
                        value: value.into(),
                    },
                )
            }
            WorkerFilter::Env(WorkerEnvFilter {
                name,
                comparator,
                value,
            }) => golem::worker::worker_filter::Filter::Env(golem::worker::WorkerEnvFilter {
                name,
                comparator: comparator.into(),
                value,
            }),
            WorkerFilter::WasiConfigVars(WorkerWasiConfigVarsFilter {
                name,
                comparator,
                value,
            }) => golem::worker::worker_filter::Filter::WasiConfigVars(
                golem::worker::WorkerWasiConfigVarsFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                },
            ),
            WorkerFilter::Status(WorkerStatusFilter { comparator, value }) => {
                golem::worker::worker_filter::Filter::Status(golem::worker::WorkerStatusFilter {
                    comparator: comparator.into(),
                    value: value.into(),
                })
            }
            WorkerFilter::CreatedAt(WorkerCreatedAtFilter { comparator, value }) => {
                golem::worker::worker_filter::Filter::CreatedAt(
                    golem::worker::WorkerCreatedAtFilter {
                        value: Some(value.into()),
                        comparator: comparator.into(),
                    },
                )
            }
            WorkerFilter::Not(WorkerNotFilter { filter }) => {
                let f: golem::worker::WorkerFilter = (*filter).into();
                golem::worker::worker_filter::Filter::Not(Box::new(
                    golem::worker::WorkerNotFilter {
                        filter: Some(Box::new(f)),
                    },
                ))
            }
            WorkerFilter::And(filter) => {
                golem::worker::worker_filter::Filter::And(golem::worker::WorkerAndFilter {
                    filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                })
            }
            WorkerFilter::Or(filter) => {
                golem::worker::worker_filter::Filter::Or(golem::worker::WorkerOrFilter {
                    filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                })
            }
        };

        golem::worker::WorkerFilter {
            filter: Some(filter),
        }
    }
}

impl From<StringFilterComparator> for golem::common::StringFilterComparator {
    fn from(value: StringFilterComparator) -> Self {
        match value {
            StringFilterComparator::Equal => golem::common::StringFilterComparator::StringEqual,
            StringFilterComparator::NotEqual => {
                golem::common::StringFilterComparator::StringNotEqual
            }
            StringFilterComparator::Like => golem::common::StringFilterComparator::StringLike,
            StringFilterComparator::NotLike => golem::common::StringFilterComparator::StringNotLike,
            StringFilterComparator::StartsWith => golem::common::StringFilterComparator::StartsWith,
        }
    }
}

impl From<FilterComparator> for golem::common::FilterComparator {
    fn from(value: FilterComparator) -> Self {
        match value {
            FilterComparator::Equal => golem::common::FilterComparator::Equal,
            FilterComparator::NotEqual => golem::common::FilterComparator::NotEqual,
            FilterComparator::Less => golem::common::FilterComparator::Less,
            FilterComparator::LessEqual => golem::common::FilterComparator::LessEqual,
            FilterComparator::Greater => golem::common::FilterComparator::Greater,
            FilterComparator::GreaterEqual => golem::common::FilterComparator::GreaterEqual,
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

impl From<golem::worker::Level> for LogLevel {
    fn from(value: golem::worker::Level) -> Self {
        match value {
            golem::worker::Level::Trace => LogLevel::Trace,
            golem::worker::Level::Debug => LogLevel::Debug,
            golem::worker::Level::Info => LogLevel::Info,
            golem::worker::Level::Warn => LogLevel::Warn,
            golem::worker::Level::Error => LogLevel::Error,
            golem::worker::Level::Critical => LogLevel::Critical,
        }
    }
}

impl From<LogLevel> for golem::worker::Level {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => golem::worker::Level::Trace,
            LogLevel::Debug => golem::worker::Level::Debug,
            LogLevel::Info => golem::worker::Level::Info,
            LogLevel::Warn => golem::worker::Level::Warn,
            LogLevel::Error => golem::worker::Level::Error,
            LogLevel::Critical => golem::worker::Level::Critical,
        }
    }
}

impl TryFrom<golem::worker::LogEvent> for WorkerEvent {
    type Error = String;

    fn try_from(value: golem::worker::LogEvent) -> Result<Self, Self::Error> {
        match value.event {
            Some(event) => match event {
                golem::worker::log_event::Event::Stdout(event) => Ok(WorkerEvent::StdOut {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    bytes: event.message.into_bytes(),
                }),
                golem::worker::log_event::Event::Stderr(event) => Ok(WorkerEvent::StdErr {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    bytes: event.message.into_bytes(),
                }),
                golem::worker::log_event::Event::Log(event) => Ok(WorkerEvent::Log {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    level: event.level().into(),
                    context: event.context,
                    message: event.message,
                }),
                golem::worker::log_event::Event::InvocationStarted(event) => {
                    Ok(WorkerEvent::InvocationStart {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        function: event.function,
                        idempotency_key: event
                            .idempotency_key
                            .ok_or("Missing idempotency key")?
                            .into(),
                    })
                }
                golem::worker::log_event::Event::InvocationFinished(event) => {
                    Ok(WorkerEvent::InvocationFinished {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        function: event.function,
                        idempotency_key: event
                            .idempotency_key
                            .ok_or("Missing idempotency key")?
                            .into(),
                    })
                }
                golem::worker::log_event::Event::ClientLagged(event) => {
                    Ok(WorkerEvent::ClientLagged {
                        number_of_missed_messages: event.number_of_missed_messages,
                    })
                }
            },
            None => Err("Missing event".to_string()),
        }
    }
}

impl TryFrom<WorkerEvent> for golem::worker::LogEvent {
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
                event: Some(golem::worker::log_event::Event::Stderr(
                    golem::worker::StdErrLog {
                        message: String::from_utf8_lossy(&bytes).to_string(),
                        timestamp: Some(timestamp.into()),
                    },
                )),
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

impl From<golem::component::ComponentFilePermissions> for ComponentFilePermissions {
    fn from(value: golem::component::ComponentFilePermissions) -> Self {
        match value {
            golem::component::ComponentFilePermissions::ReadOnly => {
                ComponentFilePermissions::ReadOnly
            }
            golem::component::ComponentFilePermissions::ReadWrite => {
                ComponentFilePermissions::ReadWrite
            }
        }
    }
}

impl From<ComponentFilePermissions> for golem::component::ComponentFilePermissions {
    fn from(value: ComponentFilePermissions) -> Self {
        match value {
            ComponentFilePermissions::ReadOnly => {
                golem::component::ComponentFilePermissions::ReadOnly
            }
            ComponentFilePermissions::ReadWrite => {
                golem::component::ComponentFilePermissions::ReadWrite
            }
        }
    }
}

impl From<InitialComponentFile> for golem::component::InitialComponentFile {
    fn from(value: InitialComponentFile) -> Self {
        let permissions: golem::component::ComponentFilePermissions = value.permissions.into();
        Self {
            content_hash: Some(value.content_hash.0.into()),
            path: value.path.to_string(),
            permissions: permissions.into(),
        }
    }
}

impl TryFrom<golem::component::InitialComponentFile> for InitialComponentFile {
    type Error = String;

    fn try_from(value: golem::component::InitialComponentFile) -> Result<Self, Self::Error> {
        let permissions: golem::component::ComponentFilePermissions = value
            .permissions
            .try_into()
            .map_err(|e| format!("Failed converting permissions {e}"))?;
        let permissions: ComponentFilePermissions = permissions.into();
        let path = ComponentFilePath::from_abs_str(&value.path).map_err(|e| e.to_string())?;
        let content_hash: diff::Hash = value
            .content_hash
            .ok_or("Missing content_hash field")?
            .try_into()?;

        Ok(Self {
            content_hash: ComponentFileContentHash(content_hash),
            path,
            permissions,
        })
    }
}

pub fn to_protobuf_resource_description(
    key: WorkerResourceId,
    description: WorkerResourceDescription,
) -> golem::worker::ResourceDescription {
    golem::worker::ResourceDescription {
        created_at: Some(description.created_at.into()),
        resource_id: key.0,
        resource_owner: description.resource_owner,
        resource_name: description.resource_name,
    }
}

pub fn from_protobuf_resource_description(
    description: golem::worker::ResourceDescription,
) -> Result<(WorkerResourceId, WorkerResourceDescription), String> {
    let key = WorkerResourceId(description.resource_id);
    let value = WorkerResourceDescription {
        created_at: description.created_at.ok_or("Missing created_at")?.into(),
        resource_owner: description.resource_owner,
        resource_name: description.resource_name,
    };
    Ok((key, value))
}
