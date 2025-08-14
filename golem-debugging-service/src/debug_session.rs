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

use crate::from_value::{
    Bucket, BucketAndKey, BucketAndKeys, BucketKeyValue, BucketKeyValues, Container,
    ContainerAndObject, ContainerAndObjects, ContainerCopyObjectInfo, ContainerObjectBeginEnd,
    ContainerObjectLength, FromValue, UpdateWorkerInfo,
};
use crate::model::params::PlaybackOverride;
use async_trait::async_trait;
use bincode::Encode;
use golem_common::model::auth::Namespace;
use golem_common::model::oplog::{
    DurableFunctionType, OplogEntry, OplogIndex, OplogPayload, WorkerError,
};
use golem_common::model::public_oplog::{
    CreateParameters, DescribeResourceParameters, ExportedFunctionCompletedParameters,
    FailedUpdateParameters, GrowMemoryParameters, ImportedFunctionInvokedParameters, LogParameters,
    PublicDurableFunctionType, PublicOplogEntry, ResourceParameters,
};
use golem_common::model::{
    IdempotencyKey, OwnedWorkerId, PluginInstallationId, PromiseId, RetryConfig, WorkerId,
    WorkerMetadata,
};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::wasmtime::ResourceTypeId;
use golem_wasm_rpc::{Value, ValueAndType};
use golem_worker_executor::durable_host::http::serialized::{
    SerializableErrorCode, SerializableHttpRequest, SerializableResponse,
};
use golem_worker_executor::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddresses,
    SerializableStreamError,
};
use golem_worker_executor::durable_host::wasm_rpc::serialized::{
    SerializableInvokeRequest, SerializableInvokeResult,
};
use golem_worker_executor::services::blob_store::ObjectMetadata;
use golem_worker_executor::services::rpc::RpcError;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// A shared debug session which will be internally used by the custom oplog service
// dedicated to running debug executor
#[async_trait]
pub trait DebugSessions: Send + Sync {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId;
    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData>;

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData>;

    async fn update(
        &self,
        debug_session_id: DebugSessionId,
        target_oplog_index: OplogIndex,
        playback_overrides: Option<PlaybackOverridesInternal>,
    ) -> Option<DebugSessionData>;

    async fn update_oplog_index(
        &self,
        debug_session_id: DebugSessionId,
        oplog_index: OplogIndex,
    ) -> Option<DebugSessionData>;
}
pub struct DebugSessionsDefault {
    pub session: Arc<Mutex<HashMap<DebugSessionId, DebugSessionData>>>,
}

