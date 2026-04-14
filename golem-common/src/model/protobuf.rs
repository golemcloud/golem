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

use super::{AgentConfigVarsFilter, AgentResourceDescription, InvocationStatus, diff};
use crate::base_model::agent::AgentFileContentHash;
use crate::model::component::{AgentFilePath, InitialAgentFile};
use crate::model::oplog::{AgentResourceId, OplogIndex};
use crate::model::{
    AgentCreatedAtFilter, AgentEnvFilter, AgentEvent, AgentFilePermissions, AgentFilter, AgentId,
    AgentNameFilter, AgentNotFilter, AgentRevisionFilter, AgentStatus, AgentStatusFilter,
    FilterComparator, IdempotencyKey, LogLevel, NumberOfShards, Pod, PromiseId, RoutingTable,
    RoutingTableEntry, ScanCursor, ShardId, StringFilterComparator, Timestamp,
};
use applying::Apply;
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::{
    IpAddress as GrpcIpAddress, Pod as GrpcPod, RoutingTable as GrpcRoutingTable,
    RoutingTableEntry as GrpcRoutingTableEntry,
};
use golem_api_grpc::proto::golem::worker::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
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

impl From<AgentId> for golem::worker::AgentId {
    fn from(value: AgentId) -> Self {
        Self {
            component_id: Some(value.component_id.into()),
            name: value.agent_id,
        }
    }
}

impl TryFrom<golem::worker::AgentId> for AgentId {
    type Error = String;

    fn try_from(value: golem::worker::AgentId) -> Result<Self, Self::Error> {
        Ok(Self {
            component_id: value.component_id.unwrap().try_into()?,
            agent_id: value.name,
        })
    }
}

impl From<PromiseId> for golem::worker::PromiseId {
    fn from(value: PromiseId) -> Self {
        Self {
            agent_id: Some(value.agent_id.into()),
            oplog_idx: value.oplog_idx.into(),
        }
    }
}

impl TryFrom<golem::worker::PromiseId> for PromiseId {
    type Error = String;

