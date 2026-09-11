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
    AgentModeFilter, AgentNameFilter, AgentNotFilter, AgentRevisionFilter, AgentStatus,
    AgentStatusFilter, FilterComparator, IdempotencyKey, LogLevel, NumberOfShards, Pod, PromiseId,
    RoutingTable, RoutingTableEntry, ScanCursor, ShardEpoch, ShardId, StringFilterComparator,
    Timestamp,
};
use applying::Apply;
use chrono::{DateTime, TimeDelta, Utc};
use golem_api_grpc::proto::golem;
use golem_api_grpc::proto::golem::shardmanager::{
    IpAddress as GrpcIpAddress, Pod as GrpcPod, RoutingTable as GrpcRoutingTable,
    RoutingTableEntry as GrpcRoutingTableEntry,
};
use golem_api_grpc::proto::golem::worker::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Add;
use std::time::{Duration, Instant};

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
            component_id: value
                .component_id
                .ok_or("Missing AgentId.component_id")?
                .try_into()?,
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

pub fn shard_epochs_to_proto(
    shard_epochs: impl IntoIterator<Item = (ShardId, ShardEpoch)>,
) -> Vec<golem::shardmanager::ShardEpochEntry> {
    shard_epochs
        .into_iter()
        .map(|(shard_id, epoch)| golem::shardmanager::ShardEpochEntry {
            shard_id: Some(shard_id.into()),
            epoch: epoch.0,
        })
        .collect()
}

pub fn shard_epochs_from_proto<C: FromIterator<(ShardId, ShardEpoch)>>(
    entries: Vec<golem::shardmanager::ShardEpochEntry>,
) -> Result<C, String> {
    entries
        .into_iter()
        .map(|entry| {
            Ok((
                entry
                    .shard_id
                    .ok_or("ShardEpochEntry.shard_id missing")?
                    .into(),
                ShardEpoch(entry.epoch),
            ))
        })
        .collect()
}

/// Decodes a lease TTL off the wire into an expiry on the receiver's own
/// monotonic clock.
///
/// The wire carries how long the lease lasts, not when it ends, so the
/// sender's clock is never compared against the receiver's. `anchor` is the
/// receiver's clock at the moment it sent the request this TTL answers - not
/// the moment the answer arrived. The sender granted the lease some time after
/// that request left, so the receiver's expiry lands before the sender's by
/// however long the request and the answer spent in flight: time on the
/// network comes off the lease and is never added to it.
///
/// What anchoring cannot remove is rate error: both sides measure the lease on
/// their own clock, so a receiver whose clock runs slow relative to the
/// sender's outlives the sender's expiry by that fraction of the lease. Steady
/// NTP discipline bounds that at roughly 500 parts per million, 30 ms on a
/// minute; a daemon slewing off a large offset does not (chrony's default
/// ceiling is a twelfth, five seconds on a minute), and neither does a forward
/// step of the sender's wall clock. No allowance is subtracted for it here:
/// the size of one would be a statement about the operator's clocks rather
/// than a property of this protocol.
///
/// Total: an absent, negative, malformed or overflowing TTL is an error, never
/// a panic.
pub fn lease_expiry_from_ttl(
    lease_ttl: Option<prost_types::Duration>,
    anchor: Instant,
    field: &str,
) -> Result<Instant, String> {
    let ttl = lease_ttl.ok_or_else(|| format!("{field} is required"))?;
    let seconds = u64::try_from(ttl.seconds).map_err(|_| format!("{field} is negative"))?;
    // Bounded here: `Duration::new` would carry a whole second of nanoseconds
    // into the seconds rather than refuse it.
    let nanos = u32::try_from(ttl.nanos)
        .ok()
        .filter(|nanos| *nanos < 1_000_000_000)
        .ok_or_else(|| format!("{field} has out-of-range nanoseconds"))?;
    anchor
        .checked_add(Duration::new(seconds, nanos))
        .ok_or_else(|| format!("{field} overflows the expiry"))
}

