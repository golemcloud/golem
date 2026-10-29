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

use crate::durable_host::durability::ClassifiedHostError;
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::durable_session::StreamSession;
use crate::durable_host::durable_stream::SessionError;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::Rpc;
use crate::services::rpc::RpcDemand;
use golem_common::model::AgentId;
use golem_common::model::IdempotencyKey;
use golem_common::model::OwnedAgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::InvocationFreshnessDisposition;
use golem_common::model::agent::ParsedAgentId;
use golem_common::model::card::ScopeCard;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::SpanId;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::host_functions::GolemRpcWasmRpcNew;
use golem_common::schema::schema_value::SchemaValue;
use golem_schema::schema::wit::PermissionCardHandleDropper;
use golem_schema::schema::wit::PermissionCardHandleRep;
use golem_schema::schema::wit::QuotaTokenHandleDropper;
use golem_schema::schema::wit::SecretHandleDropper;
use golem_schema::schema::wit::wire as core_wire;
use std::sync::Arc;
use std::time::Duration;
use wasmtime::component::Resource;

use super::schedule::validate_cancellation_token_owner;
use super::*;
use crate::services::rpc::RpcError as ServiceRpcError;
use async_trait::async_trait;
use golem_common::data_value;
use golem_common::model::card::CardId;
use golem_common::model::component::ComponentId;
use golem_schema::schema::wit::{QuotaTokenHandleRep, SecretHandleRep};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::test;
use uuid::Uuid;
use wasmtime::component::ResourceTable;

/// An accepted streaming RPC that leaves a caller-owned input unbound cancels that input. When
/// the commit of the cancellation meets an oplog whose storage did not answer, the caller
/// keeps the unreadable oplog and traps for recovery instead of receiving an RPC failure.
#[test]
#[test_r::timeout("30s")]
async fn an_unreadable_oplog_while_cancelling_unbound_inputs_traps_for_recovery() {
    use crate::durable_host::durable_stream::tests::{TestOplog, identity, registration};
    use crate::durable_host::durable_stream::{DurableStreamCommit, DurableStreamStore};
    use crate::services::oplog::{
        Oplog, OplogReadError, OplogReadSource, OplogRefusal, UnreadableOplog,
    };
    use golem_common::base_model::durable_stream::{
        SessionStreamRole, StreamBindingRecord, StreamRegistrationCoordinate,
        StreamRegistrationInvocation, StreamRootKind, StreamSessionMappingRecord, StreamSourceKind,
        StreamValuePathStep,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;

    let identity = identity();
    let oplog = Arc::new(TestOplog::default());
    let refusal = Arc::new(Mutex::new(None::<UnreadableOplog>));
    let commit: DurableStreamCommit = Arc::new({
        let oplog = oplog.clone();
        let refusal = refusal.clone();
        move |committed| {
            let oplog = oplog.clone();
            let refusal = refusal.lock().unwrap().clone();
            Box::pin(async move {
                let result = match refusal {
                    Some(unreadable) => Err(OplogRefusal::Unreadable(unreadable)),
                    None => {
                        oplog
                            .commit(CommitLevel::Always)
                            .await
                            .expect("oplog write");
                        Ok(())
                    }
                };
                if let Some(committed) = committed {
                    let _ = committed.send(result);
                }
            })
        }
    });
    let producer = DurableStreamStore::load_with_commit(
        oplog.clone(),
        identity.environment_id,
        identity.agent_id.clone(),
        identity.fingerprint,
        None,
        commit,
    )
    .await
    .unwrap();
    let handle = producer
        .register(
            None,
            registration(
                &identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![StreamValuePathStep::TupleElement(0)],
                },
                StreamSourceKind::AgentHostedInput,
            ),
        )
        .await
        .unwrap()
        .value;
    let mapping = StreamSessionMappingRecord {
        transport_stream_id: 0,
        handle,
        role: SessionStreamRole::Input,
    };
    let binding: StreamBindingRecord = producer
        .local_binding(mapping.transport_stream_id, &mapping.handle, mapping.role)
        .await
        .unwrap();
    let streams = StreamSession::open(
        producer.clone(),
        oplog.clone(),
        StreamRegistrationInvocation::Local(identity.invocation.idempotency_key.clone()),
        vec![binding],
    )
    .await
    .unwrap();
    let unreadable = UnreadableOplog::new(
        &identity.agent_id,
        OplogReadError::source_unavailable(OplogReadSource::Archive(1), "outage"),
    );
    *refusal.lock().unwrap() = Some(unreadable.clone());
    let (accepted, acceptance) = tokio::sync::oneshot::channel();
    accepted
        .send(golem_api_grpc::proto::golem::worker::InvocationAccepted::default())
        .unwrap();

    let result = await_streaming_rpc_acceptance(
        &streams,
        None,
        acceptance,
        std::future::pending::<
            Result<crate::services::rpc::DurableRpcInvocationResult, ServiceRpcError>,
        >(),
    )
    .await;

    let error = match result {
        Err(SessionError::Unreadable(error)) => error,
        Err(other) => panic!("expected the unreadable oplog of the caller, got {other}"),
        Ok(_) => panic!("expected the unreadable oplog of the caller, got an RPC result"),
    };
    assert_eq!(error, unreadable);
    let trap = SessionError::Unreadable(error).into_trap();
    assert!(
        matches!(
            trap.downcast_ref::<WorkerExecutorError>(),
            Some(WorkerExecutorError::RecoveryRequired { .. })
        ),
        "a transient unreadable oplog must trap for recovery, got {trap:?}"
    );
}