    fn try_from(value: golem::worker::PromiseId) -> Result<Self, Self::Error> {
        Ok(Self {
            agent_id: value.agent_id.ok_or("Missing agent_id")?.try_into()?,
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

impl From<Pod> for GrpcPod {
    fn from(value: Pod) -> Self {
        use crate::model::protobuf::golem::shardmanager::ip_address;

        let kind = match value.ip {
            IpAddr::V4(v4) => ip_address::Kind::Ipv4(u32::from(v4)),
            IpAddr::V6(v6) => ip_address::Kind::Ipv6(v6.octets().to_vec()),
        };

        GrpcPod {
            ip: Some(GrpcIpAddress { kind: Some(kind) }),
            port: value.port.into(),
        }
    }
}

impl TryFrom<GrpcPod> for Pod {
    type Error = String;
    fn try_from(value: GrpcPod) -> Result<Self, Self::Error> {
        use crate::model::protobuf::golem::shardmanager::ip_address;

        let ip = value.ip.ok_or("missing ip field")?;

        let ip = match ip.kind.ok_or("IpAddress.kind missing")? {
            ip_address::Kind::Ipv4(v4) => IpAddr::V4(Ipv4Addr::from(v4)),
            ip_address::Kind::Ipv6(v6) => {
                let bytes: [u8; 16] = v6
                    .try_into()
                    .map_err(|_| "invalid IPv6 length (must be 16 bytes)")?;
                IpAddr::V6(Ipv6Addr::from(bytes))
            }
        };

        Ok(Self {
            ip,
            port: value
                .port
                .try_into()
                .map_err(|_| "Could not convert port to u16")?,
        })
    }
}

impl From<RoutingTableEntry> for GrpcRoutingTableEntry {
    fn from(value: RoutingTableEntry) -> Self {
        Self {
            shard_id: Some(value.shard_id.into()),
            pod: Some(value.pod.into()),
        }
    }
}

impl TryFrom<GrpcRoutingTableEntry> for RoutingTableEntry {
    type Error = String;

    fn try_from(value: GrpcRoutingTableEntry) -> Result<Self, Self::Error> {
        Ok(Self {
            shard_id: value.shard_id.ok_or("shard_id field missing")?.into(),
            pod: value.pod.ok_or("pod field missing")?.try_into()?,
        })
    }
}

impl From<RoutingTable> for GrpcRoutingTable {
    fn from(value: RoutingTable) -> Self {
        Self {
            number_of_shards: value.number_of_shards.value as u32,
            shard_assignments: value
                .shard_assignments
                .into_iter()
                .map(|(shard_id, pod)| RoutingTableEntry { shard_id, pod }.into())
                .collect(),
        }
    }
}

impl TryFrom<GrpcRoutingTable> for RoutingTable {
    type Error = String;

    fn try_from(value: GrpcRoutingTable) -> Result<Self, Self::Error> {
        Ok(Self {
            number_of_shards: NumberOfShards {
                value: value
                    .number_of_shards
                    .try_into()
                    .map_err(|_| "failed converting number_of_shards to usize")?,
            },
            shard_assignments: value
                .shard_assignments
                .into_iter()
                .map(RoutingTableEntry::try_from)
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|routing_table_entry| (routing_table_entry.shard_id, routing_table_entry.pod))
                .collect(),
        })
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

impl From<AgentStatus> for golem::worker::AgentStatus {
    fn from(value: AgentStatus) -> Self {
        match value {
            AgentStatus::Running => golem::worker::AgentStatus::Running,
            AgentStatus::Idle => golem::worker::AgentStatus::Idle,
            AgentStatus::Suspended => golem::worker::AgentStatus::Suspended,
            AgentStatus::Interrupted => golem::worker::AgentStatus::Interrupted,
            AgentStatus::Retrying => golem::worker::AgentStatus::Retrying,
            AgentStatus::Failed => golem::worker::AgentStatus::Failed,
            AgentStatus::Exited => golem::worker::AgentStatus::Exited,
        }
    }
}

impl TryFrom<golem::worker::AgentFilter> for AgentFilter {
    type Error = String;

    fn try_from(value: golem::worker::AgentFilter) -> Result<Self, Self::Error> {
        match value.filter {
            Some(filter) => match filter {
                golem::worker::agent_filter::Filter::Name(filter) => Ok(AgentFilter::new_name(
                    filter.comparator.try_into()?,
                    filter.value,
                )),
                golem::worker::agent_filter::Filter::Revision(filter) => {
                    Ok(AgentFilter::new_revision(
                        filter.comparator.try_into()?,
                        filter.value.try_into()?,
                    ))
                }
                golem::worker::agent_filter::Filter::Status(filter) => Ok(AgentFilter::new_status(
                    filter.comparator.try_into()?,
                    filter.value.try_into()?,
                )),
                golem::worker::agent_filter::Filter::CreatedAt(filter) => {
                    let value = filter
                        .value
                        .map(|t| t.into())
                        .ok_or_else(|| "Missing value".to_string())?;
                    Ok(AgentFilter::new_created_at(
                        filter.comparator.try_into()?,
                        value,
                    ))
                }
                golem::worker::agent_filter::Filter::Env(filter) => Ok(AgentFilter::new_env(
                    filter.name,
                    filter.comparator.try_into()?,
                    filter.value,
                )),
                golem::worker::agent_filter::Filter::WasiConfig(filter) => {
                    Ok(AgentFilter::new_wasi_config(
                        filter.name,
                        filter.comparator.try_into()?,
                        filter.value,
                    ))
                }
                golem::worker::agent_filter::Filter::Not(filter) => {
                    let filter = *filter.filter.ok_or_else(|| "Missing filter".to_string())?;
                    Ok(AgentFilter::new_not(filter.try_into()?))
                }
                golem::worker::agent_filter::Filter::And(golem::worker::AgentAndFilter {
                    filters,
                }) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<AgentFilter>,
                        String,
                    >>(
                    )?;

                    Ok(AgentFilter::new_and(filters))
                }
                golem::worker::agent_filter::Filter::Or(golem::worker::AgentOrFilter {
                    filters,
                }) => {
                    let filters = filters.into_iter().map(|f| f.try_into()).collect::<Result<
                        Vec<AgentFilter>,
                        String,
                    >>(
                    )?;

                    Ok(AgentFilter::new_or(filters))
                }
            },
            None => Err("Missing filter".to_string()),
        }
    }
}

impl From<AgentFilter> for golem::worker::AgentFilter {
    fn from(value: AgentFilter) -> Self {
        let filter = match value {
            AgentFilter::Name(AgentNameFilter { comparator, value }) => {
                golem::worker::agent_filter::Filter::Name(golem::worker::AgentNameFilter {
                    comparator: comparator.into(),
                    value,
                })
            }
            AgentFilter::Revision(AgentRevisionFilter { comparator, value }) => {
                golem::worker::agent_filter::Filter::Revision(golem::worker::AgentRevisionFilter {
                    comparator: comparator.into(),
                    value: value.into(),
                })
            }
            AgentFilter::Env(AgentEnvFilter {
                name,
                comparator,
                value,
            }) => golem::worker::agent_filter::Filter::Env(golem::worker::AgentEnvFilter {
                name,
                comparator: comparator.into(),
                value,
            }),
            AgentFilter::WasiConfig(AgentConfigVarsFilter {
                name,
                comparator,
                value,
            }) => golem::worker::agent_filter::Filter::WasiConfig(
                golem::worker::AgentConfigVarsFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                },
            ),
            AgentFilter::Status(AgentStatusFilter { comparator, value }) => {
                golem::worker::agent_filter::Filter::Status(golem::worker::AgentStatusFilter {
                    comparator: comparator.into(),
                    value: value.into(),
                })
            }
            AgentFilter::CreatedAt(AgentCreatedAtFilter { comparator, value }) => {
                golem::worker::agent_filter::Filter::CreatedAt(
                    golem::worker::AgentCreatedAtFilter {
                        value: Some(value.into()),
                        comparator: comparator.into(),
                    },
                )
            }
            AgentFilter::Not(AgentNotFilter { filter }) => {
                let f: golem::worker::AgentFilter = (*filter).into();
                golem::worker::agent_filter::Filter::Not(Box::new(golem::worker::AgentNotFilter {
                    filter: Some(Box::new(f)),
                }))
            }
            AgentFilter::And(filter) => {
                golem::worker::agent_filter::Filter::And(golem::worker::AgentAndFilter {
                    filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                })
            }
            AgentFilter::Or(filter) => {
                golem::worker::agent_filter::Filter::Or(golem::worker::AgentOrFilter {
                    filters: filter.filters.into_iter().map(|f| f.into()).collect(),
                })
            }
        };

        golem::worker::AgentFilter {
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

impl TryFrom<golem::worker::LogEvent> for AgentEvent {
    type Error = String;

    fn try_from(value: golem::worker::LogEvent) -> Result<Self, Self::Error> {
        match value.event {
            Some(event) => match event {
                golem::worker::log_event::Event::Stdout(event) => Ok(AgentEvent::StdOut {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    bytes: event.message.into_bytes(),
                }),
                golem::worker::log_event::Event::Stderr(event) => Ok(AgentEvent::StdErr {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    bytes: event.message.into_bytes(),
                }),
                golem::worker::log_event::Event::Log(event) => Ok(AgentEvent::Log {
                    timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                    level: event.level().into(),
                    context: event.context,
                    message: event.message,
                }),
                golem::worker::log_event::Event::InvocationStarted(event) => {
                    Ok(AgentEvent::InvocationStart {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        function: event.function,
                        idempotency_key: event
                            .idempotency_key
                            .ok_or("Missing idempotency key")?
                            .into(),
                    })
                }
                golem::worker::log_event::Event::InvocationFinished(event) => {
                    Ok(AgentEvent::InvocationFinished {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        function: event.function,
                        idempotency_key: event
                            .idempotency_key
                            .ok_or("Missing idempotency key")?
                            .into(),
                    })
                }
                golem::worker::log_event::Event::ClientLagged(event) => {
                    Ok(AgentEvent::ClientLagged {
                        number_of_missed_messages: event.number_of_missed_messages,
                    })
                }
                golem::worker::log_event::Event::PluginError(event) => {
                    Ok(AgentEvent::PluginError {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        plugin_name: event.plugin_name,
                        message: event.message,
                    })
                }
            },
            None => Err("Missing event".to_string()),
        }
    }
}

impl TryFrom<AgentEvent> for golem::worker::LogEvent {
    type Error = String;

    fn try_from(value: AgentEvent) -> Result<Self, Self::Error> {
        match value {
            AgentEvent::StdOut { timestamp, bytes } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::Stdout(
                    golem::worker::StdOutLog {
                        message: String::from_utf8_lossy(&bytes).to_string(),
                        timestamp: Some(timestamp.into()),
                    },
                )),
            }),
            AgentEvent::StdErr { timestamp, bytes } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::Stderr(
                    golem::worker::StdErrLog {
                        message: String::from_utf8_lossy(&bytes).to_string(),
                        timestamp: Some(timestamp.into()),
                    },
                )),
            }),
            AgentEvent::Log {
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
            AgentEvent::InvocationStart {
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
            AgentEvent::InvocationFinished {
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
            AgentEvent::PluginError {
                timestamp,
                plugin_name,
                message,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::PluginError(
                    golem::worker::PluginError {
                        timestamp: Some(timestamp.into()),
                        plugin_name,
                        message,
                    },
                )),
            }),
            AgentEvent::ClientLagged {
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

impl From<golem::component::AgentFilePermissions> for AgentFilePermissions {
    fn from(value: golem::component::AgentFilePermissions) -> Self {
        match value {
            golem::component::AgentFilePermissions::ReadOnly => AgentFilePermissions::ReadOnly,
            golem::component::AgentFilePermissions::ReadWrite => AgentFilePermissions::ReadWrite,
        }
    }
}

impl From<AgentFilePermissions> for golem::component::AgentFilePermissions {
    fn from(value: AgentFilePermissions) -> Self {
        match value {
            AgentFilePermissions::ReadOnly => golem::component::AgentFilePermissions::ReadOnly,
            AgentFilePermissions::ReadWrite => golem::component::AgentFilePermissions::ReadWrite,
        }
    }
}

impl From<InitialAgentFile> for golem::component::InitialAgentFile {
    fn from(value: InitialAgentFile) -> Self {
        let permissions: golem::component::AgentFilePermissions = value.permissions.into();
        Self {
            content_hash: Some(value.content_hash.0.into()),
            path: value.path.to_abs_string(),
            permissions: permissions.into(),
            size: value.size,
        }
    }
}

impl TryFrom<golem::component::InitialAgentFile> for InitialAgentFile {
    type Error = String;

    fn try_from(value: golem::component::InitialAgentFile) -> Result<Self, Self::Error> {
        let permissions: golem::component::AgentFilePermissions = value
            .permissions
            .try_into()
            .map_err(|e| format!("Failed converting permissions {e}"))?;
        let permissions: AgentFilePermissions = permissions.into();
        let path = AgentFilePath::from_abs_str(&value.path).map_err(|e| e.to_string())?;
        let content_hash: diff::Hash = value
            .content_hash
            .ok_or("Missing content_hash field")?
            .try_into()?;

        Ok(Self {
            content_hash: AgentFileContentHash(content_hash),
            path,
            permissions,
            size: value.size,
        })
    }
}

pub fn to_protobuf_resource_description(
    key: AgentResourceId,
    description: AgentResourceDescription,
) -> golem::worker::ResourceDescription {
    golem::worker::ResourceDescription {
        created_at: Some(description.created_at.into()),
        resource_id: key.0,
        resource_owner: description.resource_owner,
        resource_name: description.resource_name,
    }
}

impl From<golem_api_grpc::proto::golem::worker::InvocationStatus> for InvocationStatus {
    fn from(value: golem_api_grpc::proto::golem::worker::InvocationStatus) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::InvocationStatus::Unknown => {
                InvocationStatus::Unknown
            }
            golem_api_grpc::proto::golem::worker::InvocationStatus::Pending => {
                InvocationStatus::Pending
            }
            golem_api_grpc::proto::golem::worker::InvocationStatus::Complete => {
                InvocationStatus::Complete
            }
        }
    }
}

impl From<InvocationStatus> for golem_api_grpc::proto::golem::worker::InvocationStatus {
    fn from(value: InvocationStatus) -> Self {
        match value {
            InvocationStatus::Unknown => {
                golem_api_grpc::proto::golem::worker::InvocationStatus::Unknown
            }
            InvocationStatus::Pending => {
                golem_api_grpc::proto::golem::worker::InvocationStatus::Pending
            }
            InvocationStatus::Complete => {
                golem_api_grpc::proto::golem::worker::InvocationStatus::Complete
            }
        }
    }
}

pub fn from_protobuf_resource_description(
    description: golem::worker::ResourceDescription,
) -> Result<(AgentResourceId, AgentResourceDescription), String> {
    let key = AgentResourceId(description.resource_id);
    let value = AgentResourceDescription {
        created_at: description.created_at.ok_or("Missing created_at")?.into(),
        resource_owner: description.resource_owner,
        resource_name: description.resource_name,
    };
    Ok((key, value))
}