/// Encodes a lease expiry for the wire as the time left on it, floored at zero:
/// a lease that has already lapsed goes out as a zero TTL, which the receiver
/// reads as expired on arrival.
pub fn lease_ttl_to_proto(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> prost_types::Duration {
    let remaining = expires_at.signed_duration_since(now).max(TimeDelta::zero());
    prost_types::Duration {
        seconds: remaining.num_seconds(),
        nanos: remaining.subsec_nanos(),
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
                golem::worker::agent_filter::Filter::Config(filter) => Ok(AgentFilter::new_config(
                    filter.name,
                    filter.comparator.try_into()?,
                    filter.value,
                )),
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
                golem::worker::agent_filter::Filter::Mode(filter) => {
                    let mode_proto: golem::component::AgentMode =
                        golem::component::AgentMode::try_from(filter.value).map_err(|err| {
                            format!("Invalid AgentMode in AgentModeFilter: {err}")
                        })?;
                    Ok(AgentFilter::new_mode(
                        filter.comparator.try_into()?,
                        mode_proto.into(),
                    ))
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
            AgentFilter::Config(AgentConfigVarsFilter {
                name,
                comparator,
                value,
            }) => {
                golem::worker::agent_filter::Filter::Config(golem::worker::AgentConfigVarsFilter {
                    name,
                    comparator: comparator.into(),
                    value,
                })
            }
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
            AgentFilter::Mode(AgentModeFilter { comparator, value }) => {
                let mode_proto: golem::component::AgentMode = value.into();
                golem::worker::agent_filter::Filter::Mode(golem::worker::AgentModeFilter {
                    comparator: comparator.into(),
                    value: mode_proto as i32,
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
                golem::worker::log_event::Event::SnapshotRecoverySucceeded(event) => {
                    Ok(AgentEvent::SnapshotRecoverySucceeded {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        snapshot_index: OplogIndex::from_u64(event.snapshot_index),
                    })
                }
                golem::worker::log_event::Event::SnapshotRecoveryFailed(event) => {
                    Ok(AgentEvent::SnapshotRecoveryFailed {
                        timestamp: event.timestamp.ok_or("Missing timestamp")?.into(),
                        snapshot_index: OplogIndex::from_u64(event.snapshot_index),
                        error: event.error,
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
            AgentEvent::SnapshotRecoverySucceeded {
                timestamp,
                snapshot_index,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::SnapshotRecoverySucceeded(
                    golem::worker::SnapshotRecoverySucceeded {
                        timestamp: Some(timestamp.into()),
                        snapshot_index: snapshot_index.into(),
                    },
                )),
            }),
            AgentEvent::SnapshotRecoveryFailed {
                timestamp,
                snapshot_index,
                error,
            } => Ok(golem::worker::LogEvent {
                event: Some(golem::worker::log_event::Event::SnapshotRecoveryFailed(
                    golem::worker::SnapshotRecoveryFailed {
                        timestamp: Some(timestamp.into()),
                        snapshot_index: snapshot_index.into(),
                        error,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ShardAssignment, ShardEpoch, ShardId, ShardLeaseRevision};
    use std::collections::HashMap;
    use std::time::Instant;
    use test_r::test;

    test_r::enable!();

    /// The round trip goes through the free conversion functions
    /// themselves — they are what `AssignShards` and `RenewShardLease` use on
    /// both sides of the wire. `epoch_of` is how a reader of the pushed set
    /// gets a shard's ownership generation back out.
    #[test]
    fn epochs_survive_a_push_round_trip() {
        let pushed: HashMap<ShardId, ShardEpoch> = HashMap::from([
            (ShardId::new(0), ShardEpoch(1)),
            (ShardId::new(7), ShardEpoch(42)),
            (ShardId::new(1023), ShardEpoch(0)),
        ]);

        let on_the_wire = shard_epochs_to_proto(pushed.clone());
        assert_eq!(on_the_wire.len(), 3);

        let received: HashMap<ShardId, ShardEpoch> =
            shard_epochs_from_proto(on_the_wire).expect("a well-formed push decodes");
        assert_eq!(received, pushed);

        let mut assignment = ShardAssignment::default();
        assignment.set_shards(1024, &received, ShardLeaseRevision(1));

        assert_eq!(assignment.epoch_of(&ShardId::new(0)), Some(ShardEpoch(1)));
        assert_eq!(assignment.epoch_of(&ShardId::new(7)), Some(ShardEpoch(42)));
        assert_eq!(
            assignment.epoch_of(&ShardId::new(1023)),
            Some(ShardEpoch(0))
        );
        assert_eq!(
            assignment.epoch_of(&ShardId::new(5)),
            None,
            "a shard absent from the push has no epoch here"
        );
    }

    #[test]
    fn an_entry_without_a_shard_id_is_rejected_rather_than_silently_dropped() {
        let on_the_wire = vec![golem::shardmanager::ShardEpochEntry {
            shard_id: None,
            epoch: 3,
        }];

        let decoded: Result<HashMap<ShardId, ShardEpoch>, String> =
            shard_epochs_from_proto(on_the_wire);

        assert!(decoded.is_err());
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_764_000_000, 0).unwrap()
    }

    /// A lease TTL is required on the wire. `None` is the executor's
    /// "never expires" sentinel, reachable only from the in-process
    /// single-shard service; letting an absent proto field decode to it would
    /// turn the self-fence off silently and permanently.
    #[test]
    fn an_absent_lease_ttl_is_rejected_rather_than_read_as_never_expiring() {
        let decoded = lease_expiry_from_ttl(None, Instant::now(), "ShardLease.lease_ttl");

        assert_eq!(decoded, Err("ShardLease.lease_ttl is required".to_string()));
    }

    /// `Duration.seconds` is an unbounded `i64` on the wire. A value that
    /// overflows the receiver's clock once added has to come back as an error:
    /// these crates abort on panic, so an inbound RPC could otherwise take the
    /// process down. A negative TTL is malformed rather than "already expired" -
    /// the sender floors at zero.
    #[test]
    fn an_out_of_range_or_negative_lease_ttl_is_an_error_and_not_a_panic() {
        let past_what_the_clock_holds = prost_types::Duration {
            seconds: i64::MAX,
            nanos: 0,
        };
        assert!(
            lease_expiry_from_ttl(Some(past_what_the_clock_holds), Instant::now(), "lease_ttl")
                .is_err()
        );

        let negative = prost_types::Duration {
            seconds: -1,
            nanos: 0,
        };
        assert_eq!(
            lease_expiry_from_ttl(Some(negative), Instant::now(), "lease_ttl"),
            Err("lease_ttl is negative".to_string())
        );
    }

    /// `Duration.nanos` is an `i32` on the wire, so it can arrive negative or
    /// past a second even though no correct producer sends that. Past a second
    /// is refused rather than carried into the seconds.
    #[test]
    fn a_lease_ttl_with_impossible_nanoseconds_is_rejected() {
        let negative = prost_types::Duration {
            seconds: 1,
            nanos: -1,
        };
        assert!(lease_expiry_from_ttl(Some(negative), Instant::now(), "lease_ttl").is_err());

        let overflowing = prost_types::Duration {
            seconds: 1,
            nanos: 2_000_000_000,
        };
        assert!(lease_expiry_from_ttl(Some(overflowing), Instant::now(), "lease_ttl").is_err());
    }

    /// The sender keeps an absolute expiry on its wall clock and the receiver
    /// keeps one on its monotonic clock; only the time left travels.
    #[test]
    fn a_lease_expiry_survives_the_round_trip_to_the_wire_and_back() {
        let now = fixed_now();
        let granted = now + TimeDelta::new(60, 123_456_789).unwrap();

        let on_the_wire = lease_ttl_to_proto(granted, now);
        assert_eq!(
            on_the_wire,
            prost_types::Duration {
                seconds: 60,
                nanos: 123_456_789,
            }
        );

        let anchor = Instant::now();
        let decoded = lease_expiry_from_ttl(Some(on_the_wire), anchor, "lease_ttl").unwrap();
        assert_eq!(decoded - anchor, Duration::new(60, 123_456_789));
    }

    /// The point of anchoring the TTL where the request was sent rather than
    /// where the answer arrived: the answer's time in flight comes off the
    /// receiver's copy of the lease instead of being added to it.
    #[test]
    fn a_lease_expiry_is_anchored_where_the_request_was_sent_not_where_the_answer_arrived() {
        let sent_at = Instant::now();
        let ttl = prost_types::Duration {
            seconds: 60,
            nanos: 0,
        };
        // the answer took its time
        std::thread::sleep(Duration::from_millis(20));

        let decoded = lease_expiry_from_ttl(Some(ttl), sent_at, "lease_ttl").unwrap();

        assert_eq!(decoded, sent_at + Duration::from_secs(60));
        assert!(
            decoded < Instant::now() + Duration::from_secs(60),
            "time the answer spent in flight must come off the lease, never on to it"
        );
    }

    /// The property the anchoring buys, end to end through both wire
    /// functions with one clock for both sides (the claim is about real time;
    /// skew is what the duration form already settles): however long the
    /// request spends in flight and the manager's write takes, the executor's
    /// expiry is never later than the manager's. The answer's own time in
    /// flight does not appear, because the decode does not depend on it.
    #[test]
    fn an_executors_lease_never_outlives_the_managers() {
        let lease = TimeDelta::seconds(60);
        let request_sent = fixed_now();
        let anchor = Instant::now();

        for request_transit in [0, 1, 5, 30].map(TimeDelta::seconds) {
            for write_time in [0, 1, 4].map(TimeDelta::seconds) {
                let granted_at = request_sent + request_transit;
                let managers_expiry = granted_at + lease;
                let on_the_wire = lease_ttl_to_proto(managers_expiry, granted_at + write_time);

                let executors_expiry =
                    lease_expiry_from_ttl(Some(on_the_wire), anchor, "lease_ttl").unwrap();

                // both measured from the moment the request was sent
                let executor_holds = executors_expiry - anchor;
                let manager_holds = (managers_expiry - request_sent).to_std().unwrap();
                assert!(
                    executor_holds <= manager_holds,
                    "request transit {request_transit}, write {write_time}: the executor holds \
                     {executor_holds:?} but the manager only {manager_holds:?}"
                );
            }
        }
    }

    /// A lease that had already lapsed when it was encoded goes out as a zero
    /// TTL rather than a negative one, and decodes as expiring at the anchor,
    /// which is in the past by the time anything checks it.
    #[test]
    fn a_lapsed_lease_is_sent_as_a_zero_ttl_and_arrives_expired() {
        let now = fixed_now();
        let lapsed = now - TimeDelta::seconds(5);

        let on_the_wire = lease_ttl_to_proto(lapsed, now);
        assert_eq!(
            on_the_wire,
            prost_types::Duration {
                seconds: 0,
                nanos: 0
            }
        );

        let anchor = Instant::now();
        assert_eq!(
            lease_expiry_from_ttl(Some(on_the_wire), anchor, "lease_ttl"),
            Ok(anchor)
        );
    }
}