#[test]
async fn dropping_rpc_task_revokes_wait_before_task_cleanup() {
    use crate::worker::suspension::OwnerSuspension;
    use crate::worker::suspension::tests::{blocked_timer, eligible_now};
    use std::future::{Future, pending};
    use std::task::Context;
    use std::time::Instant;

    let owner = OwnerSuspension::new();
    let (store, _, _timer) = blocked_timer(&owner, Instant::now() + Duration::from_secs(30));
    let activity = store.rpc_activity(Duration::ZERO, Duration::from_secs(5));
    let revoker = activity.revoker();
    let remote = revoker.clone();
    // Retain the coordinated future independently to delay background task cleanup.
    let mut delayed_cleanup = Box::pin(
        activity
            .coordinate(async move { remote.remote_wait(pending::<anyhow::Result<()>>()).await }),
    );
    let mut cx = Context::from_waker(futures::task::noop_waker_ref());
    assert!(delayed_cleanup.as_mut().poll(&mut cx).is_pending());
    assert!(eligible_now(&owner));
    let task = RpcTask {
        task: wasmtime_wasi::runtime::spawn(pending()),
        revoker: Some(revoker),
    };

    drop(task);
    assert!(!eligible_now(&owner));
    assert!(delayed_cleanup.as_mut().poll(&mut cx).is_pending());
    assert!(!eligible_now(&owner));
    drop(delayed_cleanup);
    assert!(eligible_now(&owner));
}

#[test]
fn logical_ephemeral_rpc_connection_creation_is_local_durability() {
    use crate::durable_host::durability::{DurableExecutionState, InFunctionRetryController};

    for (activates_target, function_type) in [
        (false, DurableFunctionType::WriteLocal),
        (true, DurableFunctionType::WriteRemote),
    ] {
        assert_eq!(
            rpc_connection_function_type(activates_target),
            function_type
        );
        let retry = InFunctionRetryController::new(
            rpc_connection_function_type(activates_target),
            DurableExecutionState {
                is_live: false,
                snapshotting_mode: false,
                max_in_function_retry_delay: Duration::ZERO,
            },
            GolemRpcWasmRpcNew::FQFN,
        );
        assert!(retry.can_reexecute_on_incomplete_replay());
    }
}

