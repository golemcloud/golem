// Copyright 2024 Golem Cloud
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

use crate::config::RetryConfig;
use crate::model::lucene::{LeafQuery, Query};
use crate::model::oplog::{LogLevel, OplogIndex, WorkerResourceId, WrappedFunctionType};
use crate::model::regions::OplogRegion;
use crate::model::{AccountId, ComponentVersion, IdempotencyKey, Timestamp, WorkerId};
use golem_api_grpc::proto::golem::worker::{oplog_entry, worker_invocation, wrapped_function_type};
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair};
use golem_wasm_rpc::{Value, ValueAndType};
use poem_openapi::types::{ParseFromParameter, ParseResult};
use poem_openapi::{Object, Union};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::time::Duration;

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct Empty;

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct SnapshotBasedUpdateParameters {
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicUpdateDescription {
    Automatic(Empty),
    SnapshotBased(SnapshotBasedUpdateParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct WriteRemoteBatchedParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicWrappedFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    ReadLocal(Empty),
    /// The side-effect writes to the worker's local state (for example local file system)
    WriteLocal(Empty),
    /// The side-effect reads from external state (for example a key-value store)
    ReadRemote(Empty),
    /// The side-effect manipulates external state (for example an RPC call)
    WriteRemote(Empty),
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
    WriteRemoteBatched(WriteRemoteBatchedParameters),
}

impl From<WrappedFunctionType> for PublicWrappedFunctionType {
    fn from(wrapped_function_type: WrappedFunctionType) -> Self {
        match wrapped_function_type {
            WrappedFunctionType::ReadLocal => PublicWrappedFunctionType::ReadLocal(Empty),
            WrappedFunctionType::WriteLocal => PublicWrappedFunctionType::WriteLocal(Empty),
            WrappedFunctionType::ReadRemote => PublicWrappedFunctionType::ReadRemote(Empty),
            WrappedFunctionType::WriteRemote => PublicWrappedFunctionType::WriteRemote(Empty),
            WrappedFunctionType::WriteRemoteBatched(index) => {
                PublicWrappedFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                    index,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct DetailsParameter {
    pub details: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct PublicRetryConfig {
    pub max_attempts: u32,
    #[serde(with = "humantime_serde")]
    pub min_delay: Duration,
    #[serde(with = "humantime_serde")]
    pub max_delay: Duration,
    pub multiplier: f64,
    pub max_jitter_factor: Option<f64>,
}

impl From<RetryConfig> for PublicRetryConfig {
    fn from(retry_config: RetryConfig) -> Self {
        PublicRetryConfig {
            max_attempts: retry_config.max_attempts,
            min_delay: retry_config.min_delay,
            max_delay: retry_config.max_delay,
            multiplier: retry_config.multiplier,
            max_jitter_factor: retry_config.max_jitter_factor,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ExportedFunctionParameters {
    pub idempotency_key: IdempotencyKey,
    pub full_function_name: String,
    pub function_input: Option<Vec<ValueAndType>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ManualUpdateParameters {
    pub target_version: ComponentVersion,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicWorkerInvocation {
    ExportedFunction(ExportedFunctionParameters),
    ManualUpdate(ManualUpdateParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct CreateParameters {
    pub timestamp: Timestamp,
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub account_id: AccountId,
    pub parent: Option<WorkerId>,
    pub component_size: u64,
    pub initial_total_linear_memory_size: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ImportedFunctionInvokedParameters {
    pub timestamp: Timestamp,
    pub function_name: String,
    pub request: ValueAndType,
    pub response: ValueAndType,
    pub wrapped_function_type: PublicWrappedFunctionType,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ExportedFunctionInvokedParameters {
    pub timestamp: Timestamp,
    pub function_name: String,
    pub request: Vec<ValueAndType>,
    pub idempotency_key: IdempotencyKey,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ExportedFunctionCompletedParameters {
    pub timestamp: Timestamp,
    pub response: ValueAndType,
    pub consumed_fuel: i64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct TimestampParameter {
    pub timestamp: Timestamp,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ErrorParameters {
    pub timestamp: Timestamp,
    pub error: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct JumpParameters {
    pub timestamp: Timestamp,
    pub jump: OplogRegion,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ChangeRetryPolicyParameters {
    pub timestamp: Timestamp,
    pub new_policy: PublicRetryConfig,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct EndRegionParameters {
    pub timestamp: Timestamp,
    pub begin_index: OplogIndex,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct PendingWorkerInvocationParameters {
    pub timestamp: Timestamp,
    pub invocation: PublicWorkerInvocation,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct PendingUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub description: PublicUpdateDescription,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct SuccessfulUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub new_component_size: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct FailedUpdateParameters {
    pub timestamp: Timestamp,
    pub target_version: ComponentVersion,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct GrowMemoryParameters {
    pub timestamp: Timestamp,
    pub delta: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct ResourceParameters {
    pub timestamp: Timestamp,
    pub id: WorkerResourceId,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct DescribeResourceParameters {
    pub timestamp: Timestamp,
    pub id: WorkerResourceId,
    pub resource_name: String,
    pub resource_params: Vec<ValueAndType>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Object)]
pub struct LogParameters {
    pub timestamp: Timestamp,
    pub level: LogLevel,
    pub context: String,
    pub message: String,
}

/// A mirror of the core `OplogEntry` type, without the undefined arbitrary payloads.
///
/// Instead, it encodes all payloads with wasm-rpc `Value` types. This makes this the base type
/// for exposing oplog entries through various APIs such as gRPC, REST and WIT.
///
/// The rest of the system will always use `OplogEntry` internally - the only point where the
/// oplog payloads are decoded and re-encoded as `Value` is in this module, and it should only be used
/// before exposing an oplog entry through a public API.
#[derive(Clone, Debug, Serialize, PartialEq, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
pub enum PublicOplogEntry {
    Create(CreateParameters),
    /// The worker invoked a host function
    ImportedFunctionInvoked(ImportedFunctionInvokedParameters),
    /// The worker has been invoked
    ExportedFunctionInvoked(ExportedFunctionInvokedParameters),
    /// The worker has completed an invocation
    ExportedFunctionCompleted(ExportedFunctionCompletedParameters),
    /// Worker suspended
    Suspend(TimestampParameter),
    /// Worker failed
    Error(ErrorParameters),
    /// Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
    /// more predictable.
    NoOp(TimestampParameter),
    /// The worker needs to recover up to the given target oplog index and continue running from
    /// the source oplog index from there
    /// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
    /// ignore all recorded operations in between.
    Jump(JumpParameters),
    /// Indicates that the worker has been interrupted at this point.
    /// Only used to recompute the worker's (cached) status, has no effect on execution.
    Interrupted(TimestampParameter),
    /// Indicates that the worker has been exited using WASI's exit function.
    Exited(TimestampParameter),
    /// Overrides the worker's retry policy
    ChangeRetryPolicy(ChangeRetryPolicyParameters),
    /// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
    /// recovery except if there is a corresponding `EndAtomicRegion` entry.
    BeginAtomicRegion(TimestampParameter),
    /// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
    /// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
    /// compaction.
    EndAtomicRegion(EndRegionParameters),
    /// Begins a remote write operation. Only used when idempotence mode is off. In this case each
    /// remote write must be surrounded by a `BeginRemoteWrite` and `EndRemoteWrite` log pair and
    /// unfinished remote writes cannot be recovered.
    BeginRemoteWrite(TimestampParameter),
    /// Marks the end of a remote write operation. Only used when idempotence mode is off.
    EndRemoteWrite(EndRegionParameters),
    /// An invocation request arrived while the worker was busy
    PendingWorkerInvocation(PendingWorkerInvocationParameters),
    /// An update request arrived and will be applied as soon the worker restarts
    PendingUpdate(PendingUpdateParameters),
    /// An update was successfully applied
    SuccessfulUpdate(SuccessfulUpdateParameters),
    /// An update failed to be applied
    FailedUpdate(FailedUpdateParameters),
    /// Increased total linear memory size
    GrowMemory(GrowMemoryParameters),
    /// Created a resource instance
    CreateResource(ResourceParameters),
    /// Dropped a resource instance
    DropResource(ResourceParameters),
    /// Adds additional information for a created resource instance
    DescribeResource(DescribeResourceParameters),
    /// The worker emitted a log message
    Log(LogParameters),
    /// Marks the point where the worker was restarted from clean initial state
    Restart(TimestampParameter),
}

impl PublicOplogEntry {
    pub fn matches(&self, query: &Query) -> bool {
        fn matches_impl(entry: &PublicOplogEntry, query: &Query, field_stack: &[String]) -> bool {
            match query {
                Query::Or { queries } => queries
                    .iter()
                    .any(|query| matches_impl(entry, query, field_stack)),
                Query::And { queries } => queries
                    .iter()
                    .all(|query| matches_impl(entry, query, field_stack)),
                Query::Not { query } => !matches_impl(entry, query, field_stack),
                Query::Regex { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Term { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Phrase { .. } => {
                    entry.matches_leaf_query(field_stack, &query.clone().try_into().unwrap())
                }
                Query::Field { field, query } => {
                    let mut new_stack: Vec<String> = field_stack.to_vec();
                    let parts: Vec<String> = field.split(".").map(|s| s.to_string()).collect();
                    new_stack.extend(parts);
                    matches_impl(entry, query, &new_stack)
                }
            }
        }

        matches_impl(self, query, &[])
    }

    fn string_match(s: &str, path: &[String], query_path: &[String], query: &LeafQuery) -> bool {
        let lowercase_path = path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        let lowercase_query_path = query_path
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<String>>();
        if lowercase_path == lowercase_query_path || query_path.is_empty() {
            query.matches(s)
        } else {
            false
        }
    }

    fn matches_leaf_query(&self, query_path: &[String], query: &LeafQuery) -> bool {
        match self {
            PublicOplogEntry::Create(_params) => {
                Self::string_match("create", &[], query_path, query)
            }
            PublicOplogEntry::ImportedFunctionInvoked(params) => {
                Self::string_match("importedfunctioninvoked", &[], query_path, query)
                    || Self::string_match("imported-function-invoked", &[], query_path, query)
                    || Self::string_match("imported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || Self::match_value(&params.request, &[], query_path, query)
                    || Self::match_value(&params.response, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionInvoked(params) => {
                Self::string_match("exportedfunctioninvoked", &[], query_path, query)
                    || Self::string_match("exported-function-invoked", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || Self::string_match(&params.function_name, &[], query_path, query)
                    || params
                        .request
                        .iter()
                        .any(|v| Self::match_value(v, &[], query_path, query))
                    || Self::string_match(&params.idempotency_key.value, &[], query_path, query)
            }
            PublicOplogEntry::ExportedFunctionCompleted(params) => {
                Self::string_match("exportedfunctioncompleted", &[], query_path, query)
                    || Self::string_match("exported-function-completed", &[], query_path, query)
                    || Self::string_match("exported-function", &[], query_path, query)
                    || Self::match_value(&params.response, &[], query_path, query)
                // TODO: should we store function name and idempotency key in ExportedFunctionCompleted?
            }
            PublicOplogEntry::Suspend(_params) => {
                Self::string_match("suspend", &[], query_path, query)
            }
            PublicOplogEntry::Error(params) => {
                Self::string_match("error", &[], query_path, query)
                    || Self::string_match(&params.error, &[], query_path, query)
            }
            PublicOplogEntry::NoOp(_params) => Self::string_match("noop", &[], query_path, query),
            PublicOplogEntry::Jump(_params) => Self::string_match("jump", &[], query_path, query),
            PublicOplogEntry::Interrupted(_params) => {
                Self::string_match("interrupted", &[], query_path, query)
            }
            PublicOplogEntry::Exited(_params) => {
                Self::string_match("exited", &[], query_path, query)
            }
            PublicOplogEntry::ChangeRetryPolicy(_params) => {
                Self::string_match("changeretrypolicy", &[], query_path, query)
                    || Self::string_match("change-retry-policy", &[], query_path, query)
            }
            PublicOplogEntry::BeginAtomicRegion(_params) => {
                Self::string_match("beginatomicregion", &[], query_path, query)
                    || Self::string_match("begin-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::EndAtomicRegion(_params) => {
                Self::string_match("endatomicregion", &[], query_path, query)
                    || Self::string_match("end-atomic-region", &[], query_path, query)
            }
            PublicOplogEntry::BeginRemoteWrite(_params) => {
                Self::string_match("beginremotewrite", &[], query_path, query)
                    || Self::string_match("begin-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::EndRemoteWrite(_params) => {
                Self::string_match("endremotewrite", &[], query_path, query)
                    || Self::string_match("end-remote-write", &[], query_path, query)
            }
            PublicOplogEntry::PendingWorkerInvocation(params) => {
                Self::string_match("pendingworkerinvocation", &[], query_path, query)
                    || Self::string_match("pending-worker-invocation", &[], query_path, query)
                    || match &params.invocation {
                        PublicWorkerInvocation::ExportedFunction(params) => {
                            Self::string_match(&params.full_function_name, &[], query_path, query)
                                || Self::string_match(
                                    &params.idempotency_key.value,
                                    &[],
                                    query_path,
                                    query,
                                )
                                || params
                                    .function_input
                                    .as_ref()
                                    .map(|params| {
                                        params
                                            .iter()
                                            .any(|v| Self::match_value(v, &[], query_path, query))
                                    })
                                    .unwrap_or(false)
                        }
                        PublicWorkerInvocation::ManualUpdate(params) => Self::string_match(
                            &params.target_version.to_string(),
                            &[],
                            query_path,
                            query,
                        ),
                    }
            }
            PublicOplogEntry::PendingUpdate(params) => {
                Self::string_match("pendingupdate", &[], query_path, query)
                    || Self::string_match("pending-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::SuccessfulUpdate(params) => {
                Self::string_match("successfulupdate", &[], query_path, query)
                    || Self::string_match("successful-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
            }
            PublicOplogEntry::FailedUpdate(params) => {
                Self::string_match("failedupdate", &[], query_path, query)
                    || Self::string_match("failed-update", &[], query_path, query)
                    || Self::string_match("update", &[], query_path, query)
                    || Self::string_match(
                        &params.target_version.to_string(),
                        &[],
                        query_path,
                        query,
                    )
                    || params
                        .details
                        .as_ref()
                        .map(|details| Self::string_match(details, &[], query_path, query))
                        .unwrap_or(false)
            }
            PublicOplogEntry::GrowMemory(_params) => {
                Self::string_match("growmemory", &[], query_path, query)
                    || Self::string_match("grow-memory", &[], query_path, query)
            }
            PublicOplogEntry::CreateResource(_params) => {
                Self::string_match("createresource", &[], query_path, query)
                    || Self::string_match("create-resource", &[], query_path, query)
            }
            PublicOplogEntry::DropResource(_params) => {
                Self::string_match("dropresource", &[], query_path, query)
                    || Self::string_match("drop-resource", &[], query_path, query)
            }
            PublicOplogEntry::DescribeResource(params) => {
                Self::string_match("describeresource", &[], query_path, query)
                    || Self::string_match("describe-resource", &[], query_path, query)
                    || Self::string_match(&params.resource_name, &[], query_path, query)
                    || params
                        .resource_params
                        .iter()
                        .any(|v| Self::match_value(v, &[], query_path, query))
            }
            PublicOplogEntry::Log(params) => {
                Self::string_match("log", &[], query_path, query)
                    || Self::string_match(&params.context, &[], query_path, query)
                    || Self::string_match(&params.message, &[], query_path, query)
            }
            PublicOplogEntry::Restart(_params) => {
                Self::string_match("restart", &[], query_path, query)
            }
        }
    }

    fn match_value(
        value: &ValueAndType,
        path_stack: &[String],
        query_path: &[String],
        query: &LeafQuery,
    ) -> bool {
        match (&value.value, &value.typ) {
            (Value::Bool(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::U64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S8(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S16(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::S64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F32(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::F64(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::Char(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::String(value), _) => {
                Self::string_match(&value.to_string(), path_stack, query_path, query)
            }
            (Value::List(elems), AnalysedType::List(list)) => elems.iter().any(|v| {
                Self::match_value(
                    &ValueAndType::new(v.clone(), (*list.inner).clone()),
                    path_stack,
                    query_path,
                    query,
                )
            }),
            (Value::Tuple(elems), AnalysedType::Tuple(tuple)) => {
                if elems.len() != tuple.items.len() {
                    false
                } else {
                    elems
                        .iter()
                        .zip(tuple.items.iter())
                        .enumerate()
                        .any(|(idx, (v, t))| {
                            let mut new_path: Vec<String> = path_stack.to_vec();
                            new_path.push(idx.to_string());
                            Self::match_value(
                                &ValueAndType::new(v.clone(), t.clone()),
                                &new_path,
                                query_path,
                                query,
                            )
                        })
                }
            }
            (Value::Record(fields), AnalysedType::Record(record)) => {
                if fields.len() != record.fields.len() {
                    false
                } else {
                    fields.iter().zip(record.fields.iter()).any(|(v, t)| {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(t.name.clone());
                        Self::match_value(
                            &ValueAndType::new(v.clone(), t.typ.clone()),
                            &new_path,
                            path_stack,
                            query,
                        )
                    })
                }
            }
            (
                Value::Variant {
                    case_value,
                    case_idx,
                },
                AnalysedType::Variant(variant),
            ) => {
                let case = variant.cases.get(*case_idx as usize);
                match (case_value, case) {
                    (
                        Some(value),
                        Some(NameOptionTypePair {
                            typ: Some(typ),
                            name,
                        }),
                    ) => {
                        let mut new_path: Vec<String> = path_stack.to_vec();
                        new_path.push(name.clone());
                        Self::match_value(
                            &ValueAndType::new((**value).clone(), typ.clone()),
                            &new_path,
                            query_path,
                            query,
                        )
                    }
                    _ => false,
                }
            }
            (Value::Enum(value), AnalysedType::Enum(typ)) => {
                if let Some(case) = typ.cases.get(*value as usize) {
                    Self::string_match(case, path_stack, query_path, query)
                } else {
                    false
                }
            }
            (Value::Flags(bitmap), AnalysedType::Flags(flags)) => {
                let names = bitmap
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, set)| if *set { flags.names.get(idx) } else { None })
                    .collect::<Vec<_>>();
                names
                    .iter()
                    .any(|name| Self::string_match(name, path_stack, query_path, query))
            }
            (Value::Option(Some(value)), AnalysedType::Option(typ)) => Self::match_value(
                &ValueAndType::new((**value).clone(), (*typ.inner).clone()),
                path_stack,
                query_path,
                query,
            ),
            (Value::Result(value), AnalysedType::Result(typ)) => match value {
                Ok(Some(value)) if typ.ok.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("ok".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.ok.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                Err(Some(value)) if typ.err.is_some() => {
                    let mut new_path = path_stack.to_vec();
                    new_path.push("err".to_string());
                    Self::match_value(
                        &ValueAndType::new(
                            (**value).clone(),
                            (**(typ.err.as_ref().unwrap())).clone(),
                        ),
                        &new_path,
                        query_path,
                        query,
                    )
                }
                _ => false,
            },
            (Value::Handle { .. }, _) => false,
            _ => false,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::OplogEntry> for PublicOplogEntry {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::worker::OplogEntry) -> Result<Self, String> {
        match value.entry.ok_or("Oplog entry is empty")? {
            oplog_entry::Entry::Create(create) => Ok(PublicOplogEntry::Create(CreateParameters {
                timestamp: create.timestamp.ok_or("Missing timestamp field")?.into(),
                worker_id: create
                    .worker_id
                    .ok_or("Missing worker_id field")?
                    .try_into()?,
                component_version: create.component_version,
                args: create.args,
                env: create.env.into_iter().collect(),
                account_id: create.account_id.ok_or("Missing account_id field")?.into(),
                parent: match create.parent {
                    Some(parent) => Some(parent.try_into()?),
                    None => None,
                },
                component_size: create.component_size,
                initial_total_linear_memory_size: create.initial_total_linear_memory_size,
            })),
            oplog_entry::Entry::ImportedFunctionInvoked(imported_function_invoked) => Ok(
                PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
                    timestamp: imported_function_invoked
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    function_name: imported_function_invoked.function_name,
                    request: imported_function_invoked
                        .request
                        .ok_or("Missing request field")?
                        .try_into()?,
                    response: imported_function_invoked
                        .response
                        .ok_or("Missing response field")?
                        .try_into()?,
                    wrapped_function_type: imported_function_invoked
                        .wrapped_function_type
                        .ok_or("Missing wrapped_function_type field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::ExportedFunctionInvoked(exported_function_invoked) => Ok(
                PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParameters {
                    timestamp: exported_function_invoked
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    function_name: exported_function_invoked.function_name,
                    request: exported_function_invoked
                        .request
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<ValueAndType>, String>>()?,
                    idempotency_key: exported_function_invoked
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                }),
            ),
            oplog_entry::Entry::ExportedFunctionCompleted(exported_function_completed) => Ok(
                PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
                    timestamp: exported_function_completed
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    response: exported_function_completed
                        .response
                        .ok_or("Missing response field")?
                        .try_into()?,
                    consumed_fuel: exported_function_completed.consumed_fuel,
                }),
            ),
            oplog_entry::Entry::Suspend(suspend) => {
                Ok(PublicOplogEntry::Suspend(TimestampParameter {
                    timestamp: suspend.timestamp.ok_or("Missing timestamp field")?.into(),
                }))
            }
            oplog_entry::Entry::Error(error) => Ok(PublicOplogEntry::Error(ErrorParameters {
                timestamp: error.timestamp.ok_or("Missing timestamp field")?.into(),
                error: error.error,
            })),
            oplog_entry::Entry::NoOp(no_op) => Ok(PublicOplogEntry::NoOp(TimestampParameter {
                timestamp: no_op.timestamp.ok_or("Missing timestamp field")?.into(),
            })),
            oplog_entry::Entry::Jump(jump) => Ok(PublicOplogEntry::Jump(JumpParameters {
                timestamp: jump.timestamp.ok_or("Missing timestamp field")?.into(),
                jump: OplogRegion {
                    start: OplogIndex::from_u64(jump.start),
                    end: OplogIndex::from_u64(jump.end),
                },
            })),
            oplog_entry::Entry::Interrupted(interrupted) => {
                Ok(PublicOplogEntry::Interrupted(TimestampParameter {
                    timestamp: interrupted
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }))
            }
            oplog_entry::Entry::Exited(exited) => {
                Ok(PublicOplogEntry::Exited(TimestampParameter {
                    timestamp: exited.timestamp.ok_or("Missing timestamp field")?.into(),
                }))
            }
            oplog_entry::Entry::ChangeRetryPolicy(change_retry_policy) => Ok(
                PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParameters {
                    timestamp: change_retry_policy
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    new_policy: change_retry_policy
                        .retry_policy
                        .ok_or("Missing retry_policy field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::BeginAtomicRegion(begin_atomic_region) => {
                Ok(PublicOplogEntry::BeginAtomicRegion(TimestampParameter {
                    timestamp: begin_atomic_region
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }))
            }
            oplog_entry::Entry::EndAtomicRegion(end_atomic_region) => {
                Ok(PublicOplogEntry::EndAtomicRegion(EndRegionParameters {
                    timestamp: end_atomic_region
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    begin_index: OplogIndex::from_u64(end_atomic_region.begin_index),
                }))
            }
            oplog_entry::Entry::BeginRemoteWrite(begin_remote_write) => {
                Ok(PublicOplogEntry::BeginRemoteWrite(TimestampParameter {
                    timestamp: begin_remote_write
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                }))
            }
            oplog_entry::Entry::EndRemoteWrite(end_remote_write) => {
                Ok(PublicOplogEntry::EndRemoteWrite(EndRegionParameters {
                    timestamp: end_remote_write
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    begin_index: OplogIndex::from_u64(end_remote_write.begin_index),
                }))
            }
            oplog_entry::Entry::PendingWorkerInvocation(pending_worker_invocation) => Ok(
                PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParameters {
                    timestamp: pending_worker_invocation
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    invocation: pending_worker_invocation
                        .invocation
                        .ok_or("Missing invocation field")?
                        .try_into()?,
                }),
            ),
            oplog_entry::Entry::PendingUpdate(pending_update) => {
                Ok(PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
                    timestamp: pending_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_version: pending_update.target_version,
                    description: pending_update
                        .update_description
                        .ok_or("Missing update_description field")?
                        .try_into()?,
                }))
            }
            oplog_entry::Entry::SuccessfulUpdate(successful_update) => Ok(
                PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParameters {
                    timestamp: successful_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_version: successful_update.target_version,
                    new_component_size: successful_update.new_component_size,
                }),
            ),
            oplog_entry::Entry::FailedUpdate(failed_update) => {
                Ok(PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
                    timestamp: failed_update
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    target_version: failed_update.target_version,
                    details: failed_update.details,
                }))
            }
            oplog_entry::Entry::GrowMemory(grow_memory) => {
                Ok(PublicOplogEntry::GrowMemory(GrowMemoryParameters {
                    timestamp: grow_memory
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    delta: grow_memory.delta,
                }))
            }
            oplog_entry::Entry::CreateResource(create_resource) => {
                Ok(PublicOplogEntry::CreateResource(ResourceParameters {
                    timestamp: create_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: WorkerResourceId(create_resource.resource_id),
                }))
            }
            oplog_entry::Entry::DropResource(drop_resource) => {
                Ok(PublicOplogEntry::DropResource(ResourceParameters {
                    timestamp: drop_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: WorkerResourceId(drop_resource.resource_id),
                }))
            }
            oplog_entry::Entry::DescribeResource(describe_resource) => Ok(
                PublicOplogEntry::DescribeResource(DescribeResourceParameters {
                    timestamp: describe_resource
                        .timestamp
                        .ok_or("Missing timestamp field")?
                        .into(),
                    id: WorkerResourceId(describe_resource.resource_id),
                    resource_name: describe_resource.resource_name,
                    resource_params: describe_resource
                        .resource_params
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<Vec<ValueAndType>, String>>()?,
                }),
            ),
            oplog_entry::Entry::Log(log) => Ok(PublicOplogEntry::Log(LogParameters {
                level: log.level().into(),
                timestamp: log.timestamp.ok_or("Missing timestamp field")?.into(),
                context: log.context,
                message: log.message,
            })),
            oplog_entry::Entry::Restart(restart) => {
                Ok(PublicOplogEntry::Restart(TimestampParameter {
                    timestamp: restart.timestamp.ok_or("Missing timestamp field")?.into(),
                }))
            }
        }
    }
}

impl TryFrom<PublicOplogEntry> for golem_api_grpc::proto::golem::worker::OplogEntry {
    type Error = String;

    fn try_from(value: PublicOplogEntry) -> Result<Self, String> {
        Ok(match value {
            PublicOplogEntry::Create(create) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Create(
                    golem_api_grpc::proto::golem::worker::CreateParameters {
                        timestamp: Some(create.timestamp.into()),
                        worker_id: Some(create.worker_id.into()),
                        component_version: create.component_version,
                        args: create.args,
                        env: create.env.into_iter().collect(),
                        account_id: Some(create.account_id.into()),
                        parent: create.parent.map(Into::into),
                        component_size: create.component_size,
                        initial_total_linear_memory_size: create.initial_total_linear_memory_size,
                    },
                )),
            },
            PublicOplogEntry::ImportedFunctionInvoked(imported_function_invoked) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ImportedFunctionInvoked(
                        golem_api_grpc::proto::golem::worker::ImportedFunctionInvokedParameters {
                            timestamp: Some(imported_function_invoked.timestamp.into()),
                            function_name: imported_function_invoked.function_name,
                            request: Some(imported_function_invoked.request.try_into().map_err(
                                |errors: Vec<String>| {
                                    format!("Failed to convert request: {}", errors.join(", "))
                                },
                            )?),
                            response: Some(imported_function_invoked.response.try_into().map_err(
                                |errors: Vec<String>| {
                                    format!("Failed to convert response: {}", errors.join(", "))
                                },
                            )?),
                            wrapped_function_type: Some(
                                imported_function_invoked.wrapped_function_type.into(),
                            ),
                        },
                    )),
                }
            }
            PublicOplogEntry::ExportedFunctionInvoked(exported_function_invoked) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ExportedFunctionInvoked(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionInvokedParameters {
                            timestamp: Some(exported_function_invoked.timestamp.into()),
                            function_name: exported_function_invoked.function_name,
                            request: exported_function_invoked
                                .request
                                .into_iter()
                                .map(|value| {
                                    value.try_into().map_err(|errors: Vec<String>| {
                                        format!("Failed to convert request: {}", errors.join(", "))
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?,
                            idempotency_key: Some(exported_function_invoked.idempotency_key.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::ExportedFunctionCompleted(exported_function_completed) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ExportedFunctionCompleted(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionCompletedParameters {
                            timestamp: Some(exported_function_completed.timestamp.into()),
                            response: Some(
                                exported_function_completed.response.try_into().map_err(
                                    |errors: Vec<String>| {
                                        format!("Failed to convert response: {}", errors.join(", "))
                                    },
                                )?,
                            ),
                            consumed_fuel: exported_function_completed.consumed_fuel,
                        },
                    )),
                }
            }
            PublicOplogEntry::Suspend(suspend) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Suspend(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(suspend.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::Error(error) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Error(
                    golem_api_grpc::proto::golem::worker::ErrorParameters {
                        timestamp: Some(error.timestamp.into()),
                        error: error.error,
                    },
                )),
            },
            PublicOplogEntry::NoOp(no_op) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::NoOp(
                    golem_api_grpc::proto::golem::worker::TimestampParameter {
                        timestamp: Some(no_op.timestamp.into()),
                    },
                )),
            },
            PublicOplogEntry::Jump(jump) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Jump(
                    golem_api_grpc::proto::golem::worker::JumpParameters {
                        timestamp: Some(jump.timestamp.into()),
                        start: jump.jump.start.into(),
                        end: jump.jump.end.into(),
                    },
                )),
            },
            PublicOplogEntry::Interrupted(interrupted) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Interrupted(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(interrupted.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::Exited(exited) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Exited(
                    golem_api_grpc::proto::golem::worker::TimestampParameter {
                        timestamp: Some(exited.timestamp.into()),
                    },
                )),
            },
            PublicOplogEntry::ChangeRetryPolicy(change_retry_policy) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::ChangeRetryPolicy(
                        golem_api_grpc::proto::golem::worker::ChangeRetryPolicyParameters {
                            timestamp: Some(change_retry_policy.timestamp.into()),
                            retry_policy: Some(change_retry_policy.new_policy.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::BeginAtomicRegion(begin_atomic_region) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::BeginAtomicRegion(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(begin_atomic_region.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::EndAtomicRegion(end_atomic_region) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::EndAtomicRegion(
                        golem_api_grpc::proto::golem::worker::EndAtomicRegionParameters {
                            timestamp: Some(end_atomic_region.timestamp.into()),
                            begin_index: end_atomic_region.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::BeginRemoteWrite(begin_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::BeginRemoteWrite(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(begin_remote_write.timestamp.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::EndRemoteWrite(end_remote_write) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::EndRemoteWrite(
                        golem_api_grpc::proto::golem::worker::EndRemoteWriteParameters {
                            timestamp: Some(end_remote_write.timestamp.into()),
                            begin_index: end_remote_write.begin_index.into(),
                        },
                    )),
                }
            }
            PublicOplogEntry::PendingWorkerInvocation(pending_worker_invocation) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PendingWorkerInvocation(
                        golem_api_grpc::proto::golem::worker::PendingWorkerInvocationParameters {
                            timestamp: Some(pending_worker_invocation.timestamp.into()),
                            invocation: Some(pending_worker_invocation.invocation.try_into()?),
                        },
                    )),
                }
            }
            PublicOplogEntry::PendingUpdate(pending_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::PendingUpdate(
                        golem_api_grpc::proto::golem::worker::PendingUpdateParameters {
                            timestamp: Some(pending_update.timestamp.into()),
                            target_version: pending_update.target_version,
                            update_description: Some(pending_update.description.into()),
                        },
                    )),
                }
            }
            PublicOplogEntry::SuccessfulUpdate(successful_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::SuccessfulUpdate(
                        golem_api_grpc::proto::golem::worker::SuccessfulUpdateParameters {
                            timestamp: Some(successful_update.timestamp.into()),
                            target_version: successful_update.target_version,
                            new_component_size: successful_update.new_component_size,
                        },
                    )),
                }
            }
            PublicOplogEntry::FailedUpdate(failed_update) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::FailedUpdate(
                        golem_api_grpc::proto::golem::worker::FailedUpdateParameters {
                            timestamp: Some(failed_update.timestamp.into()),
                            target_version: failed_update.target_version,
                            details: failed_update.details,
                        },
                    )),
                }
            }
            PublicOplogEntry::GrowMemory(grow_memory) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::GrowMemory(
                        golem_api_grpc::proto::golem::worker::GrowMemoryParameters {
                            timestamp: Some(grow_memory.timestamp.into()),
                            delta: grow_memory.delta,
                        },
                    )),
                }
            }
            PublicOplogEntry::CreateResource(create_resource) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::CreateResource(
                        golem_api_grpc::proto::golem::worker::CreateResourceParameters {
                            timestamp: Some(create_resource.timestamp.into()),
                            resource_id: create_resource.id.0,
                        },
                    )),
                }
            }
            PublicOplogEntry::DropResource(drop_resource) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::DropResource(
                        golem_api_grpc::proto::golem::worker::DropResourceParameters {
                            timestamp: Some(drop_resource.timestamp.into()),
                            resource_id: drop_resource.id.0,
                        },
                    )),
                }
            }
            PublicOplogEntry::DescribeResource(describe_resource) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::DescribeResource(
                        golem_api_grpc::proto::golem::worker::DescribeResourceParameters {
                            timestamp: Some(describe_resource.timestamp.into()),
                            resource_id: describe_resource.id.0,
                            resource_name: describe_resource.resource_name,
                            resource_params: describe_resource
                                .resource_params
                                .into_iter()
                                .map(|value| {
                                    value.try_into().map_err(|errors: Vec<String>| {
                                        format!("Failed to convert request: {}", errors.join(", "))
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?,
                        },
                    )),
                }
            }
            PublicOplogEntry::Log(log) => golem_api_grpc::proto::golem::worker::OplogEntry {
                entry: Some(oplog_entry::Entry::Log(
                    golem_api_grpc::proto::golem::worker::LogParameters {
                        timestamp: Some(log.timestamp.into()),
                        level: Into::<golem_api_grpc::proto::golem::worker::OplogLogLevel>::into(
                            log.level,
                        ) as i32,
                        context: log.context,
                        message: log.message,
                    },
                )),
            },
            PublicOplogEntry::Restart(restart) => {
                golem_api_grpc::proto::golem::worker::OplogEntry {
                    entry: Some(oplog_entry::Entry::Restart(
                        golem_api_grpc::proto::golem::worker::TimestampParameter {
                            timestamp: Some(restart.timestamp.into()),
                        },
                    )),
                }
            }
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WrappedFunctionType>
    for PublicWrappedFunctionType
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WrappedFunctionType,
    ) -> Result<Self, Self::Error> {
        match value.r#type() {
            wrapped_function_type::Type::ReadLocal => {
                Ok(PublicWrappedFunctionType::ReadLocal(Empty))
            }
            wrapped_function_type::Type::WriteLocal => {
                Ok(PublicWrappedFunctionType::WriteLocal(Empty))
            }
            wrapped_function_type::Type::ReadRemote => {
                Ok(PublicWrappedFunctionType::ReadRemote(Empty))
            }
            wrapped_function_type::Type::WriteRemote => {
                Ok(PublicWrappedFunctionType::WriteRemote(Empty))
            }
            wrapped_function_type::Type::WriteRemoteBatched => Ok(
                PublicWrappedFunctionType::WriteRemoteBatched(WriteRemoteBatchedParameters {
                    index: value.oplog_index.map(OplogIndex::from_u64),
                }),
            ),
        }
    }
}

impl From<PublicWrappedFunctionType> for golem_api_grpc::proto::golem::worker::WrappedFunctionType {
    fn from(value: PublicWrappedFunctionType) -> Self {
        match value {
            PublicWrappedFunctionType::ReadLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadLocal as i32,
                    oplog_index: None,
                }
            }
            PublicWrappedFunctionType::WriteLocal(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteLocal as i32,
                    oplog_index: None,
                }
            }
            PublicWrappedFunctionType::ReadRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::ReadRemote as i32,
                    oplog_index: None,
                }
            }
            PublicWrappedFunctionType::WriteRemote(_) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemote as i32,
                    oplog_index: None,
                }
            }
            PublicWrappedFunctionType::WriteRemoteBatched(parameters) => {
                golem_api_grpc::proto::golem::worker::WrappedFunctionType {
                    r#type: wrapped_function_type::Type::WriteRemoteBatched as i32,
                    oplog_index: parameters.index.map(|index| index.into()),
                }
            }
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::RetryPolicy> for PublicRetryConfig {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::RetryPolicy,
    ) -> Result<Self, Self::Error> {
        Ok(PublicRetryConfig {
            max_attempts: value.max_attempts,
            min_delay: Duration::from_millis(value.min_delay),
            max_delay: Duration::from_millis(value.max_delay),
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        })
    }
}

impl From<PublicRetryConfig> for golem_api_grpc::proto::golem::worker::RetryPolicy {
    fn from(value: PublicRetryConfig) -> Self {
        golem_api_grpc::proto::golem::worker::RetryPolicy {
            max_attempts: value.max_attempts,
            min_delay: value.min_delay.as_millis() as u64,
            max_delay: value.max_delay.as_millis() as u64,
            multiplier: value.multiplier,
            max_jitter_factor: value.max_jitter_factor,
        }
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogLogLevel> for LogLevel {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogLogLevel) -> Self {
        match value {
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace => LogLevel::Trace,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug => LogLevel::Debug,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo => LogLevel::Info,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn => LogLevel::Warn,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError => LogLevel::Error,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical => {
                LogLevel::Critical
            }
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr => LogLevel::Stderr,
            golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout => LogLevel::Stdout,
        }
    }
}

impl From<LogLevel> for golem_api_grpc::proto::golem::worker::OplogLogLevel {
    fn from(value: LogLevel) -> Self {
        match value {
            LogLevel::Trace => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogTrace,
            LogLevel::Debug => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogDebug,
            LogLevel::Info => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogInfo,
            LogLevel::Warn => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogWarn,
            LogLevel::Error => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogError,
            LogLevel::Critical => {
                golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogCritical
            }
            LogLevel::Stderr => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStderr,
            LogLevel::Stdout => golem_api_grpc::proto::golem::worker::OplogLogLevel::OplogStdout,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerInvocation> for PublicWorkerInvocation {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::WorkerInvocation,
    ) -> Result<Self, Self::Error> {
        match value.invocation.ok_or("Missing invocation field")? {
            worker_invocation::Invocation::ExportedFunction(exported_function) => Ok(
                PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                    idempotency_key: exported_function
                        .idempotency_key
                        .ok_or("Missing idempotency_key field")?
                        .into(),
                    full_function_name: exported_function.function_name,
                    function_input: if exported_function.valid_input {
                        Some(
                            exported_function
                                .input
                                .into_iter()
                                .map(TryInto::try_into)
                                .collect::<Result<Vec<ValueAndType>, String>>()?,
                        )
                    } else {
                        None
                    },
                }),
            ),
            worker_invocation::Invocation::ManualUpdate(manual_update) => Ok(
                PublicWorkerInvocation::ManualUpdate(ManualUpdateParameters {
                    target_version: manual_update,
                }),
            ),
        }
    }
}

impl TryFrom<PublicWorkerInvocation> for golem_api_grpc::proto::golem::worker::WorkerInvocation {
    type Error = String;

    fn try_from(value: PublicWorkerInvocation) -> Result<Self, Self::Error> {
        Ok(match value {
            PublicWorkerInvocation::ExportedFunction(exported_function) => {
                golem_api_grpc::proto::golem::worker::WorkerInvocation {
                    invocation: Some(worker_invocation::Invocation::ExportedFunction(
                        golem_api_grpc::proto::golem::worker::ExportedFunctionInvocationParameters {
                            idempotency_key: Some(exported_function.idempotency_key.into()),
                            function_name: exported_function.full_function_name,
                            valid_input: exported_function.function_input.is_some(),
                            input: exported_function
                                .function_input
                                .unwrap_or_default()
                                .into_iter()
                                .map(|input| input.try_into().map_err(
                                    |errors: Vec<String>| {
                                        format!("Failed to convert request: {}", errors.join(", "))
                                    },
                                )).collect::<Result<Vec<_>, _>>()?,
                        },
                    )),
                }
            }
            PublicWorkerInvocation::ManualUpdate(manual_update) => {
                golem_api_grpc::proto::golem::worker::WorkerInvocation {
                    invocation: Some(worker_invocation::Invocation::ManualUpdate(
                        manual_update.target_version,
                    )),
                }
            }
        })
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::UpdateDescription> for PublicUpdateDescription {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::UpdateDescription,
    ) -> Result<Self, Self::Error> {
        match value.description.ok_or("Missing description field")? {
            golem_api_grpc::proto::golem::worker::update_description::Description::AutoUpdate(_) => {
                Ok(PublicUpdateDescription::Automatic(Empty))
            }
            golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                snapshot_based,
            ) => Ok(PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload: snapshot_based.payload,
            })),
        }
    }
}

impl From<PublicUpdateDescription> for golem_api_grpc::proto::golem::worker::UpdateDescription {
    fn from(value: PublicUpdateDescription) -> Self {
        match value {
            PublicUpdateDescription::Automatic(_) => golem_api_grpc::proto::golem::worker::UpdateDescription {
                description: Some(
                    golem_api_grpc::proto::golem::worker::update_description::Description::AutoUpdate(
                        golem_api_grpc::proto::golem::common::Empty {},
                    ),
                ),
            },
            PublicUpdateDescription::SnapshotBased(snapshot_based) => {
                golem_api_grpc::proto::golem::worker::UpdateDescription {
                    description: Some(
                        golem_api_grpc::proto::golem::worker::update_description::Description::SnapshotBased(
                            golem_api_grpc::proto::golem::worker::SnapshotBasedUpdateParameters {
                                payload: snapshot_based.payload
                            }
                        ),
                    ),
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Object)]
pub struct OplogCursor {
    pub next_oplog_index: u64,
    pub current_component_version: u64,
}

impl ParseFromParameter for OplogCursor {
    fn parse_from_parameter(value: &str) -> ParseResult<Self> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 2 {
            return Err("Invalid oplog cursor".into());
        }
        let next_oplog_index = parts[0]
            .parse()
            .map_err(|_| "Invalid index in the oplog cursor")?;
        let current_component_version = parts[1]
            .parse()
            .map_err(|_| "Invalid component version in the oplog cursor")?;
        Ok(OplogCursor {
            next_oplog_index,
            current_component_version,
        })
    }
}

impl Display for OplogCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            self.next_oplog_index, self.current_component_version
        )
    }
}

impl From<golem_api_grpc::proto::golem::worker::OplogCursor> for OplogCursor {
    fn from(value: golem_api_grpc::proto::golem::worker::OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_version: value.current_component_version,
        }
    }
}

impl From<OplogCursor> for golem_api_grpc::proto::golem::worker::OplogCursor {
    fn from(value: OplogCursor) -> Self {
        Self {
            next_oplog_index: value.next_oplog_index,
            current_component_version: value.current_component_version,
        }
    }
}

#[cfg(test)]
mod tests {

    use super::{
        ChangeRetryPolicyParameters, CreateParameters, DescribeResourceParameters, Empty,
        EndRegionParameters, ErrorParameters, ExportedFunctionCompletedParameters,
        ExportedFunctionInvokedParameters, ExportedFunctionParameters, FailedUpdateParameters,
        GrowMemoryParameters, ImportedFunctionInvokedParameters, JumpParameters, LogParameters,
        PendingUpdateParameters, PendingWorkerInvocationParameters, PublicOplogEntry,
        PublicRetryConfig, PublicUpdateDescription, PublicWorkerInvocation,
        PublicWrappedFunctionType, ResourceParameters, SnapshotBasedUpdateParameters,
        SuccessfulUpdateParameters, TimestampParameter,
    };
    use crate::model::oplog::{LogLevel, OplogIndex, WorkerResourceId};
    use crate::model::regions::OplogRegion;
    use crate::model::{AccountId, ComponentId, IdempotencyKey, Timestamp, WorkerId};
    use golem_wasm_ast::analysis::analysed_type::{field, list, r#enum, record, s16, str, u64};
    use golem_wasm_rpc::{Value, ValueAndType};
    use poem_openapi::types::ToJSON;
    use test_r::test;
    use uuid::Uuid;

    fn rounded_ts(ts: Timestamp) -> Timestamp {
        Timestamp::from(ts.to_millis())
    }

    #[test]
    fn create_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Create(CreateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            worker_id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
                ),
                worker_name: "test1".to_string(),
            },
            component_version: 1,
            args: vec!["a".to_string(), "b".to_string()],
            env: vec![("x".to_string(), "y".to_string())]
                .into_iter()
                .collect(),
            account_id: AccountId {
                value: "account_id".to_string(),
            },
            parent: Some(WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("13A5C8D4-F05E-4E23-B982-F4D413E181CB").unwrap(),
                ),
                worker_name: "test2".to_string(),
            }),
            component_size: 100_000_000,
            initial_total_linear_memory_size: 200_000_000,
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn imported_function_invoked_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            function_name: "test".to_string(),
            request: ValueAndType {
                value: Value::String("test".to_string()),
                typ: str(),
            },
            response: ValueAndType {
                value: Value::List(vec![Value::U64(1)]),
                typ: list(u64()),
            },
            wrapped_function_type: PublicWrappedFunctionType::ReadRemote(Empty),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn exported_function_invoked_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::ExportedFunctionInvoked(ExportedFunctionInvokedParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            function_name: "test".to_string(),
            request: vec![
                ValueAndType {
                    value: Value::String("test".to_string()),
                    typ: str(),
                },
                ValueAndType {
                    value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                    typ: record(vec![field("x", s16()), field("y", s16())]),
                },
            ],
            idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn exported_function_completed_serialization_poem_serde_equivalence() {
        let entry =
            PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
                timestamp: rounded_ts(Timestamp::now_utc()),
                response: ValueAndType {
                    value: Value::Enum(1),
                    typ: r#enum(&["red", "green", "blue"]),
                },
                consumed_fuel: 100,
            });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn suspend_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Suspend(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn error_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Error(ErrorParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            error: "test".to_string(),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn no_op_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::NoOp(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn jump_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Jump(JumpParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            jump: OplogRegion {
                start: OplogIndex::from_u64(1),
                end: OplogIndex::from_u64(2),
            },
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn interrupted_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Interrupted(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn exited_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Exited(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn change_retry_policy_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::ChangeRetryPolicy(ChangeRetryPolicyParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            new_policy: PublicRetryConfig {
                max_attempts: 10,
                min_delay: std::time::Duration::from_secs(1),
                max_delay: std::time::Duration::from_secs(10),
                multiplier: 2.0,
                max_jitter_factor: Some(0.1),
            },
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn begin_atomic_region_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::BeginAtomicRegion(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn end_atomic_region_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::EndAtomicRegion(EndRegionParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            begin_index: OplogIndex::from_u64(1),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn begin_remote_write_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::BeginRemoteWrite(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn end_remote_write_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::EndRemoteWrite(EndRegionParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            begin_index: OplogIndex::from_u64(1),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn pending_worker_invocation_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::PendingWorkerInvocation(PendingWorkerInvocationParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            invocation: PublicWorkerInvocation::ExportedFunction(ExportedFunctionParameters {
                idempotency_key: IdempotencyKey::new("idempotency_key".to_string()),
                full_function_name: "test".to_string(),
                function_input: Some(vec![
                    ValueAndType {
                        value: Value::String("test".to_string()),
                        typ: str(),
                    },
                    ValueAndType {
                        value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                        typ: record(vec![field("x", s16()), field("y", s16())]),
                    },
                ]),
            }),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn pending_update_serialization_poem_serde_equivalence_1() {
        let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            target_version: 1,
            description: PublicUpdateDescription::SnapshotBased(SnapshotBasedUpdateParameters {
                payload: "test".as_bytes().to_vec(),
            }),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn pending_update_serialization_poem_serde_equivalence_2() {
        let entry = PublicOplogEntry::PendingUpdate(PendingUpdateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            target_version: 1,
            description: PublicUpdateDescription::Automatic(Empty),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn successful_update_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::SuccessfulUpdate(SuccessfulUpdateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            target_version: 1,
            new_component_size: 100_000_000,
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn failed_update_serialization_poem_serde_equivalence_1() {
        let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            target_version: 1,
            details: Some("test".to_string()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn failed_update_serialization_poem_serde_equivalence_2() {
        let entry = PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            target_version: 1,
            details: None,
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn grow_memory_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::GrowMemory(GrowMemoryParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            delta: 100_000_000,
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn create_resource_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::CreateResource(ResourceParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            id: WorkerResourceId(100),
        });

        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn drop_resource_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::DropResource(ResourceParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            id: WorkerResourceId(100),
        });

        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn describe_resource_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::DescribeResource(DescribeResourceParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            id: WorkerResourceId(100),
            resource_name: "test".to_string(),
            resource_params: vec![
                ValueAndType {
                    value: Value::String("test".to_string()),
                    typ: str(),
                },
                ValueAndType {
                    value: Value::Record(vec![Value::S16(1), Value::S16(-1)]),
                    typ: record(vec![field("x", s16()), field("y", s16())]),
                },
            ],
        });

        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn log_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Log(LogParameters {
            timestamp: rounded_ts(Timestamp::now_utc()),
            level: LogLevel::Stderr,
            context: "test".to_string(),
            message: "test".to_string(),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }

    #[test]
    fn restart_serialization_poem_serde_equivalence() {
        let entry = PublicOplogEntry::Restart(TimestampParameter {
            timestamp: rounded_ts(Timestamp::now_utc()),
        });
        let serialized = entry.to_json_string();
        let deserialized: PublicOplogEntry = serde_json::from_str(&serialized).unwrap();
        assert_eq!(entry, deserialized);
    }
}