impl Default for DebugSessionsDefault {
    fn default() -> Self {
        Self {
            session: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl DebugSessions for DebugSessionsDefault {
    async fn insert(
        &self,
        debug_session_id: DebugSessionId,
        session_value: DebugSessionData,
    ) -> DebugSessionId {
        let mut session = self.session.lock().unwrap();
        session.insert(debug_session_id.clone(), session_value);
        debug_session_id
    }

    async fn get(&self, debug_session_id: &DebugSessionId) -> Option<DebugSessionData> {
        let session = self.session.lock().unwrap();
        session.get(debug_session_id).cloned()
    }

    async fn remove(&self, debug_session_id: DebugSessionId) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        session.remove(&debug_session_id)
    }

    async fn update(
        &self,
        debug_session_id: DebugSessionId,
        target_oplog_index: OplogIndex,
        playback_overrides: Option<PlaybackOverridesInternal>,
    ) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        let session_data = session.get_mut(&debug_session_id);
        if let Some(session_data) = session_data {
            session_data.target_oplog_index = Some(target_oplog_index);
            if let Some(playback_overrides) = playback_overrides {
                session_data.playback_overrides = playback_overrides
            }
            Some(session_data.clone())
        } else {
            None
        }
    }

    async fn update_oplog_index(
        &self,
        debug_session_id: DebugSessionId,
        oplog_index: OplogIndex,
    ) -> Option<DebugSessionData> {
        let mut session = self.session.lock().unwrap();
        let session_data = session.get_mut(&debug_session_id);
        if let Some(session_data) = session_data {
            session_data.current_oplog_index = oplog_index;
            Some(session_data.clone())
        } else {
            None
        }
    }
}

#[derive(Clone)]
pub struct DebugSessionData {
    pub worker_metadata: WorkerMetadata,
    pub target_oplog_index: Option<OplogIndex>,
    pub playback_overrides: PlaybackOverridesInternal,
    // The current status of the oplog index being replayed and possibly
    // index of newly added oplog entries as part of going live in between host functions
    pub current_oplog_index: OplogIndex,
}

#[derive(Debug, Clone)]
pub struct PlaybackOverridesInternal {
    pub overrides: HashMap<OplogIndex, OplogEntry>,
}

impl PlaybackOverridesInternal {
    pub fn empty() -> PlaybackOverridesInternal {
        PlaybackOverridesInternal {
            overrides: HashMap::new(),
        }
    }
    pub fn from_playback_override(
        playback_overrides: Vec<PlaybackOverride>,
        current_index: OplogIndex,
    ) -> Result<Self, String> {
        let mut overrides = HashMap::new();
        for override_data in playback_overrides {
            let oplog_index = override_data.index;
            if oplog_index <= current_index {
                return Err(
                    "Cannot create overrides for oplogs indices that are in the past".to_string(),
                );
            }

            let public_oplog_entry: PublicOplogEntry = override_data.oplog;
            let oplog_entry = get_oplog_entry_from_public_oplog_entry(public_oplog_entry)?;
            overrides.insert(oplog_index, oplog_entry);
        }
        Ok(Self { overrides })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DebugSessionId(WorkerId);

impl Serialize for DebugSessionId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl DebugSessionId {
    pub fn new(worker_id: OwnedWorkerId) -> Self {
        DebugSessionId(worker_id.worker_id)
    }

    pub fn worker_id(&self) -> WorkerId {
        self.0.clone()
    }
}
impl Display for DebugSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone)]
pub struct ActiveSessionData {
    pub cloud_namespace: Namespace,
    pub worker_id: WorkerId,
}

impl ActiveSessionData {
    pub fn new(cloud_namespace: Namespace, worker_id: WorkerId) -> Self {
        Self {
            cloud_namespace,
            worker_id,
        }
    }
}

fn get_oplog_entry_from_public_oplog_entry(
    public_oplog_entry: PublicOplogEntry,
) -> Result<OplogEntry, String> {
    match public_oplog_entry {
        PublicOplogEntry::ExportedFunctionCompleted(ExportedFunctionCompletedParameters {
            timestamp,
            consumed_fuel,
            response,
        }) => {
            let serialize =
                golem_common::serialization::serialize(&response).map_err(|e| e.to_string())?;

            Ok(OplogEntry::ExportedFunctionCompleted {
                timestamp,
                consumed_fuel,
                response: OplogPayload::Inline(serialize.to_vec()),
            })
        }

        PublicOplogEntry::Create(CreateParameters {
            timestamp,
            worker_id,
            component_version,
            args,
            env,
            project_id,
            created_by,
            wasi_config_vars,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
        }) => Ok(OplogEntry::Create {
            timestamp,
            worker_id,
            component_version,
            args,
            env: env.into_iter().collect(),
            project_id,
            created_by,
            wasi_config_vars: wasi_config_vars.into(),
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins: initial_active_plugins
                .into_iter()
                .map(|x| x.installation_id)
                .collect(),
        }),
        PublicOplogEntry::ImportedFunctionInvoked(ImportedFunctionInvokedParameters {
            timestamp,
            function_name,
            response,
            durable_function_type: wrapped_function_type,
            request,
        }) => {
            let response: OplogPayload = convert_response_value_and_type_to_oplog_payload(
                function_name.as_str(),
                &response,
            )?;

            let request: OplogPayload =
                convert_request_value_and_type_to_oplog_payload(function_name.as_str(), &request)?;

            let durable_function_type = match wrapped_function_type {
                PublicDurableFunctionType::ReadLocal(_) => DurableFunctionType::ReadLocal,
                PublicDurableFunctionType::WriteLocal(_) => DurableFunctionType::WriteLocal,
                PublicDurableFunctionType::ReadRemote(_) => DurableFunctionType::ReadRemote,
                PublicDurableFunctionType::WriteRemote(_) => DurableFunctionType::WriteRemote,
                PublicDurableFunctionType::WriteRemoteBatched(params) => {
                    DurableFunctionType::WriteRemoteBatched(params.index)
                }
            };

            Ok(OplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                response,
                durable_function_type,
            })
        }
        PublicOplogEntry::ExportedFunctionInvoked(exported_function_invoked_parameters) => {
            // We discard the type info provided by the user to encode it as oplog payload by converting it to
            // golem_wasm_rpc::protobuf::Val
            let vals = exported_function_invoked_parameters
                .request
                .into_iter()
                .map(|x| golem_wasm_rpc::protobuf::Val::from(x.value))
                .collect::<Vec<_>>();

            let serialized = golem_common::serialization::serialize(&vals)?;
            let oplog_payload = OplogPayload::Inline(serialized.to_vec());

            Ok(OplogEntry::ExportedFunctionInvoked {
                timestamp: exported_function_invoked_parameters.timestamp,
                function_name: exported_function_invoked_parameters.function_name,
                request: oplog_payload,
                idempotency_key: exported_function_invoked_parameters.idempotency_key,
                trace_id: exported_function_invoked_parameters.trace_id,
                trace_states: exported_function_invoked_parameters.trace_states,
                invocation_context: vec![], // TODO: Make decode_public_span_data public in OSS and use it here
            })
        }

        PublicOplogEntry::Suspend(timestamp_parameter) => Ok(OplogEntry::Suspend {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::Error(error) => Ok(OplogEntry::Error {
            timestamp: error.timestamp,
            error: WorkerError::Unknown(error.error),
        }),
        PublicOplogEntry::NoOp(timestamp_parameter) => Ok(OplogEntry::NoOp {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::Jump(jump) => Ok(OplogEntry::Jump {
            timestamp: jump.timestamp,
            jump: jump.jump,
        }),
        PublicOplogEntry::Interrupted(interrupted) => Ok(OplogEntry::Interrupted {
            timestamp: interrupted.timestamp,
        }),
        PublicOplogEntry::Exited(exited) => Ok(OplogEntry::Exited {
            timestamp: exited.timestamp,
        }),
        PublicOplogEntry::ChangeRetryPolicy(change_retry_policy) => {
            Ok(OplogEntry::ChangeRetryPolicy {
                timestamp: change_retry_policy.timestamp,
                new_policy: RetryConfig {
                    max_attempts: change_retry_policy.new_policy.max_attempts,
                    min_delay: change_retry_policy.new_policy.min_delay,
                    max_delay: change_retry_policy.new_policy.max_delay,
                    multiplier: change_retry_policy.new_policy.multiplier,
                    max_jitter_factor: change_retry_policy.new_policy.max_jitter_factor,
                },
            })
        }
        PublicOplogEntry::BeginAtomicRegion(_) => {
            Err("Cannot override an oplog with a begin atomic region oplog".to_string())
        }
        PublicOplogEntry::EndAtomicRegion(_) => {
            Err("Cannot override an oplog with a end atomic region oplog".to_string())
        }
        PublicOplogEntry::BeginRemoteWrite(_) => {
            Err("Cannot override an oplog with a begin atomic write oplog".to_string())
        }
        PublicOplogEntry::EndRemoteWrite(_) => {
            Err("Cannot override an oplog with an end atomic write oplog".to_string())
        }
        PublicOplogEntry::PendingWorkerInvocation(_) => {
            Err("Cannot override an oplog with a pending worker invocation".to_string())?
        }
        PublicOplogEntry::PendingUpdate(_) => {
            Err("Cannot override an oplog with a pending update".to_string())?
        }
        PublicOplogEntry::SuccessfulUpdate(successful_update_params) => {
            let plugin_installation_ids: HashSet<PluginInstallationId> = successful_update_params
                .new_active_plugins
                .iter()
                .map(|plugin| plugin.installation_id.clone())
                .collect();

            Ok(OplogEntry::SuccessfulUpdate {
                timestamp: successful_update_params.timestamp,
                target_version: successful_update_params.target_version,
                new_component_size: successful_update_params.new_component_size,
                new_active_plugins: plugin_installation_ids,
            })
        }
        PublicOplogEntry::FailedUpdate(FailedUpdateParameters {
            timestamp,
            target_version,
            details,
        }) => Ok(OplogEntry::FailedUpdate {
            timestamp,
            target_version,
            details,
        }),
        PublicOplogEntry::GrowMemory(GrowMemoryParameters { timestamp, delta }) => {
            Ok(OplogEntry::GrowMemory { timestamp, delta })
        }
        PublicOplogEntry::CreateResource(ResourceParameters {
            timestamp,
            id,
            owner,
            name,
        }) => Ok(OplogEntry::CreateResource {
            timestamp,
            id,
            resource_type_id: ResourceTypeId { owner, name },
        }),
        PublicOplogEntry::DropResource(ResourceParameters {
            timestamp,
            id,
            owner,
            name,
        }) => Ok(OplogEntry::DropResource {
            timestamp,
            id,
            resource_type_id: ResourceTypeId { owner, name },
        }),
        PublicOplogEntry::DescribeResource(DescribeResourceParameters {
            timestamp,
            id,
            resource_owner,
            resource_name,
            resource_params,
        }) => {
            let resource_params = resource_params
                .iter()
                .map(|value_and_type| value_and_type.to_string()) // This will call to_string of wasm wave
                .collect::<Vec<_>>();

            Ok(OplogEntry::DescribeResource {
                timestamp,
                id,
                resource_type_id: ResourceTypeId {
                    owner: resource_owner,
                    name: resource_name,
                },
                indexed_resource_parameters: resource_params,
            })
        }
        PublicOplogEntry::Log(LogParameters {
            timestamp,
            level,
            context,
            message,
        }) => Ok(OplogEntry::Log {
            timestamp,
            level,
            context,
            message,
        }),
        PublicOplogEntry::Restart(timestamp_parameter) => Ok(OplogEntry::Restart {
            timestamp: timestamp_parameter.timestamp,
        }),
        PublicOplogEntry::ActivatePlugin(activate_plugin_params) => {
            Ok(OplogEntry::ActivatePlugin {
                timestamp: activate_plugin_params.timestamp,
                plugin: activate_plugin_params.plugin.installation_id,
            })
        }
        PublicOplogEntry::DeactivatePlugin(deactivate_plugin_params) => {
            Ok(OplogEntry::DeactivatePlugin {
                timestamp: deactivate_plugin_params.timestamp,
                plugin: deactivate_plugin_params.plugin.installation_id,
            })
        }
        PublicOplogEntry::Revert(revert_params) => Ok(OplogEntry::Revert {
            timestamp: revert_params.timestamp,
            dropped_region: revert_params.dropped_region,
        }),
        PublicOplogEntry::CancelInvocation(cancel_invocation_params) => {
            Ok(OplogEntry::CancelPendingInvocation {
                timestamp: cancel_invocation_params.timestamp,
                idempotency_key: cancel_invocation_params.idempotency_key,
            })
        }
        PublicOplogEntry::StartSpan(start_span) => Ok(OplogEntry::StartSpan {
            timestamp: start_span.timestamp,
            span_id: start_span.span_id,
            parent_id: start_span.parent_id,
            linked_context_id: start_span.linked_context,
            attributes: start_span
                .attributes
                .into_iter()
                .map(|attr| (attr.key, attr.value.into()))
                .collect(),
        }),
        PublicOplogEntry::FinishSpan(finish_span) => Ok(OplogEntry::FinishSpan {
            timestamp: finish_span.timestamp,
            span_id: finish_span.span_id,
        }),
        PublicOplogEntry::SetSpanAttribute(set_span_attribute) => {
            Ok(OplogEntry::SetSpanAttribute {
                timestamp: set_span_attribute.timestamp,
                span_id: set_span_attribute.span_id,
                key: set_span_attribute.key,
                value: set_span_attribute.value.into(),
            })
        }
        PublicOplogEntry::ChangePersistenceLevel(change_persistence_level) => {
            Ok(OplogEntry::ChangePersistenceLevel {
                timestamp: change_persistence_level.timestamp,
                level: change_persistence_level.persistence_level,
            })
        }
        PublicOplogEntry::CreateAgentInstance(create_agent_instance) => {
            Ok(OplogEntry::CreateAgentInstance {
                timestamp: create_agent_instance.timestamp,
                key: create_agent_instance.key,
                parameters: create_agent_instance.parameters,
            })
        }
        PublicOplogEntry::DropAgentInstance(drop_agent_instance) => {
            Ok(OplogEntry::DropAgentInstance {
                timestamp: drop_agent_instance.timestamp,
                key: drop_agent_instance.key,
            })
        }
    }
}

#[allow(clippy::type_complexity)]
fn convert_request_value_and_type_to_oplog_payload(
    function_name: &str,
    value_and_type: &ValueAndType,
) -> Result<OplogPayload, String> {
    match function_name {
        "golem::rpc::future-invoke-result::get" => {
            let payload: SerializableInvokeRequest =
                get_serializable_invoke_request(value_and_type)?;

            create_oplog_payload(&payload)
        }
        "http::types::future_incoming_response::get" => {
            let payload: SerializableHttpRequest =
                SerializableHttpRequest::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem io::poll::poll" => Ok(empty_payload()),
        "golem blobstore::container::object_info" => {
            let payload: ContainerAndObject =
                ContainerAndObject::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.container, payload.object))
        }
        "golem blobstore::container::delete_objects" => {
            let payload: ContainerAndObjects =
                ContainerAndObjects::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.container, payload.objects))
        }
        "golem blobstore::container::list_objects" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::container::get_data" => {
            let payload: ContainerObjectBeginEnd =
                ContainerObjectBeginEnd::from_value(&value_and_type.value)?;

            create_oplog_payload(&(
                payload.container,
                payload.object,
                payload.begin,
                payload.end,
            ))
        }
        "golem blobstore::container::write_data" => {
            let payload: ContainerObjectLength =
                ContainerObjectLength::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.container, payload.object, payload.length))
        }
        "golem blobstore::container::delete_object" => {
            let payload: ContainerAndObject =
                ContainerAndObject::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.container, payload.object))
        }
        "golem blobstore::container::has_object" => {
            let payload: ContainerAndObject =
                ContainerAndObject::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.container, payload.object))
        }
        "golem blobstore::container::clear" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::blobstore::copy_object" => {
            let payload: ContainerCopyObjectInfo =
                ContainerCopyObjectInfo::from_value(&value_and_type.value)?;

            create_oplog_payload(&(
                payload.src_container,
                payload.src_object,
                payload.dest_container,
                payload.dest_object,
            ))
        }
        "golem blobstore::blobstore::delete_container" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::blobstore::create_container" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::blobstore::get_container" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::blobstore::container_exists" => {
            let payload: Container = Container::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.0))
        }
        "golem blobstore::blobstore::move_object" => {
            let payload: ContainerCopyObjectInfo =
                ContainerCopyObjectInfo::from_value(&value_and_type.value)?;

            create_oplog_payload(&(
                payload.src_container,
                payload.src_object,
                payload.dest_container,
                payload.dest_object,
            ))
        }
        "golem_environment::get_arguments" => Ok(empty_payload()),
        "golem_environment::get_environment" => Ok(empty_payload()),
        "golem_environment::initial_cwd" => Ok(empty_payload()),
        "monotonic_clock::resolution" => Ok(empty_payload()),
        "monotonic_clock::now" => Ok(empty_payload()),
        "monotonic_clock::subscribe_duration" => Ok(empty_payload()),
        "wall_clock::now" => Ok(empty_payload()),
        "wall_clock::resolution" => Ok(empty_payload()),
        "golem_delete_promise" => {
            let payload = PromiseId::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_complete_promise" => {
            let payload = PromiseId::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem::api::update-worker" => {
            let payload: UpdateWorkerInfo = UpdateWorkerInfo::from_value(&value_and_type.value)?;

            create_oplog_payload(&(
                payload.worker_id,
                payload.component_version,
                payload.update_mode,
            ))
        }
        "http::types::incoming_body_stream::skip" => {
            let serializable_http_request =
                SerializableHttpRequest::from_value(&value_and_type.value)?;
            create_oplog_payload(&serializable_http_request)
        }
        "http::types::incoming_body_stream::read" => {
            let serializable_http_request =
                SerializableHttpRequest::from_value(&value_and_type.value)?;
            create_oplog_payload(&serializable_http_request)
        }
        "http::types::incoming_body_stream::blocking_read" => {
            let serializable_http_request =
                SerializableHttpRequest::from_value(&value_and_type.value)?;
            create_oplog_payload(&serializable_http_request)
        }
        "http::types::incoming_body_stream::blocking_skip" => {
            let serializable_http_request =
                SerializableHttpRequest::from_value(&value_and_type.value)?;
            create_oplog_payload(&serializable_http_request)
        }
        "golem keyvalue::eventual::delete" => {
            let payload: BucketAndKey = BucketAndKey::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.key))
        }
        "golem keyvalue::eventual::get" => {
            let payload: BucketAndKey = BucketAndKey::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.key))
        }
        "golem keyvalue::eventual::set" => {
            let payload: BucketKeyValue = BucketKeyValue::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.key, payload.value))
        }
        "golem keyvalue::eventual::exists" => {
            let payload: BucketAndKey = BucketAndKey::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.key))
        }
        "golem keyvalue::eventual_batch::set_many" => {
            let payload: BucketKeyValues = BucketKeyValues::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.key_values))
        }
        "golem keyvalue::eventual_batch::get_many" => {
            let payload: BucketAndKeys = BucketAndKeys::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.keys))
        }
        "golem keyvalue::eventual_batch::get_keys" => {
            let payload: Bucket = Bucket::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload.0)
        }
        "golem keyvalue::eventual_batch::delete_many" => {
            let payload: BucketAndKeys = BucketAndKeys::from_value(&value_and_type.value)?;

            create_oplog_payload(&(payload.bucket, payload.keys))
        }
        "golem random::insecure::get_insecure_random_bytes" => Ok(empty_payload()),
        "golem random::insecure::get_insecure_random_u64" => Ok(empty_payload()),
        "golem random::insecure_seed::insecure_seed" => Ok(empty_payload()),
        "golem random::get_random_bytes" => Ok(empty_payload()),
        "golem random::get_random_u64" => Ok(empty_payload()),
        "sockets::ip_name_lookup::resolve_addresses" => {
            let payload: String = String::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke" => {
            let payload: SerializableInvokeRequest =
                get_serializable_invoke_request(value_and_type)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke-and-await" => {
            let payload: SerializableInvokeRequest =
                get_serializable_invoke_request(value_and_type)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => Ok(empty_payload()),
        "cli::preopens::get_directories" => Ok(empty_payload()),
        "filesystem::types::descriptor::stat" => {
            let payload: String = String::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "filesystem::types::descriptor::stat_at" => {
            let payload: String = String::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem api::generate_idempotency_key" => Ok(empty_payload()),
        "golem http::types::future_trailers::get" => {
            let payload: SerializableHttpRequest =
                SerializableHttpRequest::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke idempotency key" => Ok(empty_payload()),
        "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => Ok(empty_payload()),
        "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => Ok(empty_payload()),
        _ => Err(format!("Unsupported host function name: {function_name}")),
    }
}

#[allow(clippy::type_complexity)]
fn convert_response_value_and_type_to_oplog_payload(
    function_name: &str,
    value_and_type: &ValueAndType,
) -> Result<OplogPayload, String> {
    match function_name {
        "golem::rpc::future-invoke-result::get" => {
            let payload: SerializableInvokeResult = get_serializable_invoke_result(value_and_type)?;

            create_oplog_payload(&payload)
        }
        "http::types::future_incoming_response::get" => {
            let payload: SerializableResponse =
                SerializableResponse::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem io::poll::poll" => {
            let payload: Result<u32, SerializableError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem blobstore::container::object_info" => {
            let payload: Result<ObjectMetadata, SerializableError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem blobstore::container::delete_objects" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::container::list_objects" => {
            let payload: Result<Vec<String>, SerializableError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem blobstore::container::get_data" => {
            let payload: Result<Vec<u8>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::container::write_data" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::container::delete_object" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::container::has_object" => {
            let payload: Result<bool, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::container::clear" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::copy_object" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::delete_container" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::create_container" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::get_container" => {
            let payload: Result<Option<u64>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::container_exists" => {
            let payload: Result<bool, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem blobstore::blobstore::move_object" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_environment::get_arguments" => {
            let payload: Result<Vec<String>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_environment::get_environment" => {
            let payload: Result<Vec<(String, String)>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_environment::initial_cwd" => {
            let payload: Result<Option<String>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "monotonic_clock::resolution" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "monotonic_clock::now" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "monotonic_clock::subscribe_duration" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "wall_clock::now" => {
            let payload: Result<SerializableDateTime, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "wall_clock::resolution" => {
            let payload: Result<SerializableDateTime, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_delete_promise" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem_complete_promise" => {
            let payload: Result<bool, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem::api::update-worker" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "http::types::incoming_body_stream::skip" => {
            let payload: Result<u64, SerializableStreamError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "http::types::incoming_body_stream::read" => {
            let payload: Result<Vec<u8>, SerializableStreamError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "http::types::incoming_body_stream::blocking_read" => {
            let payload: Result<Vec<u8>, SerializableStreamError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "http::types::incoming_body_stream::blocking_skip" => {
            let payload: Result<u64, SerializableStreamError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual::delete" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual::get" => {
            let payload: Result<Option<Vec<u8>>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual::set" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual::exists" => {
            let payload: Result<bool, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual_batch::set_many" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual_batch::get_many" => {
            let payload: Result<Vec<Option<Vec<u8>>>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual_batch::get_keys" => {
            let payload: Result<Vec<String>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem keyvalue::eventual_batch::delete_many" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem random::insecure::get_insecure_random_bytes" => {
            let payload: Result<Vec<u8>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem random::insecure::get_insecure_random_u64" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem random::insecure_seed::insecure_seed" => {
            let payload: Result<(u64, u64), SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem random::get_random_bytes" => {
            let payload: Result<Vec<u8>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem random::get_random_u64" => {
            let payload: Result<u64, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "sockets::ip_name_lookup::resolve_addresses" => {
            let payload: Result<SerializableIpAddresses, SerializableError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke" => {
            let payload: Result<(), SerializableError> = Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke-and-await" => {
            let payload = get_invoke_and_await_result(value_and_type);
            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => {
            let payload: Result<WorkerId, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "cli::preopens::get_directories" => {
            let payload: Result<Vec<String>, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "filesystem::types::descriptor::stat" => {
            let payload: Result<SerializableFileTimes, SerializableError> =
                Result::from_value(&value_and_type.value)?;
            create_oplog_payload(&payload)
        }
        "filesystem::types::descriptor::stat_at" => {
            let payload: Result<SerializableFileTimes, SerializableError> =
                Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem api::generate_idempotency_key" => {
            // ValueAndType corresponds to a UUID, and UUID serialized as a tuple of high and low.
            let uuid: Result<Uuid, SerializableError> = Result::from_value(&value_and_type.value)?;
            let payload = uuid.map(|x| {
                let (h, l) = x.as_u64_pair();
                (h, l)
            });

            create_oplog_payload(&payload)
        }
        "golem http::types::future_trailers::get" => {
            let payload: Result<
                Option<Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableErrorCode>, ()>>,
                SerializableError,
            > = Result::from_value(&value_and_type.value)?;

            create_oplog_payload(&payload)
        }
        "golem::rpc::wasm-rpc::invoke idempotency key" => create_uuid_payload(value_and_type),
        "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => {
            create_uuid_payload(value_and_type)
        }
        "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => {
            create_uuid_payload(value_and_type)
        }
        _ => Err(format!("Unsupported host function name: {function_name}")),
    }
}

fn get_invoke_and_await_result(
    value_and_type: &ValueAndType,
) -> Result<Result<ValueAndType, SerializableError>, String> {
    match &value_and_type.value {
        Value::Result(Ok(Some(value))) => match &value_and_type.typ {
            AnalysedType::Result(type_result) => {
                let typ = type_result
                    .clone()
                    .ok
                    .ok_or("Failed to get type of ok")?
                    .deref()
                    .clone();
                let value = value.deref().clone();
                let value_and_type = ValueAndType::new(value, typ);
                Ok(Ok(value_and_type))
            }

            _ => Err("Failed to obtain type annotated value".to_string()),
        },

        Value::Result(Err(Some(err))) => {
            let serializable_error = SerializableError::from_value(err)?;
            Ok(Err(serializable_error))
        }

        Value::Option(None) => {
            Err("Failed to get invoke-and-await result back from Value".to_string())
        }

        _ => Err("Failed to get invoke-and-await result back from Value".to_string()),
    }
}

fn get_serializable_invoke_request(
    value_and_type: &ValueAndType,
) -> Result<SerializableInvokeRequest, String> {
    match &value_and_type.value {
        Value::Record(values) => {
            if values.len() != 4 {
                return Err("Failed to get SerializableInvokeRequest".to_string());
            }
            let remote_worker_id = &values[0];
            let idempotency_key = &values[1];
            let function_name = &values[2];
            let function_params = &values[3];

            let remote_worker_id = WorkerId::from_value(remote_worker_id)?;
            let idempotency_key = IdempotencyKey::from_uuid(Uuid::from_value(idempotency_key)?);
            let function_name = String::from_value(function_name)?;

            let function_param_cases = match &value_and_type.typ {
                AnalysedType::Record(type_record) => {
                    let function_param_type = type_record
                        .fields
                        .iter()
                        .find(|x| x.name == "function-params")
                        .ok_or("Failed to find function-params type".to_string())?;

                    match function_param_type.typ.clone() {
                        AnalysedType::List(inner) => match *inner.inner {
                            AnalysedType::Record(inner) => {
                                let nodes_types =
                                    inner.fields.iter().find(|x| x.name == "nodes").ok_or(
                                        "Failed to find nodes field inside function-params",
                                    )?;
                                match &nodes_types.typ {
                                    AnalysedType::List(list_type) => match &*list_type.inner {
                                        AnalysedType::Variant(cases) => cases.cases.clone(),
                                        _ => Err("nodes element type was not of type variant"
                                            .to_string())?,
                                    },
                                    _ => Err("nodes field was not of type list".to_string())?,
                                }
                            }
                            _ => Err("function-params type was not a list".to_string())?,
                        },
                        _ => Err("Failed to get function param list".to_string())?,
                    }
                }
                _ => Err("Internal Error. Failed to get SerializableInvokeRequest".to_string())?,
            };

            let mut parsed_function_params = Vec::new();
            match function_params {
                Value::List(params) => {
                    for param in params {
                        match param {
                            Value::Record(fields) => {
                                match fields.as_slice() {
                                    [Value::List(list_values)] => match list_values.as_slice() {
                                        [Value::Variant {
                                            case_idx,
                                            case_value: Some(case_value),
                                        }] => {
                                            let value_and_type = ValueAndType::new(
                                                *case_value.clone(),
                                                function_param_cases[*case_idx as usize]
                                                    .typ
                                                    .clone()
                                                    .expect("Variant case should have typ"),
                                            );
                                            parsed_function_params.push(value_and_type);
                                        }
                                        _ => Err(
                                            "Function param field did not contain a single variant"
                                                .to_string(),
                                        )?,
                                    },
                                    _ => Err("Function param did not have a single list field"
                                        .to_string())?,
                                }
                            }
                            _ => Err("Function was not a record".to_string())?,
                        }
                    }
                }
                _ => Err("Function params were not a list".to_string())?,
            }

            Ok(SerializableInvokeRequest {
                remote_worker_id,
                idempotency_key,
                function_name,
                function_params: parsed_function_params,
            })
        }
        _ => Err("Failed to get SerializableInvokeRequest".to_string()),
    }
}

fn get_serializable_invoke_result(
    value_and_type: &ValueAndType,
) -> Result<SerializableInvokeResult, String> {
    match &value_and_type.value {
        Value::Variant {
            case_idx,
            case_value,
        } => match (case_idx, case_value) {
            (0, Some(payload)) => {
                let error = SerializableError::from_value(payload)?;
                Ok(SerializableInvokeResult::Failed(error))
            }

            (1, None) => Ok(SerializableInvokeResult::Pending),
            (2, Some(payload)) => match payload.deref() {
                Value::Result(Ok(Some(value))) => {
                    let value_of_type_annotated_value = value.deref();
                    match &value_and_type.typ {
                        AnalysedType::Variant(typed_variant) => {
                            let typ = typed_variant
                                .cases
                                .iter()
                                .find(|x| x.name == "Completed")
                                .and_then(|x| x.typ.clone())
                                .ok_or("Failed to get SerializableInvokeResult")?;

                            let value_and_type =
                                ValueAndType::new(value_of_type_annotated_value.clone(), typ);

                            Ok(SerializableInvokeResult::Completed(Ok(Some(
                                value_and_type,
                            ))))
                        }

                        _ => Err("Failed to get SerializableInvokeResult from Value".to_string()),
                    }
                }
                Value::Result(Err(Some(value))) => {
                    let rpc_error = RpcError::from_value(value)?;

                    Ok(SerializableInvokeResult::Completed(Err(rpc_error)))
                }
                _ => Err("Failed to get SerializableInvokeResult from Value".to_string()),
            },

            _ => Err("Failed to get SerializableInvokeResult from Value".to_string()),
        },
        _ => Err("Failed to get SerializableInvokeResult from Value".to_string()),
    }
}

fn create_uuid_payload(value_and_type: &ValueAndType) -> Result<OplogPayload, String> {
    let uuid: Result<Uuid, SerializableError> = Result::from_value(&value_and_type.value)?;

    let payload = uuid.map(|x| {
        let (h, l) = x.as_u64_pair();
        (h, l)
    });

    create_oplog_payload(&payload)
}

fn create_oplog_payload<T: Encode>(payload: &T) -> Result<OplogPayload, String> {
    let serialized = golem_common::serialization::serialize(payload).map_err(|e| e.to_string())?;
    Ok(OplogPayload::Inline(serialized.to_vec()))
}

fn empty_payload() -> OplogPayload {
    OplogPayload::Inline(vec![])
}

#[cfg(test)]
mod tests {
    use crate::debug_session::{get_serializable_invoke_request, get_serializable_invoke_result};
    use golem_common::model::{ComponentId, IdempotencyKey, WorkerId};
    use golem_wasm_ast::analysis::analysed_type::{case, str, variant};
    use golem_wasm_ast::analysis::NameOptionTypePair;
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
    use golem_worker_executor::durable_host::wasm_rpc::serialized::{
        SerializableInvokeRequest, SerializableInvokeResult,
    };
    use golem_worker_executor::services::rpc::RpcError;
    use test_r::test;
    use uuid::Uuid;

    #[test]
    fn test_get_serializable_invoke_result_1() {
        let error_value = Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::String("generic_error".to_string()))),
        };

        let value_and_type = ValueAndType {
            value: Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(error_value)),
            },
            typ: variant(vec![NameOptionTypePair {
                name: "Failed".to_string(),
                typ: None,
            }]),
        };

        let result = get_serializable_invoke_result(&value_and_type);
        assert!(matches!(result, Ok(SerializableInvokeResult::Failed(_))));
    }

    #[test]
    fn test_get_serializable_invoke_result_2() {
        let value_and_type = ValueAndType {
            value: Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            typ: variant(vec![]),
        };

        let result = get_serializable_invoke_result(&value_and_type);
        assert!(matches!(result, Ok(SerializableInvokeResult::Pending)));
    }

    #[test]
    fn test_get_serializable_invoke_result_3() {
        let payload = Value::Result(Ok(Some(Box::new(Value::String("foo".to_string())))));

        let value_and_type = ValueAndType {
            value: Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(payload)),
            },
            typ: variant(vec![case("Completed", str())]),
        };

        let result = get_serializable_invoke_result(&value_and_type);
        assert_eq!(
            result,
            Ok(SerializableInvokeResult::Completed(Ok(Some(
                "foo".into_value_and_type()
            ))))
        );
    }

    #[test]
    fn test_get_serializable_invoke_result_4() {
        let error_value = Value::Variant {
            case_idx: 0,
            case_value: Some(Box::new(Value::String("generic_error".to_string()))),
        };

        let payload = Value::Result(Err(Some(Box::new(error_value))));

        let value_and_type = ValueAndType {
            value: Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(payload)),
            },
            typ: variant(vec![case("Completed", str())]),
        };

        let result = get_serializable_invoke_result(&value_and_type);

        assert_eq!(
            result,
            Ok(SerializableInvokeResult::Completed(Err(
                RpcError::ProtocolError {
                    details: "generic_error".to_string()
                }
            )))
        );
    }

    #[test]
    fn test_get_serializable_invoke_request() {
        let remote_worker_id = WorkerId {
            component_id: ComponentId::new_v4(),
            worker_name: "foo".to_string(),
        };

        let idempotency_key = IdempotencyKey::from_uuid(Uuid::new_v4());
        let function_params = vec![
            ValueAndType::new(Value::String("foo".to_string()), str()),
            ValueAndType::new(Value::String("bar".to_string()), str()),
        ];

        let serializable_invoke_request = SerializableInvokeRequest {
            remote_worker_id,
            idempotency_key,
            function_name: "foo".to_string(),
            function_params,
        };

        let value_and_type = serializable_invoke_request.clone().into_value_and_type();
        let result = get_serializable_invoke_request(&value_and_type).unwrap();
        assert_eq!(result, serializable_invoke_request);
    }
}