#[test]
fn embedded_rpc_span_uses_parent_within_trace_and_link_across_traces() {
    let trace_a = golem_common::model::invocation_context::TraceId::generate();
    let trace_b = golem_common::model::invocation_context::TraceId::generate();
    let current = SpanId::generate();
    let connection = SpanId::generate();
    let key = IdempotencyKey::new("rpc-key".to_string());

    let same_trace = embedded_invocation_span_started(
        &trace_a,
        &["a=1".to_string()],
        &current,
        &connection,
        (trace_a.clone(), vec!["a=1".to_string()]),
        "same",
        &key,
    );
    assert_eq!(same_trace.parent_span_id, Some(connection.clone()));
    assert!(same_trace.links.is_empty());

    let cross_trace = embedded_invocation_span_started(
        &trace_b,
        &["b=2".to_string()],
        &current,
        &connection,
        (trace_a.clone(), vec!["a=1".to_string()]),
        "cross",
        &key,
    );
    assert_eq!(cross_trace.parent_span_id, Some(current));
    assert_eq!(cross_trace.links.len(), 1);
    assert_eq!(cross_trace.links[0].trace_id, trace_a);
    assert_eq!(cross_trace.links[0].span_id, connection);
    assert_eq!(cross_trace.links[0].trace_states, vec!["a=1".to_string()]);
}

struct FixedDemand {
    fingerprint: AgentFingerprint,
}

impl RpcDemand for FixedDemand {
    fn fingerprint(&self) -> AgentFingerprint {
        self.fingerprint
    }
}

struct FingerprintMismatchRpc {
    live_fingerprint: AgentFingerprint,
    invoke_called: AtomicBool,
}

struct ActivationFailureRpc {
    invoke_called: AtomicBool,
}

struct RecordingEnvRpc {
    fingerprint: AgentFingerprint,
    activation_env: Mutex<Option<Vec<(String, String)>>>,
    dispatched_scope_card: Mutex<Option<ScopeCard>>,
}

struct TableCapabilityDropper {
    table: ResourceTable,
}

impl QuotaTokenHandleDropper for TableCapabilityDropper {
    fn drop_quota_token_handle(&mut self, handle: Resource<QuotaTokenHandleRep>) {
        let _ = self.table.delete(handle);
    }
}

impl SecretHandleDropper for TableCapabilityDropper {
    fn drop_secret_handle(&mut self, handle: Resource<SecretHandleRep>) {
        let _ = self.table.delete(handle);
    }
}

impl PermissionCardHandleDropper for TableCapabilityDropper {
    fn drop_permission_card_handle(&mut self, handle: Resource<PermissionCardHandleRep>) {
        let _ = self.table.delete(handle);
    }
}

#[async_trait]
impl Rpc for FingerprintMismatchRpc {
    async fn create_demand(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _method_name: &str,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, ServiceRpcError> {
        Ok(Box::new(FixedDemand {
            fingerprint: self.live_fingerprint,
        }))
    }

    async fn invoke_and_await(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, ServiceRpcError> {
        self.invoke_called.store(true, Ordering::SeqCst);
        Ok(SchemaValue::Tuple { elements: vec![] })
    }

    async fn invoke(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), ServiceRpcError> {
        unreachable!("test only exercises invoke-and-await dispatch")
    }
}

#[async_trait]
impl Rpc for ActivationFailureRpc {
    async fn create_demand(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _method_name: &str,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, ServiceRpcError> {
        Err(ServiceRpcError::Denied {
            details: "activation denied".to_string(),
        })
    }

    async fn invoke_and_await(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, ServiceRpcError> {
        self.invoke_called.store(true, Ordering::SeqCst);
        Ok(SchemaValue::Tuple { elements: vec![] })
    }

    async fn invoke(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), ServiceRpcError> {
        unreachable!("test only exercises invoke-and-await dispatch")
    }
}

#[async_trait]
impl Rpc for RecordingEnvRpc {
    async fn create_demand(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _method_name: &str,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
    ) -> Result<Box<dyn RpcDemand>, ServiceRpcError> {
        *self.activation_env.lock().unwrap() = Some(self_env.to_vec());
        Ok(Box::new(FixedDemand {
            fingerprint: self.fingerprint,
        }))
    }

    async fn invoke_and_await(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
        scope_card: Option<ScopeCard>,
    ) -> Result<SchemaValue, ServiceRpcError> {
        *self.dispatched_scope_card.lock().unwrap() = scope_card;
        Ok(SchemaValue::Tuple { elements: vec![] })
    }

    async fn invoke(
        &self,
        _owned_agent_id: &OwnedAgentId,
        _idempotency_key: Option<IdempotencyKey>,
        _freshness_disposition: golem_common::model::agent::InvocationFreshnessDisposition,
        _method_name: String,
        _method_parameters: SchemaValue,
        _self_created_by: AccountId,
        _self_agent_id: &AgentId,
        _self_env: &[(String, String)],
        _self_stack: InvocationContextStack,
        _config: Vec<AgentConfigEntryDto>,
        _auth_ctx: &AuthCtx,
    ) -> Result<(), ServiceRpcError> {
        unreachable!("test only exercises invoke-and-await dispatch")
    }
}

fn agent_id(name: &str) -> AgentId {
    AgentId {
        component_id: ComponentId(Uuid::from_u128(1)),
        agent_id: name.to_string(),
    }
}

fn cancellation_token(owner: OwnedAgentId, recipient: OwnedAgentId) -> CancellationTokenEntry {
    CancellationTokenEntry {
        schedule_id: vec![],
        metadata: invocation_metadata(
            &recipient,
            &IdempotencyKey::new("scheduled-invocation".to_string()),
        ),
        owner,
        recipient,
    }
}

#[test]
fn scheduled_invocation_owner_can_cancel_its_token() {
    let owner = OwnedAgentId::new(EnvironmentId::new(), &agent_id("owner"));
    let recipient = OwnedAgentId::new(EnvironmentId::new(), &agent_id("recipient"));
    let token = cancellation_token(owner.clone(), recipient);

    assert_eq!(validate_cancellation_token_owner(&token, &owner), Ok(()));
}

#[test]
fn scheduled_invocation_non_owner_cannot_cancel_token() {
    let owner = OwnedAgentId::new(EnvironmentId::new(), &agent_id("owner"));
    let recipient = OwnedAgentId::new(EnvironmentId::new(), &agent_id("recipient"));
    let non_owner = OwnedAgentId::new(EnvironmentId::new(), &agent_id("non-owner"));
    let token = cancellation_token(owner, recipient);

    assert!(matches!(
        validate_cancellation_token_owner(&token, &non_owner),
        Err(WorkerExecutorError::PermissionDenied { .. })
    ));
}

#[test]
async fn deferred_activation_fingerprint_mismatch_is_a_host_failure_not_rpc_result() {
    let persisted_fingerprint = AgentFingerprint(Uuid::from_u128(10));
    let live_fingerprint = AgentFingerprint(Uuid::from_u128(11));
    let rpc = Arc::new(FingerprintMismatchRpc {
        live_fingerprint,
        invoke_called: AtomicBool::new(false),
    });

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("deferred-activation-mismatch".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        vec![],
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        Some(RpcTargetActivation {
            target_fingerprint: Some(persisted_fingerprint),
            env: vec![],
            config: vec![],
        }),
        InvocationFreshnessDisposition::MayExist,
        None,
    )
    .await;

    assert!(
        !rpc.invoke_called.load(Ordering::SeqCst),
        "fingerprint mismatch must stop before dispatching the RPC method"
    );
    assert!(
        result.is_err(),
        "fingerprint mismatch is a replay/activation violation and must be an outer host failure, not a completed RPC result: {result:?}"
    );
}

#[test]
async fn first_authorized_call_activates_a_new_durable_target_without_a_prior_fingerprint() {
    let rpc = Arc::new(FingerprintMismatchRpc {
        live_fingerprint: AgentFingerprint(Uuid::from_u128(11)),
        invoke_called: AtomicBool::new(false),
    });

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("first-authorized-activation".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        vec![],
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        Some(RpcTargetActivation {
            target_fingerprint: None,
            env: vec![],
            config: vec![],
        }),
        InvocationFreshnessDisposition::MayExist,
        None,
    )
    .await;

    result.unwrap().unwrap();
    assert!(rpc.invoke_called.load(Ordering::SeqCst));
}

#[test]
async fn deferred_activation_create_demand_failure_is_a_host_failure_not_rpc_result() {
    let rpc = Arc::new(ActivationFailureRpc {
        invoke_called: AtomicBool::new(false),
    });

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("deferred-activation-failure".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        vec![],
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        Some(RpcTargetActivation {
            target_fingerprint: Some(AgentFingerprint(Uuid::from_u128(10))),
            env: vec![],
            config: vec![],
        }),
        InvocationFreshnessDisposition::MayExist,
        None,
    )
    .await;

    assert!(
        !rpc.invoke_called.load(Ordering::SeqCst),
        "activation failure must stop before dispatching the RPC method"
    );
    assert!(
        result.is_err(),
        "activation failure happened before the RPC method call and must be an outer host failure, not a completed RPC result: {result:?}"
    );
}

#[test]
async fn deferred_activation_fingerprint_mismatch_is_permanent_host_failure() {
    let persisted_fingerprint = AgentFingerprint(Uuid::from_u128(10));
    let live_fingerprint = AgentFingerprint(Uuid::from_u128(11));
    let rpc = Arc::new(FingerprintMismatchRpc {
        live_fingerprint,
        invoke_called: AtomicBool::new(false),
    });

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("deferred-activation-mismatch-classification".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        vec![],
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        Some(RpcTargetActivation {
            target_fingerprint: Some(persisted_fingerprint),
            env: vec![],
            config: vec![],
        }),
        InvocationFreshnessDisposition::MayExist,
        None,
    )
    .await;

    assert!(
        !rpc.invoke_called.load(Ordering::SeqCst),
        "fingerprint mismatch must stop before dispatching the RPC method"
    );
    let err = result.expect_err("fingerprint mismatch must be an outer host failure");
    let classified = err.downcast_ref::<ClassifiedHostError>().expect(
        "fingerprint mismatch must be classified so future get does not retry it as transient",
    );
    assert_eq!(classified.kind, HostFailureKind::Permanent);
}

#[test]
async fn deferred_replay_activation_uses_new_env_not_async_invocation_env() {
    let persisted_fingerprint = AgentFingerprint(Uuid::from_u128(10));
    let env_from_wasm_rpc_new = vec![("SOURCE".to_string(), "wasm-rpc-new".to_string())];
    let env_from_async_invocation = vec![("SOURCE".to_string(), "async-invoke".to_string())];
    let rpc = Arc::new(RecordingEnvRpc {
        fingerprint: persisted_fingerprint,
        activation_env: Mutex::new(None),
        dispatched_scope_card: Mutex::new(None),
    });
    let replay_pending = WasmRpcTargetActivation::ReplayPending {
        target_fingerprint: persisted_fingerprint,
        env: env_from_wasm_rpc_new.clone(),
        config: vec![],
    };

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("deferred-activation-env".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        env_from_async_invocation,
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        replay_pending.deferred_activation(),
        InvocationFreshnessDisposition::MayExist,
        None,
    )
    .await;

    result
        .expect("activation should succeed")
        .expect("RPC invocation should succeed");
    assert_eq!(
        *rpc.activation_env.lock().unwrap(),
        Some(env_from_wasm_rpc_new),
        "deferred replay activation must use the environment captured by wasm-rpc::new, not the later async invocation environment"
    );
}

#[test]
async fn await_dispatch_forwards_the_resolved_scope_card_to_the_rpc_layer() {
    let scope_card = ScopeCard {
        scope_card_id: CardId(Uuid::from_u128(20)),
        root_card_ids: vec![CardId(Uuid::from_u128(10))],
        lower_positive: vec![],
        lower_negative: vec![],
        upper_positive: vec![],
        upper_negative: vec![],
        expires_at: None,
    };
    let rpc = Arc::new(RecordingEnvRpc {
        fingerprint: AgentFingerprint(Uuid::from_u128(30)),
        activation_env: Mutex::new(None),
        dispatched_scope_card: Mutex::new(None),
    });

    let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
        None,
        rpc.clone(),
        OwnedAgentId::new(EnvironmentId::new(), &agent_id("target")),
        IdempotencyKey::new("scope-card-dispatch".to_string()),
        "run".to_string(),
        SchemaValue::Tuple { elements: vec![] },
        AccountId::new(),
        agent_id("caller"),
        vec![],
        vec![],
        InvocationContextStack::fresh(),
        None,
        AuthCtx::system(),
        None,
        InvocationFreshnessDisposition::MayExist,
        Some(scope_card.clone()),
    )
    .await;

    result
        .expect("dispatch should not fail at the host layer")
        .expect("RPC invocation should succeed");
    assert_eq!(*rpc.dispatched_scope_card.lock().unwrap(), Some(scope_card));
}

#[test]
fn non_await_variants_reject_any_scope_card_argument() {
    let scope_card = Some(Resource::<PermissionCardHandleRep>::new_borrow(1));

    for operation in [
        "invoke",
        "schedule-invocation",
        "schedule-cancelable-invocation",
    ] {
        let mut dropper = TableCapabilityDropper {
            table: ResourceTable::new(),
        };
        let input = || core_wire::SchemaValueTree {
            value_nodes: vec![core_wire::SchemaValueNode::BoolValue(false)],
            root: 0,
        };
        assert!(
            reject_non_await_scope_card(&scope_card, input(), operation, &mut dropper).is_err()
        );
        assert!(reject_non_await_scope_card(&None, input(), operation, &mut dropper).is_ok());
    }
}

#[test]
fn rejected_scope_card_call_consumes_owned_capabilities_from_rpc_input() {
    let mut dropper = TableCapabilityDropper {
        table: ResourceTable::new(),
    };
    let input_quota = dropper
        .table
        .push(QuotaTokenHandleRep::new(()))
        .expect("input quota token should be inserted");
    let input_secret = dropper
        .table
        .push(SecretHandleRep::new(()))
        .expect("input secret should be inserted");
    let input_card = dropper
        .table
        .push(PermissionCardHandleRep::new(()))
        .expect("input card should be inserted");
    let input_quota_rep = input_quota.rep();
    let input_secret_rep = input_secret.rep();
    let input_card_rep = input_card.rep();
    let input = core_wire::SchemaValueTree {
        value_nodes: vec![
            core_wire::SchemaValueNode::QuotaTokenHandle(input_quota),
            core_wire::SchemaValueNode::SecretValue(input_secret),
            core_wire::SchemaValueNode::PermissionCardHandle(input_card),
            core_wire::SchemaValueNode::TupleValue(vec![0, 1, 2]),
        ],
        root: 3,
    };
    let scope_card = Some(Resource::<PermissionCardHandleRep>::new_borrow(1));

    assert!(reject_non_await_scope_card(&scope_card, input, "invoke", &mut dropper).is_err());
    assert!(
        dropper
            .table
            .get(&Resource::<QuotaTokenHandleRep>::new_borrow(
                input_quota_rep
            ))
            .is_err()
    );
    assert!(
        dropper
            .table
            .get(&Resource::<SecretHandleRep>::new_borrow(input_secret_rep))
            .is_err()
    );
    assert!(
        dropper
            .table
            .get(&Resource::<PermissionCardHandleRep>::new_borrow(
                input_card_rep
            ))
            .is_err()
    );
}

#[test]
fn rejected_rpc_creation_drains_every_config_handle() {
    let mut dropper = TableCapabilityDropper {
        table: ResourceTable::new(),
    };
    let first = dropper
        .table
        .push(QuotaTokenHandleRep::new(()))
        .expect("first config quota token should be inserted");
    let second = dropper
        .table
        .push(QuotaTokenHandleRep::new(()))
        .expect("second config quota token should be inserted");
    let first_rep = first.rep();
    let second_rep = second.rep();
    let config = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, handle)| {
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue {
                path: vec![format!("config-{index}")],
                value: core_wire::TypedSchemaValue {
                    graph: core_wire::SchemaGraph {
                        type_nodes: vec![],
                        defs: vec![],
                        root: 0,
                    },
                    value: core_wire::SchemaValueTree {
                        value_nodes: vec![core_wire::SchemaValueNode::QuotaTokenHandle(handle)],
                        root: 0,
                    },
                },
            }
        })
        .collect();

    discard_owned_rpc_config(config, &mut dropper);

    assert!(
        dropper
            .table
            .get(&Resource::<QuotaTokenHandleRep>::new_borrow(first_rep))
            .is_err()
    );
    assert!(
        dropper
            .table
            .get(&Resource::<QuotaTokenHandleRep>::new_borrow(second_rep))
            .is_err()
    );
}

#[test]
fn ephemeral_invocation_target_is_derived_from_the_host_call_key() {
    let environment_id = EnvironmentId::new();
    let component_id = ComponentId(Uuid::from_u128(1));
    let logical_agent_id = ParsedAgentId::try_new(
        golem_common::model::agent::AgentTypeName("ephemeral-target".to_string()),
        data_value!("constructor"),
        None,
    )
    .unwrap();
    let logical_agent = AgentId::from_agent_id(component_id, &logical_agent_id).unwrap();
    let logical_owned_agent = OwnedAgentId::new(environment_id, &logical_agent);
    let first_key = IdempotencyKey::new("first-call".to_string());

    let first =
        invocation_target_agent_id(&logical_owned_agent, Some(&logical_agent_id), &first_key)
            .unwrap();
    let first_retry = invocation_target_agent_id(
        &logical_owned_agent,
        Some(&logical_agent_id),
        &IdempotencyKey::new("first-call".to_string()),
    )
    .unwrap();
    let second = invocation_target_agent_id(
        &logical_owned_agent,
        Some(&logical_agent_id),
        &IdempotencyKey::new("second-call".to_string()),
    )
    .unwrap();

    assert_eq!(first, first_retry);
    assert_ne!(first, second);
    assert_eq!(first.environment_id, environment_id);
    assert_eq!(first.component_id(), component_id);
    let metadata = invocation_metadata(&first, &first_key);
    assert_eq!(metadata.agent_id, first.agent_id.agent_id);
    assert_eq!(metadata.idempotency_key, first_key.value);
}

#[test]
fn only_the_first_sender_dispatch_can_be_known_fresh() {
    let first_dispatch = AtomicBool::new(true);

    assert_eq!(
        take_dispatch_freshness(&first_dispatch),
        InvocationFreshnessDisposition::KnownFresh
    );
    assert_eq!(
        take_dispatch_freshness(&first_dispatch),
        InvocationFreshnessDisposition::MayExist
    );
}

#[test]
fn known_fresh_requires_live_execution_and_an_ephemeral_target() {
    assert!(known_fresh_dispatch_allowed(true, true));
    assert!(!known_fresh_dispatch_allowed(true, false));
    assert!(!known_fresh_dispatch_allowed(false, true));
    assert!(!known_fresh_dispatch_allowed(false, false));
}

#[test]
fn deferred_ephemeral_target_never_requests_replay_activation() {
    let target = WasmRpcTargetActivation::DeferredEphemeral {
        env: vec![("KEY".to_string(), "value".to_string())],
        config: vec![],
    };

    assert!(target.deferred_activation().is_none());
    assert_eq!(
        target.target_creation_data().0,
        vec![("KEY".to_string(), "value".to_string())]
    );
}

#[test]
fn pinned_ephemeral_identity_skips_durable_activation() {
    let target = initial_target_activation(vec![], vec![], true);

    assert!(matches!(
        target,
        WasmRpcTargetActivation::DeferredEphemeral { .. }
    ));
    assert!(target.deferred_activation().is_none());
}

#[test]
fn deferred_durable_target_activates_without_a_replay_fingerprint() {
    let target = WasmRpcTargetActivation::DeferredDurable {
        env: vec![("KEY".to_string(), "value".to_string())],
        config: vec![],
    };

    let activation = target.deferred_activation().unwrap();
    assert_eq!(activation.target_fingerprint, None);
    assert_eq!(
        activation.env,
        vec![("KEY".to_string(), "value".to_string())]
    );
}
