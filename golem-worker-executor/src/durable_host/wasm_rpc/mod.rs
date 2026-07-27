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

use crate::durable_host::concurrent::{
    CallHandle, CallReplayOutcome, Cancellable, DeferredCallReplayOutcome, NotCancellable,
    finish_span_in_memory,
};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind, InFunctionRetryHost};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::{
    AsyncInvocationWithMetadata, CancelableScheduledInvocationReceipt, CancellationToken,
    FutureInvokeResult, HostCancellationToken, HostFutureInvokeResult,
    HostFutureInvokeResultWithStore, HostWasmRpc, InvocationMetadata, InvocationResultWithMetadata,
    RpcError, ScheduledInvocationReceipt,
};
use crate::services::HasWorker;
use crate::services::environment_state::EnvironmentStateService;
use crate::services::oplog::CommitLevel;
use crate::services::rpc::{Rpc, RpcDemand, RpcError as InternalRpcError};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use anyhow::Error;
use async_trait::async_trait;
use futures::future::Either;
use golem_common::base_model::agent::{AgentMode, Principal};
use golem_common::model::account::AccountId;
use golem_common::model::agent::{InvocationFreshnessDisposition, ParsedAgentId};
use golem_common::model::component::ComponentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use golem_common::model::oplog::host_functions::{
    GolemRpcCancellationTokenCancel, GolemRpcWasmRpcInvoke, GolemRpcWasmRpcInvokeAndAwaitResult,
    GolemRpcWasmRpcNew, GolemRpcWasmRpcScheduleInvocation,
};
use golem_common::model::oplog::types::{SerializableRpcError, SerializableScheduleId};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRpcInvoke, HostRequestGolemRpcScheduledInvocation,
    HostRequestGolemRpcScheduledInvocationCancellation, HostResponseGolemRpcCreate,
    HostResponseGolemRpcInvokeAndAwait, HostResponseGolemRpcScheduledInvocation,
    HostResponseGolemRpcUnit, HostResponseGolemRpcUnitOrFailure, OplogEntry, PersistenceLevel,
};
use golem_common::model::{
    AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, NamedRetryPolicy, OplogIndex,
    OwnedAgentId, RetryContext, RetryProperties, ScheduleId, ScheduledAction,
};
use golem_common::schema::agent::{AgentMethodSchema, AgentTypeSchema};
use golem_common::schema::schema_value::SchemaValue;
use golem_common::serialization::{deserialize, serialize};
use golem_schema::schema::wit::{
    EncodeError, decode_typed_rejecting_quota_with, decode_value_with, encode_value_with,
};

use crate::durable_host::golem::agent::schema_value_tree_to_typed_constructor_parameters;
use golem_schema::schema::wit::wire as core_wire;
use std::any::Any;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;
use tracing::{Instrument, error};
use wasmtime::component::{Accessor, HasSelf, Resource, ResourceTableError};
use wasmtime_wasi::runtime::AbortOnDropJoinHandle;

use golem_common::model::oplog::payload::HostRequestGolemRpcCreate;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;

/// Host-side resource table entry backing the `golem:agent/host.wasm-rpc` resource.
pub struct WasmRpcEntry {
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

/// Type-erased payload of a [`FutureInvokeResultEntry`] that can be polled for readiness.
#[async_trait::async_trait]
pub trait SubscribeAny: std::any::Any {
    async fn ready(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// Host-side resource table entry backing the `golem:agent/host.future-invoke-result` resource.
pub struct FutureInvokeResultEntry {
    pub payload: Box<dyn SubscribeAny + Send + Sync>,
    /// Tracks child Pollable rep indices created by `subscribe()`.
    /// Used to defer parent deletion until all children are dropped,
    /// because JS GC does not guarantee LIFO drop order.
    pub child_pollables: Vec<u32>,
    /// Set to `true` when the guest drops the parent while children still exist.
    /// The parent entry stays alive until the last child pollable is dropped.
    pub drop_pending: bool,
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for FutureInvokeResultEntry {
    async fn ready(&mut self) {
        self.payload.ready().await
    }
}

impl wasmtime_wasi::DynamicPollable for FutureInvokeResultEntry {
    fn override_index(&self) -> Option<u32> {
        None
    }
}

/// Host-side resource table entry backing the `golem:agent/host.cancellation-token` resource.
pub struct CancellationTokenEntry {
    pub schedule_id: Vec<u8>, // ScheduleId is defined locally in the worker-executor, so store a serialized version here
    pub metadata: InvocationMetadata,
}

fn classify_rpc_error(err: &InternalRpcError) -> HostFailureKind {
    match err {
        InternalRpcError::ProtocolError { .. }
        | InternalRpcError::Denied { .. }
        | InternalRpcError::NotFound { .. } => HostFailureKind::Permanent,
        InternalRpcError::RemoteInternalError { .. } => HostFailureKind::Transient,
    }
}

fn invocation_metadata(
    remote_agent_id: &OwnedAgentId,
    idempotency_key: &IdempotencyKey,
) -> InvocationMetadata {
    InvocationMetadata {
        agent_id: remote_agent_id.agent_id.agent_id.clone(),
        idempotency_key: idempotency_key.value.clone(),
    }
}

impl<Ctx: WorkerCtx> HostWasmRpc for DurableWorkerCtx<Ctx> {
    async fn new(
        &mut self,
        agent_type_name: String,
        constructor: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
        config: Vec<
            golem_common::schema::agent::bindings::golem::agent::common::TypedAgentConfigValue,
        >,
    ) -> anyhow::Result<Resource<WasmRpcEntry>> {
        let mut env =
            wasmtime_wasi::p2::bindings::cli::environment::Host::get_environment(self).await?;
        crate::model::AgentConfig::remove_dynamic_vars(&mut env);

        let registered_agent_type = self
            .get_agent_type_schema_model(golem_common::model::agent::AgentTypeName(
                agent_type_name.clone(),
            ))
            .await?
            .ok_or_else(|| anyhow::anyhow!("Agent type '{}' not found", agent_type_name))?;

        let input = schema_value_tree_to_typed_constructor_parameters(
            constructor,
            &registered_agent_type.agent_type,
            self,
        )
        .map_err(|err| anyhow::anyhow!("Invalid constructor input: {err}"))?;

        let component_id: golem_common::model::component::ComponentId =
            registered_agent_type.implemented_by.component_id;
        let component_revision = registered_agent_type.implemented_by.component_revision;
        let agent_mode = registered_agent_type.agent_type.mode;

        // Share the canonical agent type through `WasmRpcEntryPayload`. Every
        // subsequent RPC entry resolves the per-method input/output schema from
        // this cached value to drive the typed flow. `registered_agent_type` is
        // owned and no longer used, so move its agent type into the `Arc` rather
        // than cloning the whole schema graph.
        let remote_agent_type: Arc<AgentTypeSchema> = Arc::new(registered_agent_type.agent_type);

        let agent_id = golem_common::model::agent::ParsedAgentId::try_new(
            golem_common::model::agent::AgentTypeName(agent_type_name),
            input,
            phantom_id.map(|id| id.into()),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        let remote_agent_id = golem_common::model::AgentId::from_agent_id(component_id, &agent_id)
            .map_err(|err| anyhow::anyhow!("{err}"))?;

        // Each config value is a guest-owned `typed-schema-value` and never
        // legally carries a quota token. Decode through the rejecting path so any
        // owned `quota-token` handle is deleted from the resource table rather
        // than leaked, and drain every config value before surfacing the first
        // error so a handle in a later entry cannot leak when an earlier one is
        // rejected.
        let mut decoded_config = Vec::with_capacity(config.len());
        let mut config_error: Option<anyhow::Error> = None;
        for c in config {
            match decode_typed_rejecting_quota_with(c.value, self) {
                Ok(typed) => {
                    if config_error.is_none() {
                        // The config value travels as a self-contained
                        // `golem:core@2.0.0` typed-schema-value. Render the inner
                        // `SchemaValue` as plain (schema-guided) JSON, matching
                        // the `AgentConfigEntryDto` service-boundary contract: the
                        // DTO carries plain user JSON which
                        // `parse_worker_creation_agent_config` decodes with the
                        // schema graph (`from_json_value`).
                        match golem_common::schema::render::to_json_value(
                            typed.graph(),
                            typed.root_type(),
                            typed.value(),
                        ) {
                            Ok(encoded) => decoded_config.push(AgentConfigEntryDto {
                                path: c.path,
                                value: encoded.into(),
                            }),
                            Err(err) => {
                                config_error =
                                    Some(anyhow::anyhow!("Failed serializing agent config: {err}"));
                            }
                        }
                    }
                }
                Err(err) => {
                    if config_error.is_none() {
                        config_error = Some(anyhow::anyhow!("Invalid agent config value: {err}"));
                    }
                }
            }
        }
        if let Some(err) = config_error {
            return Err(err);
        }
        let config = decoded_config;
        if agent_mode == AgentMode::Ephemeral
            && agent_id.phantom_id.is_some()
            && self.state.is_live()
        {
            return Err(anyhow::anyhow!(
                "An ephemeral RPC proxy cannot select a phantom ID"
            ));
        }

        let span = create_rpc_connection_span(self, &remote_agent_id).await?;

        if agent_mode == AgentMode::Ephemeral {
            let logical_agent_id = agent_id
                .with_phantom_id(None)
                .map_err(|err| anyhow::anyhow!(err))?;
            let logical_remote_agent_id = AgentId::from_agent_id(component_id, &logical_agent_id)
                .map_err(|err| anyhow::anyhow!(err))?;
            let mut logical_remote_agent_id =
                OwnedAgentId::new(self.owned_agent_id.environment_id, &logical_remote_agent_id);

            let mut handle = CallHandle::<GolemRpcWasmRpcNew, NotCancellable>::start(
                self,
                HostRequestGolemRpcCreate {
                    remote_agent_id: remote_agent_id.clone(),
                },
                DurableFunctionType::WriteRemote,
            )
            .await?;
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(response) => {
                        logical_remote_agent_id.environment_id = response.target_environment_id;
                    }
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        handle
                            .complete(
                                self,
                                HostResponseGolemRpcCreate {
                                    target_fingerprint: AgentFingerprint(uuid::Uuid::nil()),
                                    target_environment_id: logical_remote_agent_id.environment_id,
                                },
                            )
                            .await?;
                    }
                }
            } else {
                handle
                    .complete(
                        self,
                        HostResponseGolemRpcCreate {
                            target_fingerprint: AgentFingerprint(uuid::Uuid::nil()),
                            target_environment_id: logical_remote_agent_id.environment_id,
                        },
                    )
                    .await?;
            }

            return construct_ephemeral_wasm_rpc_resource(
                self,
                logical_remote_agent_id,
                logical_agent_id,
                env,
                config,
                span,
                remote_agent_type,
                component_revision,
            );
        }

        let handle = CallHandle::<GolemRpcWasmRpcNew, NotCancellable>::start(
            self,
            HostRequestGolemRpcCreate {
                remote_agent_id: remote_agent_id.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        if !handle.is_live() {
            match handle.replay(self).await? {
                CallReplayOutcome::Replayed(response) => {
                    return reconstruct_wasm_rpc_resource(
                        self,
                        remote_agent_id,
                        response.target_environment_id,
                        response.target_fingerprint,
                        env,
                        config,
                        span,
                        remote_agent_type,
                        component_revision,
                    )
                    .await;
                }
                CallReplayOutcome::Incomplete(live) => {
                    return construct_wasm_rpc_resource(
                        self,
                        live,
                        remote_agent_id,
                        &env,
                        config,
                        span,
                        remote_agent_type,
                        component_revision,
                    )
                    .await;
                }
            }
        }

        construct_wasm_rpc_resource(
            self,
            handle,
            remote_agent_id,
            &env,
            config.clone(),
            span,
            remote_agent_type,
            component_revision,
        )
        .await
    }

    async fn invoke_and_await(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<InvocationResultWithMetadata, RpcError>> {
        let prepared = match prepare_rpc_invocation(
            self,
            &self_,
            "golem::rpc::wasm-rpc::invoke-and-await",
            method_name,
            input,
        )? {
            Ok(prepared) => prepared,
            Err(err) => return Ok(Err(err)),
        };

        let begun = CallHandle::<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let idempotency_key = self.derive_idempotency_key(begun.begin_index());
        let remote_agent_id = invocation_target_agent_id(
            &prepared.logical_remote_agent_id,
            prepared.ephemeral_logical_agent_id.as_ref(),
            &idempotency_key,
        )?;
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
        let request = prepared.invoke_request(&remote_agent_id, &idempotency_key);
        let mut handle = if begun.is_live() {
            begun.start_live(self, request).await?
        } else {
            begun.start_replay(self).await?
        };
        let span = match create_invocation_span(
            self,
            &prepared.connection_span_id,
            &prepared.method_name,
            &idempotency_key,
        )
        .await
        {
            Ok(span) => span,
            Err(err) => {
                handle.abandon_for_trap();
                return Err(err);
            }
        };
        match run_invoke_and_await(
            self,
            self_,
            prepared,
            remote_agent_id,
            idempotency_key,
            span,
            handle,
        )
        .await?
        {
            Ok(value) => Ok(Ok(InvocationResultWithMetadata {
                metadata,
                result: schema_value_to_wire_output(&value, self)?,
            })),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn invoke(
        &mut self,
        self_: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<InvocationMetadata, RpcError>> {
        let prepared = match prepare_rpc_invocation(
            self,
            &self_,
            "golem::rpc::wasm-rpc::invoke",
            method_name,
            input,
        )? {
            Ok(prepared) => prepared,
            Err(err) => return Ok(Err(err)),
        };

        let begun = CallHandle::<GolemRpcWasmRpcInvoke, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let idempotency_key = self.derive_idempotency_key(begun.begin_index());
        let remote_agent_id = invocation_target_agent_id(
            &prepared.logical_remote_agent_id,
            prepared.ephemeral_logical_agent_id.as_ref(),
            &idempotency_key,
        )?;
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
        let request = prepared.invoke_request(&remote_agent_id, &idempotency_key);
        let mut handle = if begun.is_live() {
            begun.start_live(self, request).await?
        } else {
            begun.start_replay(self).await?
        };
        let span = match create_invocation_span(
            self,
            &prepared.connection_span_id,
            &prepared.method_name,
            &idempotency_key,
        )
        .await
        {
            Ok(span) => span,
            Err(err) => {
                handle.abandon_for_trap();
                return Err(err);
            }
        };
        match run_invoke(
            self,
            self_,
            prepared,
            remote_agent_id,
            idempotency_key,
            span,
            handle,
        )
        .await?
        {
            Ok(()) => Ok(Ok(metadata)),
            Err(err) => Ok(Err(err)),
        }
    }

    async fn async_invoke_and_await(
        &mut self,
        this: Resource<WasmRpcEntry>,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<AsyncInvocationWithMetadata> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::async-invoke-and-await")
            .map_err(wasmtime::Error::from)?;

        let own_agent_id = self.owned_agent_id().clone();

        let (
            logical_remote_agent_id,
            connection_span_id,
            remote_agent_type,
            env,
            config,
            deferred_activation,
            ephemeral_logical_agent_id,
        ) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.span_id.clone(),
                payload.remote_agent_type.clone(),
                env,
                config,
                payload.target_activation.deferred_activation(),
                payload.ephemeral_logical_agent_id.clone(),
            )
        };

        if ephemeral_logical_agent_id.is_none() && logical_remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }

        // Check the per-invocation RPC call limit before initiating the call.
        self.state
            .check_and_increment_rpc_call_count()
            .map_err(wasmtime::Error::from)?;

        // Returns Err(WorkerMonthlyRpcCallBudgetExhausted) when exhausted,
        // which maps to RetryDecision::TryStop — suspending the worker.
        self.record_monthly_rpc_call()?;

        // Resolve the method and lift the input before opening any durability. Failures here are
        // deterministic functions of the cached remote agent type and the guest payload, so they
        // are baked into the future's result and surfaced on the first `get` — without opening a
        // durable host call. Live and replay agree because the resolution is pure and no oplog
        // entry (beyond the invocation span) is written for it.
        let input_value =
            match resolve_method_and_lift_input(&remote_agent_type, &method_name, input, self) {
                Ok(parts) => parts,
                Err(rpc_error) => {
                    // The method/input could not be resolved, so no remote call is dispatched. The
                    // idempotency key is informational only and is derived from the current oplog
                    // index; it exists solely to label the invocation span.
                    let oplog_index = self.state.oplog.current_oplog_index().await;
                    let idempotency_key = self.derive_idempotency_key(oplog_index);
                    let remote_agent_id = invocation_target_agent_id(
                        &logical_remote_agent_id,
                        ephemeral_logical_agent_id.as_ref(),
                        &idempotency_key,
                    )?;
                    let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
                    let span = create_invocation_span(
                        self,
                        &connection_span_id,
                        &method_name,
                        &idempotency_key,
                    )
                    .await?;
                    let fut = self.table().push(FutureInvokeResultEntry {
                        payload: Box::new(FutureInvokeResultState::Baked {
                            result: Ok(Err(rpc_error)),
                            span_id: span.span_id().clone(),
                        }),
                        child_pollables: Vec::new(),
                        drop_pending: false,
                    })?;
                    return Ok(AsyncInvocationWithMetadata {
                        future: fut,
                        metadata,
                    });
                }
            };

        // Open the single durable host call for this async RPC as a `WriteRemote` — the same
        // durable function type as the synchronous `invoke_and_await`. It is a two-step call:
        // `begin` yields the begin index and `start_live` then appends the eager host-call `Start`
        // with the built request. The remote idempotency key is derived from the begin index.
        // `start_live` appends the host-call `Start` unconditionally (even under
        // `assume_idempotence`), and the accessor terminals (`complete_access` / `cancel_access` /
        // `replay_access`) all support `WriteRemote`, so no separate durable scope is needed to make
        // the key unique: each concurrently-created future advances the oplog by its own `Start` and
        // therefore derives a distinct key. Under `assume_idempotence` `begin` opens no scope, so
        // the begin index equals the host-call `Start` index; otherwise `begin` opens the durable
        // scope that the terminal later closes. The read-only side-effect guard was already applied
        // at the top of this function and is re-applied by `begin`.
        let begun = CallHandle::<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let begin_index = begun.begin_index();
        let idempotency_key = self.derive_idempotency_key(begin_index);
        let remote_agent_id = invocation_target_agent_id(
            &logical_remote_agent_id,
            ephemeral_logical_agent_id.as_ref(),
            &idempotency_key,
        )?;
        if remote_agent_id == own_agent_id {
            return Err(anyhow::anyhow!(
                "RPC calls to the same agent are not supported"
            ));
        }
        let metadata = invocation_metadata(&remote_agent_id, &idempotency_key);

        let span =
            create_invocation_span(self, &connection_span_id, &method_name, &idempotency_key)
                .await?;

        let request = HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: method_name.clone(),
            input: input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        };

        if begun.is_live() {
            let handle = match begun.start_live(self, request.clone()).await {
                Ok(handle) => handle,
                Err(err) => {
                    // The eager `Start` could not be written; close any durable scope opened by
                    // `begin` and finish the span so no half-open call is left behind.
                    self.end_function(&DurableFunctionType::WriteRemote, begin_index)
                        .await?;
                    self.finish_span(span.span_id()).await?;
                    return Err(err.into());
                }
            };

            let initial_freshness_disposition = if known_fresh_dispatch_allowed(
                self.state.is_live(),
                ephemeral_logical_agent_id.is_some(),
                self.state.assume_idempotence,
                self.state.persistence_level,
            ) {
                InvocationFreshnessDisposition::KnownFresh
            } else {
                InvocationFreshnessDisposition::MayExist
            };
            if initial_freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
                self.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }

            let task = spawn_invoke_and_await_task(
                self,
                &remote_agent_id,
                idempotency_key,
                method_name,
                input_value,
                env.clone(),
                config.clone(),
                deferred_activation.clone(),
                span.span_id(),
                begin_index,
                initial_freshness_disposition,
            );

            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: Some(Arc::new(tokio::sync::Mutex::new(task))),
                    request,
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata,
            })
        } else {
            // Replay: claim the eager `Start` from the oplog now. The RPC is not re-dispatched here.
            // On a normal replay `get` replays the matching `End`, and `cancel` / `drop` consume the
            // matching `Cancelled`. If the worker crashed after this `Start` but before its terminal,
            // `get`'s `replay_access` returns `Incomplete` (a read-only `WriteRemote` call is safe to
            // re-execute) and `get` re-dispatches the RPC there to complete the existing `Start`.
            let handle = begun.start_replay(self).await?;
            let fut = self.table().push(FutureInvokeResultEntry {
                payload: Box::new(FutureInvokeResultState::Active {
                    handle: Some(handle),
                    task: None,
                    request,
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id: span.span_id().clone(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                }),
                child_pollables: Vec::new(),
                drop_pending: false,
            })?;
            Ok(AsyncInvocationWithMetadata {
                future: fut,
                metadata,
            })
        }
    }

    async fn schedule_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<ScheduledInvocationReceipt> {
        let cancellation_token = self
            .schedule_cancelable_invocation_impl(this, scheduled_time, method_name, input)
            .await?;
        let entry = self.table().delete(cancellation_token)?;
        Ok(ScheduledInvocationReceipt {
            metadata: entry.metadata,
        })
    }

    async fn schedule_cancelable_invocation(
        &mut self,
        this: Resource<WasmRpcEntry>,
        scheduled_time: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<CancelableScheduledInvocationReceipt> {
        let cancellation_token = self
            .schedule_cancelable_invocation_impl(this, scheduled_time, method_name, input)
            .await?;
        let metadata = self.table().get(&cancellation_token)?.metadata.clone();
        Ok(CancelableScheduledInvocationReceipt {
            metadata,
            cancellation_token,
        })
    }

    async fn drop(&mut self, rep: Resource<WasmRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::wasm-rpc", "drop");

        let entry = self.table().delete(rep)?;
        let payload = entry.payload.downcast::<WasmRpcEntryPayload>();
        if let Ok(payload) = payload {
            self.finish_span(&payload.span_id).await?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    async fn schedule_cancelable_invocation_impl(
        &mut self,
        this: Resource<WasmRpcEntry>,
        datetime: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
        method_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Resource<CancellationToken>> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::wasm-rpc::schedule-cancelable-invocation")
            .map_err(wasmtime::Error::from)?;

        // Deterministic local validation must happen before opening
        // durability so a guest bug (unknown method, input incompatible
        // with the declared schema, or invalid datetime) does not leave
        // an open durable function. `schedule_cancelable_invocation`
        // has no `RpcError` return channel, so these are surfaced as
        // wasmtime traps.
        let (
            logical_remote_agent_id,
            ephemeral_logical_agent_id,
            remote_agent_type,
            remote_component_revision,
            env,
            config,
        ) = {
            let entry = self.table().get(&this)?;
            let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
            let (env, config) = payload.target_activation.target_creation_data();
            (
                payload.remote_agent_id.clone(),
                payload.ephemeral_logical_agent_id.clone(),
                payload.remote_agent_type.clone(),
                payload.remote_component_revision,
                env,
                config,
            )
        };

        // Lift the input first, then validate the method exists. Lifting
        // consumes any owned `quota-token` handle the guest passed (releasing it
        // from the resource table into a trusted snapshot via the
        // `QuotaTokenResolver`), so it cannot leak if the method check fails. The
        // input then travels as a schema-free `SchemaValue`; the callee
        // validates it against its own schema when it lowers the scheduled
        // invocation.
        let input_value = decode_value_with(input, self)
            .map_err(|err| anyhow::anyhow!("Invalid RPC input: {err}"))?;
        find_agent_method(&remote_agent_type, &method_name)?;
        let scheduled_at = chrono::DateTime::from_timestamp(datetime.seconds, datetime.nanoseconds)
            .ok_or_else(|| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Received invalid datetime from wasi: seconds={}, nanoseconds={}",
                    datetime.seconds, datetime.nanoseconds
                )))
            })?;
        // The persisted request embeds an idempotency key derived from the durable-scope begin
        // index, so this is a two-step call: open the scope first to learn the index, then build
        // the request from it.
        let begun = CallHandle::<GolemRpcWasmRpcScheduleInvocation, NotCancellable>::begin(
            self,
            DurableFunctionType::WriteRemote,
        )
        .await?;
        let begin_index = begun.begin_index();

        // Obtain a live handle to complete — either freshly (writing the eager `Start`) or by
        // recovering an incomplete `Start` from a previous run — or short-circuit on a fully
        // replayed call. The idempotency key is derived once per execution from `begin_index`
        // (which is stable across an incomplete-replay re-execution), so both live paths reproduce
        // the same key and `ScheduleId`.
        let idempotency_key;
        let remote_agent_id;
        let metadata;
        let mut handle;
        if begun.is_live() {
            idempotency_key = self.derive_idempotency_key(begin_index);
            remote_agent_id = invocation_target_agent_id(
                &logical_remote_agent_id,
                ephemeral_logical_agent_id.as_ref(),
                &idempotency_key,
            )?;
            metadata = invocation_metadata(&remote_agent_id, &idempotency_key);

            let request = HostRequestGolemRpcScheduledInvocation {
                remote_agent_id: remote_agent_id.agent_id(),
                idempotency_key: idempotency_key.clone(),
                method_name: method_name.clone(),
                input: input_value.clone(),
                datetime: datetime.into(),
                remote_agent_type: None,
                remote_agent_parameters: None,
            };
            handle = begun.start_live(self, request).await?;
        } else {
            match begun.start_replay(self).await?.replay(self).await? {
                CallReplayOutcome::Replayed(result) => {
                    let idempotency_key = self.derive_idempotency_key(begin_index);
                    let remote_agent_id = invocation_target_agent_id(
                        &logical_remote_agent_id,
                        ephemeral_logical_agent_id.as_ref(),
                        &idempotency_key,
                    )?;
                    let serialized_result = serialize(&result.schedule_id).map_err(|err| {
                        anyhow::Error::from(WorkerExecutorError::runtime(format!(
                            "Failed to serialize schedule id: {err}"
                        )))
                    })?;
                    let resource = self.table().push(CancellationTokenEntry {
                        schedule_id: serialized_result,
                        metadata: invocation_metadata(&remote_agent_id, &idempotency_key),
                    })?;
                    return Ok(resource);
                }
                CallReplayOutcome::Incomplete(live) => {
                    idempotency_key = self.derive_idempotency_key(begin_index);
                    remote_agent_id = invocation_target_agent_id(
                        &logical_remote_agent_id,
                        ephemeral_logical_agent_id.as_ref(),
                        &idempotency_key,
                    )?;
                    metadata = invocation_metadata(&remote_agent_id, &idempotency_key);
                    handle = live;
                }
            }
        }

        let schedule_id = ScheduleId::from_idempotency_key(&idempotency_key);

        let stack = InvocationContextStack::new(
            self.state.invocation_context.trace_id.clone(),
            InvocationContextSpan::external_parent(self.state.current_span_id.clone()),
            self.state.invocation_context.trace_states.clone(),
        );

        let invocation = Box::new(AgentInvocation::AgentMethod {
            idempotency_key: idempotency_key.clone(),
            method_name,
            input: input_value,
            invocation_context: stack,
            principal: Principal::anonymous(),
        });
        let action = if ephemeral_logical_agent_id.is_some() {
            ScheduledAction::InvokeEphemeral {
                account_id: self.created_by(),
                owned_agent_id: remote_agent_id,
                invocation,
                component_revision: remote_component_revision,
                env,
                config,
                parent: Some(self.agent_id().clone()),
                creation_principal: Box::new(Principal::Agent(
                    golem_common::model::agent::AgentPrincipal {
                        agent_id: self.agent_id().clone(),
                    },
                )),
            }
        } else {
            let target_worker_fingerprint = match ensure_rpc_target_activated(self, this).await {
                Ok(fingerprint) => fingerprint,
                Err(err) => {
                    handle.abandon_for_trap();
                    return Err(err);
                }
            };
            ScheduledAction::Invoke {
                account_id: self.created_by(),
                owned_agent_id: remote_agent_id,
                invocation,
                target_worker_fingerprint,
            }
        };

        let result = self
            .state
            .scheduler_service
            .schedule_with_id(schedule_id, scheduled_at, action)
            .await;

        let schedule_id = SerializableScheduleId::from_domain(result);

        let result = handle
            .complete(
                self,
                HostResponseGolemRpcScheduledInvocation { schedule_id },
            )
            .await?;

        let serialized_result = serialize(&result.schedule_id).map_err(|err| {
            anyhow::Error::from(WorkerExecutorError::runtime(format!(
                "Failed to serialize schedule id: {err}"
            )))
        })?;
        let cancellation_token = CancellationTokenEntry {
            schedule_id: serialized_result,
            metadata,
        };

        let resource = self.table().push(cancellation_token)?;
        Ok(resource)
    }
}

struct PreparedRpcInvocation {
    logical_remote_agent_id: OwnedAgentId,
    ephemeral_logical_agent_id: Option<ParsedAgentId>,
    connection_span_id: SpanId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    method_name: String,
    input_value: SchemaValue,
}

impl PreparedRpcInvocation {
    fn invoke_request(
        &self,
        remote_agent_id: &OwnedAgentId,
        idempotency_key: &IdempotencyKey,
    ) -> HostRequestGolemRpcInvoke {
        HostRequestGolemRpcInvoke {
            remote_agent_id: remote_agent_id.agent_id(),
            idempotency_key: idempotency_key.clone(),
            method_name: self.method_name.clone(),
            input: self.input_value.clone(),
            remote_agent_type: None,
            remote_agent_parameters: None,
        }
    }

    fn is_ephemeral(&self) -> bool {
        self.ephemeral_logical_agent_id.is_some()
    }
}

fn prepare_rpc_invocation<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: &Resource<WasmRpcEntry>,
    host_function_name: &str,
    method_name: String,
    input: core_wire::SchemaValueTree,
) -> anyhow::Result<Result<PreparedRpcInvocation, RpcError>> {
    ctx.check_read_only_allows(host_function_name)
        .map_err(wasmtime::Error::from)?;

    let own_agent_id = ctx.owned_agent_id().clone();
    let (
        logical_remote_agent_id,
        ephemeral_logical_agent_id,
        connection_span_id,
        remote_agent_type,
        env,
        config,
    ) = {
        let entry = ctx.table().get(resource)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        let (env, config) = payload.target_activation.target_creation_data();
        (
            payload.remote_agent_id.clone(),
            payload.ephemeral_logical_agent_id.clone(),
            payload.span_id.clone(),
            payload.remote_agent_type.clone(),
            env,
            config,
        )
    };

    ctx.state
        .check_and_increment_rpc_call_count()
        .map_err(wasmtime::Error::from)?;
    ctx.record_monthly_rpc_call()?;

    let input_value =
        match resolve_method_and_lift_input(&remote_agent_type, &method_name, input, ctx) {
            Ok(input_value) => input_value,
            Err(err) => return Ok(Err(err.into())),
        };

    if ephemeral_logical_agent_id.is_none() && logical_remote_agent_id == own_agent_id {
        return Err(anyhow::anyhow!(
            "RPC calls to the same agent are not supported"
        ));
    }

    Ok(Ok(PreparedRpcInvocation {
        logical_remote_agent_id,
        ephemeral_logical_agent_id,
        connection_span_id,
        env,
        config,
        method_name,
        input_value,
    }))
}

async fn run_invoke_and_await<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: Resource<WasmRpcEntry>,
    prepared: PreparedRpcInvocation,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    span: Arc<InvocationContextSpan>,
    mut handle: CallHandle<GolemRpcWasmRpcInvokeAndAwaitResult, NotCancellable>,
) -> anyhow::Result<Result<SchemaValue, RpcError>> {
    let mut freshness_disposition = if known_fresh_dispatch_allowed(
        handle.is_live(),
        prepared.is_ephemeral(),
        ctx.state.assume_idempotence,
        ctx.state.persistence_level,
    ) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };

    let result: Result<SchemaValue, InternalRpcError> = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(persisted) => {
                    break 'result persisted.result.map_err(Into::into);
                }
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }

        if !prepared.is_ephemeral()
            && let Err(err) = ensure_rpc_target_activated(ctx, resource).await
        {
            handle.abandon_for_trap();
            return Err(err);
        }

        let retry_properties =
            RetryContext::rpc("invoke-and-await", &remote_agent_id, &prepared.method_name);
        let result: Result<SchemaValue, InternalRpcError> = loop {
            let stack = ctx.clone_as_inherited_stack(span.span_id());
            let interrupt_signal = ctx
                .execution_status
                .read()
                .unwrap()
                .create_await_interrupt_signal();
            let rpc = ctx.rpc();
            let created_by = ctx.created_by();
            let agent_id = ctx.agent_id().clone();
            let auth_ctx = ctx.agent_auth_ctx();
            let dispatch_freshness = freshness_disposition;
            freshness_disposition = InvocationFreshnessDisposition::MayExist;

            if dispatch_freshness == InvocationFreshnessDisposition::KnownFresh {
                ctx.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }

            let either_result = futures::future::select(
                rpc.invoke_and_await(
                    &remote_agent_id,
                    Some(idempotency_key.clone()),
                    dispatch_freshness,
                    prepared.method_name.clone(),
                    prepared.input_value.clone(),
                    created_by,
                    &agent_id,
                    &prepared.env,
                    stack,
                    prepared.config.clone(),
                    &auth_ctx,
                ),
                interrupt_signal,
            )
            .await;
            let result: Result<SchemaValue, InternalRpcError> = match either_result {
                Either::Left((result, _)) => result,
                Either::Right((interrupt_kind, _)) => {
                    tracing::info!("Interrupted while waiting for RPC result");
                    handle.abandon_for_trap();
                    return Err(interrupt_kind.into());
                }
            };
            match handle
                .try_trigger_retry_or_loop_with_properties(
                    ctx,
                    &result,
                    classify_rpc_error,
                    retry_properties.clone(),
                )
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };

        handle
            .complete(
                ctx,
                HostResponseGolemRpcInvokeAndAwait {
                    result: result.clone().map_err(Into::into),
                },
            )
            .await?;
        result
    };

    ctx.finish_span(span.span_id()).await?;

    match result {
        Ok(value) => Ok(Ok(value)),
        Err(err) => {
            error!("RPC error: {err}");
            Ok(Err(err.into()))
        }
    }
}

async fn run_invoke<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    resource: Resource<WasmRpcEntry>,
    prepared: PreparedRpcInvocation,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    span: Arc<InvocationContextSpan>,
    mut handle: CallHandle<GolemRpcWasmRpcInvoke, NotCancellable>,
) -> anyhow::Result<Result<(), RpcError>> {
    let mut freshness_disposition = if known_fresh_dispatch_allowed(
        handle.is_live(),
        prepared.is_ephemeral(),
        ctx.state.assume_idempotence,
        ctx.state.persistence_level,
    ) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    };

    let result = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await {
                Ok(CallReplayOutcome::Replayed(replayed)) => break 'result Ok(replayed),
                Ok(CallReplayOutcome::Incomplete(live)) => handle = live,
                Err(err) => break 'result Err(err),
            }
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }

        if !prepared.is_ephemeral()
            && let Err(err) = ensure_rpc_target_activated(ctx, resource).await
        {
            handle.abandon_for_trap();
            return Err(err);
        }

        let retry_properties = RetryContext::rpc("invoke", &remote_agent_id, &prepared.method_name);
        let result = loop {
            let stack = ctx.clone_as_inherited_stack(span.span_id());
            let dispatch_freshness = freshness_disposition;
            freshness_disposition = InvocationFreshnessDisposition::MayExist;

            if dispatch_freshness == InvocationFreshnessDisposition::KnownFresh {
                ctx.public_state
                    .worker()
                    .commit_oplog_and_update_state(CommitLevel::DurableOnly)
                    .await;
            }

            let result = ctx
                .rpc()
                .invoke(
                    &remote_agent_id,
                    Some(idempotency_key.clone()),
                    dispatch_freshness,
                    prepared.method_name.clone(),
                    prepared.input_value.clone(),
                    ctx.created_by(),
                    ctx.agent_id(),
                    &prepared.env,
                    stack,
                    prepared.config.clone(),
                    &ctx.agent_auth_ctx(),
                )
                .await;
            match handle
                .try_trigger_retry_or_loop_with_properties(
                    ctx,
                    &result,
                    classify_rpc_error,
                    retry_properties.clone(),
                )
                .await?
            {
                InternalRetryResult::Persist => break result,
                InternalRetryResult::RetryInternally => continue,
            }
        };

        let result = result.map_err(|err| err.into());
        handle
            .complete(ctx, HostResponseGolemRpcUnitOrFailure { result })
            .await
            .map_err(|err| WorkerExecutorError::runtime(err.to_string()))
    };

    ctx.finish_span(span.span_id()).await?;

    match result?.result {
        Ok(_) => Ok(Ok(())),
        Err(err) => {
            let rpc_error: InternalRpcError = err.into();
            error!("RPC error: {rpc_error}");
            Ok(Err(rpc_error.into()))
        }
    }
}

type FutureInvokeTaskResult = Result<Result<SchemaValue, InternalRpcError>, Error>;
type FutureInvokeTaskHandle =
    Arc<tokio::sync::Mutex<AbortOnDropJoinHandle<FutureInvokeTaskResult>>>;
type FutureInvokeGetResult = Result<Result<SchemaValue, RpcError>, Error>;
type FutureInvokeCallHandle = CallHandle<GolemRpcWasmRpcInvokeAndAwaitResult, Cancellable>;

/// Projects a background RPC task result (as produced for the [`FutureInvokeResultState::Baked`]
/// path) into the wire result shape returned by `future-invoke-result.get`. A hard task failure
/// (`Err`) is surfaced as a `get` trap.
fn future_invoke_get_result_to_wire<Ctx: WorkerCtx>(
    result: FutureInvokeGetResult,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(Ok(value)) => Ok(Ok(schema_value_to_wire_output(&value, ctx)?)),
        Ok(Err(rpc_error)) => Ok(Err(rpc_error)),
        Err(err) => Err(err),
    }
}

/// Projects a completed `invoke-and-await` durable response (the payload of the call's `End`) into
/// the wire result shape returned by `future-invoke-result.get`.
fn invoke_and_await_response_to_wire<Ctx: WorkerCtx>(
    result: Result<SchemaValue, SerializableRpcError>,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    match result {
        Ok(value) => Ok(Ok(schema_value_to_wire_output(&value, ctx)?)),
        Err(error) => {
            let rpc_error: InternalRpcError = error.into();
            Ok(Err(rpc_error.into()))
        }
    }
}

fn future_invoke_task_result_to_get_result(
    result: &FutureInvokeTaskResult,
) -> FutureInvokeGetResult {
    match result {
        Ok(Ok(value)) => Ok(Ok(value.clone())),
        Ok(Err(error)) => Ok(Err(error.clone().into())),
        Err(err) => Err(anyhow::anyhow!(err.to_string())),
    }
}

async fn finish_span_access<T, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    span_id: &SpanId,
) -> Result<(), WorkerExecutorError> {
    let (is_live, worker, replay_state) = accessor.with(|mut access| {
        let ctx = access.get();
        (
            ctx.state.is_live(),
            ctx.public_state.worker(),
            ctx.state.replay_state.clone(),
        )
    });

    if is_live {
        worker
            .add_to_oplog(OplogEntry::finish_span(span_id.clone()))
            .await;
    } else {
        crate::get_oplog_entry_owned!(replay_state, OplogEntry::FinishSpan)?;
    }

    accessor.with(|mut access| {
        let ctx = access.get();
        if &ctx.state.current_span_id == span_id {
            let span = ctx.state.invocation_context.get(span_id).map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "span {span_id} missing during finish_span replay: {err}"
                ))
            })?;
            ctx.state.current_span_id = span
                .parent()
                .map(|p| p.span_id().clone())
                .unwrap_or_else(|| ctx.state.invocation_context.root.span_id().clone());
        }
        let _ = ctx
            .state
            .invocation_context
            .finish_span(span_id)
            .map_err(WorkerExecutorError::runtime);
        Ok(())
    })
}

/// Terminal path for a live in-flight `future-invoke-result.get` whose shared cancel token was
/// triggered by a concurrent `future-invoke-result::cancel`. The caller has already dropped the
/// background task (aborting the in-flight RPC). This best-effort cancels the remote invocation,
/// records the call's `Cancelled` terminal with a partial "Invocation cancelled" response (so a
/// replayed `get` deterministically delivers the same result via `replay_access_deferred`),
/// finishes the invocation span, and marks the resource consumed.
async fn cancel_in_flight_get<T: Send + 'static, Ctx: WorkerCtx>(
    accessor: &Accessor<T, HasSelf<DurableWorkerCtx<Ctx>>>,
    handle: FutureInvokeCallHandle,
    request: &HostRequestGolemRpcInvoke,
    span_id: &SpanId,
    this_rep: u32,
) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
    let (worker_proxy, auth_ctx) = accessor.with(|mut access| {
        let ctx = access.get();
        (ctx.worker_proxy(), ctx.agent_auth_ctx())
    });
    if let Err(err) = worker_proxy
        .cancel_invocation(
            &request.remote_agent_id,
            request.idempotency_key.clone(),
            &auth_ctx,
        )
        .await
    {
        tracing::info!(err=%err, "Best-effort cancel_invocation failed");
    }

    let partial_result: Result<SchemaValue, SerializableRpcError> =
        Err(SerializableRpcError::ProtocolError {
            details: "Invocation cancelled".to_string(),
        });
    handle
        .cancel_access(
            accessor,
            accessor.getter(),
            Some(HostResponseGolemRpcInvokeAndAwait {
                result: partial_result.clone(),
            }),
        )
        .await
        .map_err(anyhow::Error::from)?;
    finish_span_access(accessor, span_id).await?;
    accessor.with(|mut access| {
        let ctx = access.get();
        let entry = ctx
            .table()
            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
        let state = entry
            .payload
            .as_any_mut()
            .downcast_mut::<FutureInvokeResultState>()
            .unwrap();
        *state = FutureInvokeResultState::Consumed {
            span_id: span_id.clone(),
        };
        Ok::<_, anyhow::Error>(())
    })?;
    accessor.with(|mut access| invoke_and_await_response_to_wire(partial_result, access.get()))
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>> {
        let this_rep = this.rep();

        // Decide what `get` must do while holding the state lock, then run the async terminal
        // outside it. The durable call handle (and, on the live path, the background task) are taken
        // out of the state here so the terminal runs while nothing in the resource table still owns
        // them: a live handle left behind and later dropped would spuriously enqueue a `Cancelled`.
        #[allow(clippy::large_enum_variant)]
        enum GetPlan {
            /// The result is already known — a baked deterministic failure or a prior cancellation.
            Ready {
                result: anyhow::Result<Result<Option<core_wire::SchemaValueTree>, RpcError>>,
                span_id: SpanId,
                finish_span: bool,
            },
            /// The single durable call is still open; drive it to its `End`. `request`,
            /// `remote_agent_id`, and `env` are carried so a replay that finds an incomplete `Start`
            /// (crash-after-`Start`) can re-dispatch the read-only RPC and complete it.
            /// `cancel_token` is shared with the state left in the table: a concurrent
            /// `future-invoke-result::cancel` (which finds `handle: None` there) triggers it, and
            /// the live await below reacts by cancelling this call.
            Active {
                handle: FutureInvokeCallHandle,
                task: Option<FutureInvokeTaskHandle>,
                request: HostRequestGolemRpcInvoke,
                remote_agent_id: OwnedAgentId,
                env: Vec<(String, String)>,
                config: Vec<AgentConfigEntryDto>,
                deferred_activation: Option<RpcTargetActivation>,
                span_id: SpanId,
                cancel_token: tokio_util::sync::CancellationToken,
            },
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "get");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Consumed { span_id, .. } => GetPlan::Ready {
                    result: Err(anyhow::Error::new(ClassifiedHostError {
                        kind: HostFailureKind::Permanent,
                        message: "future-invoke-result already consumed".to_string(),
                    })),
                    span_id: span_id.clone(),
                    finish_span: false,
                },
                FutureInvokeResultState::Baked { result, span_id } => {
                    let result = future_invoke_task_result_to_get_result(result);
                    let span_id = span_id.clone();
                    *state = FutureInvokeResultState::Consumed {
                        span_id: span_id.clone(),
                    };
                    GetPlan::Ready {
                        result: future_invoke_get_result_to_wire(result, ctx),
                        span_id,
                        finish_span: true,
                    }
                }
                FutureInvokeResultState::Cancelled { span_id } => {
                    let rpc_error = InternalRpcError::ProtocolError {
                        details: "Invocation cancelled".to_string(),
                    };
                    let span_id = span_id.clone();
                    *state = FutureInvokeResultState::Consumed {
                        span_id: span_id.clone(),
                    };
                    GetPlan::Ready {
                        result: Ok(Err(rpc_error.into())),
                        span_id,
                        // The span was already finished by `cancel`.
                        finish_span: false,
                    }
                }
                FutureInvokeResultState::Active {
                    handle,
                    task,
                    request,
                    remote_agent_id,
                    env,
                    config,
                    deferred_activation,
                    span_id,
                    cancel_token,
                } => {
                    let handle = handle
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("future-invoke-result already consumed"))?;
                    GetPlan::Active {
                        handle,
                        task: task.take(),
                        request: request.clone(),
                        remote_agent_id: remote_agent_id.clone(),
                        env: env.clone(),
                        config: config.clone(),
                        deferred_activation: deferred_activation.clone(),
                        span_id: span_id.clone(),
                        cancel_token: cancel_token.clone(),
                    }
                }
            })
        })?;

        match plan {
            GetPlan::Ready {
                result,
                span_id,
                finish_span,
            } => {
                if finish_span {
                    finish_span_access(accessor, &span_id).await?;
                }
                result
            }
            GetPlan::Active {
                mut handle,
                task,
                request,
                remote_agent_id,
                env,
                config,
                deferred_activation,
                span_id,
                cancel_token,
            } => {
                // The response crosses more guest-facing work after the durable `End` (span
                // finish, resource-state transition, wire conversion), so the terminal is
                // recorded through the deferred-delivery API: the returned token stays armed
                // until this method actually returns the result, and a torn future in between
                // records the `CompletionDiscarded` marker. The call's durable `FinishSpan` is
                // appended by the same owned task as the `End` (see `post_end_entry`), so replay
                // can rely on it unconditionally following the `End` on this path.
                let (response, delivery) = if handle.is_live() {
                    let task =
                        task.expect("a live future-invoke-result must own its background task");
                    let task_result = {
                        let mut guard = task.lock().await;
                        tokio::select! {
                            biased;
                            _ = cancel_token.cancelled() => None,
                            result = &mut *guard => Some(result),
                        }
                    };
                    let task_result = match task_result {
                        Some(result) => result,
                        None => {
                            // A concurrent `future-invoke-result::cancel` fired while this `get`
                            // owned the handle. Drop the background task (aborting the in-flight
                            // RPC), then cancel the durable call: best-effort remote cancellation
                            // plus a `Cancelled` terminal carrying the partial "Invocation
                            // cancelled" response, so replay delivers the same result here.
                            drop(task);
                            return cancel_in_flight_get(
                                accessor, handle, &request, &span_id, this_rep,
                            )
                            .await;
                        }
                    };
                    match task_result {
                        Ok(rpc_result) => handle
                            .complete_access_deferred(
                                accessor,
                                accessor.getter(),
                                HostResponseGolemRpcInvokeAndAwait {
                                    result: rpc_result.map_err(Into::into),
                                },
                                Some(OplogEntry::finish_span(span_id.clone())),
                            )
                            .await
                            .map_err(anyhow::Error::from)?,
                        Err(err) => {
                            // The background RPC failed hard after its in-task retries. This is a
                            // trap, not a durable result: abandon the call, leaving its `Start`
                            // incomplete for durable-scope recovery, instead of recording an `End`.
                            return Err(handle.trap(anyhow::anyhow!(err.to_string())));
                        }
                    }
                } else {
                    match handle
                        .replay_access_deferred(accessor, accessor.getter())
                        .await
                        .map_err(anyhow::Error::from)?
                    {
                        DeferredCallReplayOutcome::Replayed(response, delivery) => {
                            (response, delivery)
                        }
                        DeferredCallReplayOutcome::Incomplete(mut live) => {
                            // Crash-after-`Start` recovery: the eager `Start` is committed but its
                            // terminal was never written. A read-only `WriteRemote` call is safe to
                            // re-execute, so re-dispatch the RPC now and complete the existing `Start`
                            // (mirrors the synchronous path's `Incomplete` -> re-run).
                            let retry_point = live.begin_index();
                            let mut task = accessor.with(|mut access| {
                                let ctx = access.get();
                                spawn_invoke_and_await_task(
                                    ctx,
                                    &remote_agent_id,
                                    request.idempotency_key.clone(),
                                    request.method_name.clone(),
                                    request.input.clone(),
                                    env.clone(),
                                    config.clone(),
                                    deferred_activation.clone(),
                                    &span_id,
                                    retry_point,
                                    InvocationFreshnessDisposition::MayExist,
                                )
                            });
                            let task_result = tokio::select! {
                                biased;
                                _ = cancel_token.cancelled() => None,
                                result = &mut task => Some(result),
                            };
                            match task_result {
                                None => {
                                    // Cancelled while re-executing the recovered call: same
                                    // handling as the live-path cancellation race above.
                                    drop(task);
                                    return cancel_in_flight_get(
                                        accessor, live, &request, &span_id, this_rep,
                                    )
                                    .await;
                                }
                                Some(Ok(rpc_result)) => live
                                    .complete_access_deferred(
                                        accessor,
                                        accessor.getter(),
                                        HostResponseGolemRpcInvokeAndAwait {
                                            result: rpc_result.map_err(Into::into),
                                        },
                                        Some(OplogEntry::finish_span(span_id.clone())),
                                    )
                                    .await
                                    .map_err(anyhow::Error::from)?,
                                Some(Err(err)) => {
                                    return Err(live.trap(anyhow::anyhow!(err.to_string())));
                                }
                            }
                        }
                    }
                };

                if delivery.is_replay_discarded() {
                    // The recorded run persisted the `End` (and its `FinishSpan`) but the guest
                    // dropped this future before `get` returned. Mirror the recorded post-`End`
                    // continuation deterministically — consume the positional `FinishSpan` and
                    // mark the resource consumed — then park: never return the response, so the
                    // deterministic guest drops this future at the same point it did live (its
                    // resource `drop` sees no open handle and writes nothing durable).
                    finish_span_access(accessor, &span_id).await?;
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed {
                            span_id: span_id.clone(),
                        };
                        Ok::<_, anyhow::Error>(())
                    })?;
                    std::future::pending::<()>().await;
                    unreachable!("std::future::pending never completes")
                }

                if delivery.is_live_armed() {
                    // Live: the durable `FinishSpan` is already recorded by the owned terminal
                    // task; everything left before the return is synchronous (in-memory span
                    // finish, resource transition, wire conversion), so no tear window remains
                    // between here and consuming the token.
                    let finalize = accessor.with(|mut access| {
                        let ctx = access.get();
                        finish_span_in_memory(ctx, &span_id).map_err(anyhow::Error::from)?;
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed {
                            span_id: span_id.clone(),
                        };
                        invoke_and_await_response_to_wire(response.result, ctx)
                    });
                    match finalize {
                        Ok(wire) => {
                            // The wire result returned below still crosses Wasmtime's lowering
                            // and terminal-consumption boundary: hand the token to the terminal
                            // observer instead of consuming it here.
                            delivery.deliver_at_accessor_terminal(accessor);
                            Ok(wire)
                        }
                        Err(err) => {
                            // The error is observed by the caller (the worker traps): the
                            // completion was not silently discarded, so no marker.
                            delivery.suppress();
                            Err(err)
                        }
                    }
                } else {
                    // Replay of a delivered completion, or a live unpersisted (snapshotting)
                    // call: the original span handling applies — replay consumes the positional
                    // `FinishSpan`, an unpersisted live call appends it here.
                    finish_span_access(accessor, &span_id).await?;
                    accessor.with(|mut access| {
                        let ctx = access.get();
                        let entry = ctx
                            .table()
                            .get_mut(&Resource::<FutureInvokeResult>::new_borrow(this_rep))?;
                        let state = entry
                            .payload
                            .as_any_mut()
                            .downcast_mut::<FutureInvokeResultState>()
                            .unwrap();
                        *state = FutureInvokeResultState::Consumed {
                            span_id: span_id.clone(),
                        };
                        Ok::<_, anyhow::Error>(())
                    })?;
                    let wire = accessor.with(|mut access| {
                        invoke_and_await_response_to_wire(response.result, access.get())
                    });
                    delivery.delivered();
                    wire
                }
            }
        }
    }

    async fn drop(
        accessor: &Accessor<U, Self>,
        this: Resource<FutureInvokeResult>,
    ) -> anyhow::Result<()> {
        let future_rep = this.rep();

        #[allow(clippy::large_enum_variant)]
        enum DropPlan {
            /// An open durable call must be cancelled and its span finished.
            Cancel {
                handle: FutureInvokeCallHandle,
                span_id: SpanId,
            },
            /// No durable call, but the invocation span of a baked failure is still open.
            FinishSpan { span_id: SpanId },
            /// Nothing to finish — already consumed or cancelled.
            Nothing,
        }

        let plan = accessor.with(|mut access| {
            let ctx = access.get();
            ctx.observe_function_call("golem::rpc::future-invoke-result", "drop");
            let entry = ctx
                .table()
                .get_mut(&Resource::<FutureInvokeResult>::new_borrow(future_rep))?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            Ok::<_, anyhow::Error>(match state {
                FutureInvokeResultState::Active {
                    handle, span_id, ..
                } => match handle.take() {
                    Some(handle) => DropPlan::Cancel {
                        handle,
                        span_id: span_id.clone(),
                    },
                    None => DropPlan::Nothing,
                },
                FutureInvokeResultState::Baked { span_id, .. } => DropPlan::FinishSpan {
                    span_id: span_id.clone(),
                },
                FutureInvokeResultState::Cancelled { .. }
                | FutureInvokeResultState::Consumed { .. } => DropPlan::Nothing,
            })
        })?;

        match plan {
            DropPlan::Cancel { handle, span_id } => {
                handle
                    .cancel_access(accessor, accessor.getter(), None)
                    .await
                    .map_err(anyhow::Error::from)?;
                finish_span_access(accessor, &span_id).await?;
            }
            DropPlan::FinishSpan { span_id } => {
                finish_span_access(accessor, &span_id).await?;
            }
            DropPlan::Nothing => {}
        }

        accessor.with(|mut access| {
            let ctx = access.get();
            match ctx.table().delete(this) {
                Ok(entry) => {
                    for child_rep in &entry.child_pollables {
                        ctx.state.rpc_pollable_to_parent.remove(child_rep);
                    }
                }
                Err(ResourceTableError::HasChildren) => {
                    let parent: Resource<FutureInvokeResult> = Resource::new_borrow(future_rep);
                    ctx.table().get_mut(&parent)?.drop_pending = true;
                }
                Err(err) => return Err(err.into()),
            }

            Ok(())
        })
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<FutureInvokeResult>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::future-invoke-result", "cancel");

        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::future-invoke-result::cancel")
            .map_err(wasmtime::Error::from)?;

        // Decide what to cancel while holding the table borrow, taking the durable call handle out
        // of the state (a live handle must never drop while still owned here). The background task,
        // if any, is dropped together with the old `Active` state, aborting the in-flight RPC.
        #[allow(clippy::large_enum_variant)]
        enum CancelPlan {
            /// The single durable call is still open; record its `Cancelled` and, for a live call,
            /// best-effort cancel the remote invocation.
            Cancel {
                handle: FutureInvokeCallHandle,
                remote_agent_id: AgentId,
                idempotency_key: IdempotencyKey,
                span_id: SpanId,
            },
            /// Nothing to cancel: a baked failure, or already cancelled / consumed.
            Nothing,
        }

        let plan = {
            let entry = self.table().get_mut(&this)?;
            let state = entry
                .payload
                .as_any_mut()
                .downcast_mut::<FutureInvokeResultState>()
                .unwrap();
            match state {
                FutureInvokeResultState::Active {
                    handle,
                    request,
                    span_id,
                    cancel_token,
                    ..
                } => match handle.take() {
                    Some(handle) => {
                        let remote_agent_id = request.remote_agent_id.clone();
                        let idempotency_key = request.idempotency_key.clone();
                        let span_id = span_id.clone();
                        *state = FutureInvokeResultState::Cancelled {
                            span_id: span_id.clone(),
                        };
                        CancelPlan::Cancel {
                            handle,
                            remote_agent_id,
                            idempotency_key,
                            span_id,
                        }
                    }
                    None => {
                        // An in-flight `get` owns the handle. Signal it through the shared
                        // token: the racing `get` aborts the RPC, records the `Cancelled`
                        // terminal (with the partial "Invocation cancelled" response), and
                        // finishes the span, so nothing durable is written here.
                        cancel_token.cancel();
                        CancelPlan::Nothing
                    }
                },
                FutureInvokeResultState::Baked { .. }
                | FutureInvokeResultState::Cancelled { .. }
                | FutureInvokeResultState::Consumed { .. } => CancelPlan::Nothing,
            }
        };

        if let CancelPlan::Cancel {
            handle,
            remote_agent_id,
            idempotency_key,
            span_id,
        } = plan
        {
            // Best-effort remote cancellation, only meaningful for a live call — on replay the
            // recorded `Cancelled` is re-applied without re-issuing the side effect.
            if handle.is_live()
                && let Err(err) = self
                    .worker_proxy()
                    .cancel_invocation(&remote_agent_id, idempotency_key, &self.agent_auth_ctx())
                    .await
            {
                tracing::info!(err=%err, "Best-effort cancel_invocation failed");
            }

            handle
                .cancel(self, None)
                .await
                .map_err(anyhow::Error::from)?;
            self.finish_span(&span_id).await?;
        }

        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostCancellationToken for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        // Trap immediately if the invocation is restricted to read-only side effects.
        self.check_read_only_allows("golem::rpc::cancellation-token::cancel")
            .map_err(wasmtime::Error::from)?;

        let entry = self.table().get(&this)?;
        let serialized_schedule_id: SerializableScheduleId = deserialize(&entry.schedule_id)
            .map_err(|err| {
                anyhow::Error::from(WorkerExecutorError::runtime(format!(
                    "Failed to deserialize cancellation token: {err}"
                )))
            })?;

        let mut handle = CallHandle::<GolemRpcCancellationTokenCancel, NotCancellable>::start(
            self,
            HostRequestGolemRpcScheduledInvocationCancellation {
                schedule_id: serialized_schedule_id.clone(),
            },
            DurableFunctionType::WriteRemote,
        )
        .await?;

        'cancel: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(_) => break 'cancel,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            self.scheduler_service()
                .cancel(serialized_schedule_id.into_domain())
                .await;

            handle.complete(self, HostResponseGolemRpcUnit {}).await?;
        }

        Ok(())
    }

    async fn drop(&mut self, this: Resource<CancellationToken>) -> anyhow::Result<()> {
        self.observe_function_call("golem::rpc::cancellation-token", "drop");
        let _ = self.table().delete(this)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> core_wire::Host for DurableWorkerCtx<Ctx> {
    async fn parse_uuid(
        &mut self,
        uuid: String,
    ) -> anyhow::Result<Result<core_wire::Uuid, String>> {
        Ok(uuid::Uuid::parse_str(&uuid)
            .map(|uuid| uuid.into())
            .map_err(|e| e.to_string()))
    }

    async fn uuid_to_string(&mut self, uuid: core_wire::Uuid) -> anyhow::Result<String> {
        let uuid: uuid::Uuid = uuid.into();
        Ok(uuid.to_string())
    }
}

fn construct_ephemeral_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: OwnedAgentId,
    logical_agent_id: ParsedAgentId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_component_revision: ComponentRevision,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    Ok(ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: Some(logical_agent_id),
            span_id: span.span_id().clone(),
            target_activation: WasmRpcTargetActivation::DeferredEphemeral { env, config },
            remote_agent_type,
            remote_component_revision,
        }),
    })?)
}

fn invocation_target_agent_id(
    logical_remote_agent_id: &OwnedAgentId,
    ephemeral_logical_agent_id: Option<&ParsedAgentId>,
    idempotency_key: &IdempotencyKey,
) -> anyhow::Result<OwnedAgentId> {
    let Some(logical_agent_id) = ephemeral_logical_agent_id else {
        return Ok(logical_remote_agent_id.clone());
    };

    let invocation_agent_id = logical_agent_id
        .with_ephemeral_invocation_phantom(idempotency_key)
        .map_err(|err| anyhow::anyhow!(err))?;
    let invocation_agent_id =
        AgentId::from_agent_id(logical_remote_agent_id.component_id(), &invocation_agent_id)
            .map_err(|err| anyhow::anyhow!(err))?;
    Ok(OwnedAgentId::new(
        logical_remote_agent_id.environment_id,
        &invocation_agent_id,
    ))
}

pub async fn construct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    mut handle: CallHandle<GolemRpcWasmRpcNew, NotCancellable>,
    remote_agent_id: AgentId,
    env: &[(String, String)],
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_component_revision: ComponentRevision,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let stack = ctx.clone_as_inherited_stack(span.span_id());

    let target_component = match ctx
        .component_service()
        .get_metadata(remote_agent_id.component_id, None)
        .await
    {
        Ok(target_component) => target_component,
        Err(err) => {
            handle.abandon_for_trap();
            return Err(err.into());
        }
    };
    let target_environment_id = target_component.environment_id;
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let demand = match ctx
        .rpc()
        .create_demand(
            &remote_agent_id,
            ctx.created_by(),
            ctx.agent_id(),
            env,
            stack,
            config.clone(),
            &ctx.agent_auth_ctx(),
        )
        .await
    {
        Ok(demand) => demand,
        Err(err) => {
            handle.abandon_for_trap();
            return Err(err.into());
        }
    };
    let target_fingerprint = demand.fingerprint();

    handle
        .complete(
            ctx,
            HostResponseGolemRpcCreate {
                target_fingerprint,
                target_environment_id,
            },
        )
        .await?;

    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: None,
            span_id: span.span_id().clone(),
            target_activation: WasmRpcTargetActivation::Activated {
                demand,
                target_fingerprint,
                env: env.to_vec(),
                config,
            },
            remote_agent_type,
            remote_component_revision,
        }),
    })?;
    Ok(entry)
}

async fn reconstruct_wasm_rpc_resource<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: AgentId,
    target_environment_id: EnvironmentId,
    target_fingerprint: AgentFingerprint,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    span: Arc<InvocationContextSpan>,
    remote_agent_type: Arc<AgentTypeSchema>,
    remote_component_revision: ComponentRevision,
) -> anyhow::Result<Resource<WasmRpcEntry>> {
    let remote_agent_id = OwnedAgentId::new(target_environment_id, &remote_agent_id);
    let entry = ctx.table().push(WasmRpcEntry {
        payload: Box::new(WasmRpcEntryPayload {
            remote_agent_id,
            ephemeral_logical_agent_id: None,
            span_id: span.span_id().clone(),
            target_activation: WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            },
            remote_agent_type,
            remote_component_revision,
        }),
    })?;
    Ok(entry)
}

/// Activates the remote RPC target by creating a fresh demand and verifying that the live
/// fingerprint matches the one persisted at construction time. Failures are returned as
/// [`ClassifiedHostError`]s so every call site classifies activation errors the same way: a
/// `create_demand` failure inherits its transient/permanent kind from [`classify_rpc_error`], while
/// a fingerprint change is always permanent.
async fn activate_rpc_target(
    rpc: &dyn Rpc,
    remote_agent_id: &OwnedAgentId,
    created_by: AccountId,
    agent_id: &AgentId,
    env: &[(String, String)],
    stack: InvocationContextStack,
    config: Vec<AgentConfigEntryDto>,
    auth_ctx: &AuthCtx,
    expected_fingerprint: AgentFingerprint,
) -> Result<Box<dyn RpcDemand>, Error> {
    let demand = rpc
        .create_demand(
            remote_agent_id,
            created_by,
            agent_id,
            env,
            stack,
            config,
            auth_ctx,
        )
        .await
        .map_err(|err| classified_host_error(classify_rpc_error(&err), err.to_string()))?;
    let target_fingerprint = demand.fingerprint();
    if target_fingerprint != expected_fingerprint {
        return Err(classified_host_error(
            HostFailureKind::Permanent,
            format!(
                "RPC target activation fingerprint changed during replay: persisted={expected_fingerprint}, live={target_fingerprint}"
            ),
        ));
    }
    Ok(demand)
}

async fn ensure_rpc_target_activated<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    this: Resource<WasmRpcEntry>,
) -> anyhow::Result<AgentFingerprint> {
    let (remote_agent_id, span_id, env, config, replayed_target_fingerprint) = {
        let entry = ctx.table().get(&this)?;
        let payload = entry.payload.downcast_ref::<WasmRpcEntryPayload>().unwrap();
        match &payload.target_activation {
            WasmRpcTargetActivation::Activated {
                target_fingerprint, ..
            } => return Ok(*target_fingerprint),
            WasmRpcTargetActivation::DeferredEphemeral { .. } => {
                return Err(anyhow::anyhow!(
                    "An ephemeral RPC target does not support pre-activation"
                ));
            }
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => (
                payload.remote_agent_id.clone(),
                payload.span_id.clone(),
                env.clone(),
                config.clone(),
                *target_fingerprint,
            ),
        }
    };

    let stack = ctx.clone_as_inherited_stack(&span_id);
    let rpc = ctx.rpc();
    let demand = activate_rpc_target(
        rpc.as_ref(),
        &remote_agent_id,
        ctx.created_by(),
        ctx.agent_id(),
        &env,
        stack,
        config.clone(),
        &ctx.agent_auth_ctx(),
        replayed_target_fingerprint,
    )
    .await?;

    let entry = ctx.table().get_mut(&this)?;
    let payload = entry.payload.downcast_mut::<WasmRpcEntryPayload>().unwrap();
    payload.target_activation = WasmRpcTargetActivation::Activated {
        demand,
        target_fingerprint: replayed_target_fingerprint,
        env,
        config,
    };

    Ok(replayed_target_fingerprint)
}

struct TaskRetryParams<Ctx: WorkerCtx> {
    environment_state_service: Arc<dyn EnvironmentStateService>,
    environment_id: EnvironmentId,
    default_retry_policy: NamedRetryPolicy,
    agent_config_retry_policies: Vec<NamedRetryPolicy>,
    runtime_retry_policy_mutations: std::collections::BTreeMap<String, Option<NamedRetryPolicy>>,
    retry_properties: RetryProperties,
    max_in_function_retry_delay: Duration,
    worker: Arc<crate::worker::Worker<Ctx>>,
    retry_point: OplogIndex,
    execution_status: Arc<std::sync::RwLock<crate::model::ExecutionStatus>>,
}

enum RpcTaskError {
    Rpc(InternalRpcError),
    Host(Error),
}

impl std::fmt::Display for RpcTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcTaskError::Rpc(err) => write!(f, "{err}"),
            RpcTaskError::Host(err) => write!(f, "{err}"),
        }
    }
}

fn classify_rpc_task_error(err: &RpcTaskError) -> HostFailureKind {
    match err {
        RpcTaskError::Rpc(err) => classify_rpc_error(err),
        RpcTaskError::Host(err) => err
            .downcast_ref::<ClassifiedHostError>()
            .map(|err| err.kind)
            .unwrap_or(HostFailureKind::Permanent),
    }
}

fn classified_host_error(kind: HostFailureKind, message: String) -> Error {
    Error::new(ClassifiedHostError { kind, message })
}

fn take_dispatch_freshness(
    known_fresh_dispatch_available: &std::sync::atomic::AtomicBool,
) -> InvocationFreshnessDisposition {
    if known_fresh_dispatch_available.swap(false, std::sync::atomic::Ordering::SeqCst) {
        InvocationFreshnessDisposition::KnownFresh
    } else {
        InvocationFreshnessDisposition::MayExist
    }
}

fn known_fresh_dispatch_allowed(
    is_live: bool,
    is_ephemeral: bool,
    assume_idempotence: bool,
    persistence_level: PersistenceLevel,
) -> bool {
    is_live
        && is_ephemeral
        && !assume_idempotence
        && persistence_level != PersistenceLevel::PersistNothing
}

fn spawn_rpc_task_with_retry<Ctx: WorkerCtx>(
    rpc: Arc<dyn Rpc>,
    remote_agent_id: OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    created_by: AccountId,
    agent_id: AgentId,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    stack: InvocationContextStack,
    retry_params: Option<TaskRetryParams<Ctx>>,
    auth_ctx: AuthCtx,
    target_activation: Option<RpcTargetActivation>,
    initial_freshness_disposition: InvocationFreshnessDisposition,
) -> AbortOnDropJoinHandle<Result<Result<SchemaValue, InternalRpcError>, Error>> {
    let first_dispatch = Arc::new(std::sync::atomic::AtomicBool::new(
        initial_freshness_disposition == InvocationFreshnessDisposition::KnownFresh,
    ));
    let invoke = move || {
        let rpc = rpc.clone();
        let remote_agent_id = remote_agent_id.clone();
        let idempotency_key = idempotency_key.clone();
        let method_name = method_name.clone();
        let input = input.clone();
        let created_by = created_by;
        let agent_id = agent_id.clone();
        let env = env.clone();
        let stack = stack.clone();
        let auth_ctx = auth_ctx.clone();
        let target_activation = target_activation.clone();
        let config = config.clone();
        let freshness_disposition = take_dispatch_freshness(&first_dispatch);
        async move {
            let _demand = if let Some(target_activation) = target_activation {
                let demand = activate_rpc_target(
                    rpc.as_ref(),
                    &remote_agent_id,
                    created_by,
                    &agent_id,
                    &target_activation.env,
                    stack.clone(),
                    target_activation.config,
                    &auth_ctx,
                    target_activation.target_fingerprint,
                )
                .await
                .map_err(RpcTaskError::Host)?;
                Some(demand)
            } else {
                None
            };

            let result = rpc
                .invoke_and_await(
                    &remote_agent_id,
                    Some(idempotency_key),
                    freshness_disposition,
                    method_name,
                    input,
                    created_by,
                    &agent_id,
                    &env,
                    stack,
                    config,
                    &auth_ctx,
                )
                .await
                .map_err(RpcTaskError::Rpc)?;
            Ok(result)
        }
    };

    wasmtime_wasi::runtime::spawn(
        async move {
            let result = if let Some(retry_params) = retry_params {
                let execution_status = retry_params.execution_status;
                let current_retry_policy_state = retry_params
                    .worker
                    .get_non_detached_last_known_status()
                    .await
                    .current_retry_state
                    .get(&retry_params.retry_point)
                    .cloned();
                let task_ctx = crate::durable_host::durability::TaskRetryContext {
                    retry_point: retry_params.retry_point,
                    environment_state_service: retry_params.environment_state_service,
                    environment_id: retry_params.environment_id,
                    default_retry_policy: retry_params.default_retry_policy,
                    agent_config_retry_policies: retry_params.agent_config_retry_policies,
                    runtime_retry_policy_mutations: retry_params.runtime_retry_policy_mutations,
                    max_in_function_retry_delay: retry_params.max_in_function_retry_delay,
                    current_retry_policy_state,
                    retry_properties: retry_params.retry_properties,
                    worker: retry_params.worker,
                };
                crate::durable_host::durability::in_task_retry_loop(
                    task_ctx,
                    classify_rpc_task_error,
                    invoke,
                    || {
                        execution_status
                            .read()
                            .unwrap()
                            .create_await_interrupt_signal()
                    },
                )
                .await
            } else {
                invoke().await
            };
            match result {
                Ok(result) => Ok(Ok(result)),
                Err(RpcTaskError::Rpc(err)) => Ok(Err(err)),
                Err(RpcTaskError::Host(err)) => Err(err),
            }
        }
        .in_current_span(),
    )
}

fn spawn_invoke_and_await_task<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    remote_agent_id: &OwnedAgentId,
    idempotency_key: IdempotencyKey,
    method_name: String,
    input: SchemaValue,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
    deferred_activation: Option<RpcTargetActivation>,
    span_id: &SpanId,
    retry_point: OplogIndex,
    initial_freshness_disposition: InvocationFreshnessDisposition,
) -> AbortOnDropJoinHandle<FutureInvokeTaskResult> {
    let retry_params = if ctx.in_atomic_region() {
        None
    } else {
        let mut retry_properties =
            RetryContext::rpc("invoke-and-await", remote_agent_id, &method_name);
        ctx.state.enrich_retry_properties(&mut retry_properties);
        Some(TaskRetryParams {
            environment_state_service: ctx.state.environment_state_service.clone(),
            environment_id: ctx.state.owned_agent_id.environment_id,
            default_retry_policy: NamedRetryPolicy::default_from_config(&ctx.state.config.retry),
            agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
            runtime_retry_policy_mutations: ctx.state.runtime_retry_policy_mutations.clone(),
            retry_properties,
            max_in_function_retry_delay: ctx.durable_execution_state().max_in_function_retry_delay,
            worker: ctx.public_state.worker(),
            retry_point,
            execution_status: ctx.execution_status.clone(),
        })
    };
    spawn_rpc_task_with_retry(
        ctx.rpc(),
        remote_agent_id.clone(),
        idempotency_key,
        method_name,
        input,
        ctx.created_by(),
        ctx.agent_id().clone(),
        env,
        config,
        ctx.clone_as_inherited_stack(span_id),
        retry_params,
        ctx.agent_auth_ctx(),
        deferred_activation,
        initial_freshness_disposition,
    )
}

pub struct WasmRpcEntryPayload {
    pub remote_agent_id: OwnedAgentId,
    pub ephemeral_logical_agent_id: Option<ParsedAgentId>,
    pub span_id: SpanId,
    pub target_activation: WasmRpcTargetActivation,
    /// Cached remote agent type, used to resolve per-method input/output
    /// schemas for the in-process [`SchemaValue`] / [`TypedSchemaValue`]
    /// flow. Sourced from the durable `get_agent_type` lookup performed in
    /// [`HostWasmRpc::new`], so it is consistent across live execution and
    /// replay.
    pub remote_agent_type: Arc<AgentTypeSchema>,
    pub remote_component_revision: ComponentRevision,
}

pub enum WasmRpcTargetActivation {
    DeferredEphemeral {
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
    Activated {
        #[allow(dead_code)]
        demand: Box<dyn RpcDemand>,
        target_fingerprint: AgentFingerprint,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
    ReplayPending {
        target_fingerprint: AgentFingerprint,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
    },
}

impl WasmRpcTargetActivation {
    fn target_creation_data(&self) -> (Vec<(String, String)>, Vec<AgentConfigEntryDto>) {
        match self {
            WasmRpcTargetActivation::DeferredEphemeral { env, config }
            | WasmRpcTargetActivation::Activated { env, config, .. }
            | WasmRpcTargetActivation::ReplayPending { env, config, .. } => {
                (env.clone(), config.clone())
            }
        }
    }

    fn deferred_activation(&self) -> Option<RpcTargetActivation> {
        match self {
            WasmRpcTargetActivation::DeferredEphemeral { .. } => None,
            WasmRpcTargetActivation::ReplayPending {
                target_fingerprint,
                env,
                config,
            } => Some(RpcTargetActivation {
                target_fingerprint: *target_fingerprint,
                env: env.clone(),
                config: config.clone(),
            }),
            WasmRpcTargetActivation::Activated { .. } => None,
        }
    }
}

#[derive(Clone)]
struct RpcTargetActivation {
    target_fingerprint: AgentFingerprint,
    env: Vec<(String, String)>,
    config: Vec<AgentConfigEntryDto>,
}

impl Debug for WasmRpcEntryPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmRpcEntryPayload")
            .field("remote_agent_id", &self.remote_agent_id)
            .finish()
    }
}

/// Look up an [`AgentMethod`] by name from the cached remote agent type.
/// Used on the schedule path where the result is surfaced as a
/// `wasmtime::Error` trap, since `schedule_cancelable_invocation` has no
/// way to return `Err(RpcError)` to the guest.
fn find_agent_method<'a>(
    agent_type: &'a AgentTypeSchema,
    method_name: &str,
) -> anyhow::Result<&'a AgentMethodSchema> {
    agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            )
        })
}

/// Resolve and lift the guest-side input value tree into the schema-native
/// [`SchemaValue`] carrier used across the executor↔executor RPC hop.
///
/// The wire value tree is transported as a schema-free [`SchemaValue`]; each
/// end validates it against its own declared schema (the callee when it lowers
/// the invocation, see [`lower_invocation`](crate::worker::invocation)). The
/// method is resolved only to fast-fail an unknown method before durability is
/// opened — a deterministic check that replay reproduces, surfaced as
/// [`InternalRpcError`] so the caller can return `Err(RpcError)` to the guest.
///
/// Any owned `quota-token` handle the guest passed in the input is consumed
/// from the resource table here and converted into its trusted
/// [`SchemaValue::QuotaToken`] snapshot via the `QuotaTokenResolver`, so the
/// capability travels across the RPC hop as an unforgeable host snapshot rather
/// than a guest-visible handle.
fn resolve_method_and_lift_input<Ctx: WorkerCtx>(
    agent_type: &AgentTypeSchema,
    method_name: &str,
    input: core_wire::SchemaValueTree,
    resolver: &mut DurableWorkerCtx<Ctx>,
) -> Result<SchemaValue, InternalRpcError> {
    // Lift (and thereby consume) the guest input *before* the method-existence
    // check. The owned `quota-token` handles the input may carry were already
    // transferred into the host resource table at the WIT boundary, and the
    // unknown-method branch returns a non-trapping `RpcError` (or, for
    // `async-invoke-and-await`, a baked future) that leaves the instance — and
    // its resource table — alive. Decoding first guarantees those handles are
    // consumed/dropped even when the method is unknown.
    let input_value =
        decode_value_with(input, resolver).map_err(|err| InternalRpcError::ProtocolError {
            details: format!("Invalid RPC input for method '{method_name}': {err}"),
        })?;
    agent_type
        .methods
        .iter()
        .find(|m| m.name == method_name)
        .ok_or_else(|| InternalRpcError::NotFound {
            details: format!(
                "Method '{method_name}' not found on agent type '{}'",
                agent_type.type_name
            ),
        })?;
    Ok(input_value)
}

/// Project an RPC output [`SchemaValue`] into the WIT
/// `option<schema-value-tree>` result shape used by `invoke-and-await` and
/// `future-invoke-result.get`.
///
/// Per the `golem:agent@2.0.0` contract a declared `unit` output (the
/// canonical empty tuple) maps to `none`, while a `single` output maps to
/// `some(value)`. A method that declares a single `()`/empty-tuple output is
/// structurally indistinguishable from `unit` here and is likewise reported as
/// `none`; both live and replay paths funnel through this helper, so the choice
/// is applied consistently.
///
/// Lowering the reply to the guest-facing wire form mints a fresh owned
/// `quota-token` handle for every [`SchemaValue::QuotaToken`] snapshot via the
/// `QuotaTokenResolver`, so a capability returned from an RPC call reaches the
/// caller's guest as an opaque, unforgeable resource handle.
fn schema_value_to_wire_output<Ctx: WorkerCtx>(
    value: &SchemaValue,
    resolver: &mut DurableWorkerCtx<Ctx>,
) -> Result<Option<core_wire::SchemaValueTree>, EncodeError> {
    match value {
        SchemaValue::Tuple { elements } if elements.is_empty() => Ok(None),
        value => Ok(Some(encode_value_with(value, resolver)?)),
    }
}

pub async fn create_rpc_connection_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    target_agent_id: &AgentId,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_span(
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-connection".to_string()),
                ),
                (
                    "target_agent_id".to_string(),
                    AttributeValue::String(target_agent_id.to_string()),
                ),
            ],
            false,
        )
        .await?)
}

pub async fn create_invocation_span<Ctx: InvocationContextManagement>(
    ctx: &mut Ctx,
    connection_span_id: &SpanId,
    function_name: &str,
    idempotency_key: &IdempotencyKey,
) -> anyhow::Result<Arc<InvocationContextSpan>> {
    Ok(ctx
        .start_child_span(
            connection_span_id,
            &[
                (
                    "name".to_string(),
                    AttributeValue::String("rpc-invocation".to_string()),
                ),
                (
                    "function_name".to_string(),
                    AttributeValue::String(function_name.to_string()),
                ),
                (
                    "idempotency_key".to_string(),
                    AttributeValue::String(idempotency_key.to_string()),
                ),
            ],
        )
        .await?)
}

#[allow(clippy::large_enum_variant)]
enum FutureInvokeResultState {
    /// The eager host-call `Start` has been written (live) or claimed from the oplog (replay). The
    /// single [`FutureInvokeCallHandle`] owns both halves of the durable call: `get` finishes it
    /// with an `End`, `cancel` / `drop` with a `Cancelled`. It is always `take`n out of the state
    /// before a terminal runs, so a live handle is never dropped while still owned here (which would
    /// spuriously enqueue a `Cancelled`).
    ///
    /// `task` is `Some` only on the live path — the background RPC task whose result feeds the `End`.
    /// On the replay path it is `None`: `get` replays the recorded `End`. If replay instead finds an
    /// incomplete `Start` (the worker crashed after the eager `Start` but before its terminal), `get`
    /// re-dispatches the read-only RPC — using `remote_agent_id`, `request`, and `env` — and
    /// completes the existing `Start`.
    ///
    /// `cancel_token` is the side-channel for cancelling an *in-flight* `get`: `get` takes the
    /// handle (and task) out of this state before awaiting, so a concurrent `cancel` finds
    /// `handle: None` and cannot record the `Cancelled` itself. Instead it triggers this token;
    /// the in-flight `get` races its await against it, and on cancellation aborts the RPC task,
    /// best-effort cancels the remote invocation, and records the `Cancelled` (with a partial
    /// "Invocation cancelled" response so replay deterministically delivers the same result).
    Active {
        handle: Option<FutureInvokeCallHandle>,
        task: Option<FutureInvokeTaskHandle>,
        request: HostRequestGolemRpcInvoke,
        remote_agent_id: OwnedAgentId,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
        deferred_activation: Option<RpcTargetActivation>,
        span_id: SpanId,
        cancel_token: tokio_util::sync::CancellationToken,
    },
    /// Method resolution / input lifting failed deterministically before any host call was opened,
    /// so no `Start` / `End` is written for this future. `get` surfaces the baked error and finishes
    /// the span. Live and replay agree because the failure is a pure function of the cached remote
    /// agent type and the guest payload.
    Baked {
        result: FutureInvokeTaskResult,
        span_id: SpanId,
    },
    /// The future was cancelled: its host call recorded a `Cancelled` and its span was finished.
    /// `get` returns a cancellation error without touching the oplog.
    Cancelled { span_id: SpanId },
    /// `get` already produced the result and finished the span; a second `get` traps.
    Consumed { span_id: SpanId },
}

impl Debug for FutureInvokeResultState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active { .. } => write!(f, "Active"),
            Self::Baked { .. } => write!(f, "Baked"),
            Self::Cancelled { .. } => write!(f, "Cancelled"),
            Self::Consumed { .. } => write!(f, "Consumed"),
        }
    }
}

#[async_trait]
impl SubscribeAny for FutureInvokeResultState {
    async fn ready(&mut self) {
        // The p3 `future-invoke-result` resource exposes no `subscribe`, so this pollable-readiness
        // hook is never driven: `get` is the sole consumer and awaits the background task directly.
        // Kept only to satisfy the `SubscribeAny` bound of the resource-table payload.
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::rpc::RpcError as ServiceRpcError;
    use async_trait::async_trait;
    use golem_common::data_value;
    use golem_common::model::component::ComponentId;
    use golem_service_base::model::auth::AuthCtx;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use test_r::test;
    use uuid::Uuid;

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
    }

    #[async_trait]
    impl Rpc for FingerprintMismatchRpc {
        async fn create_demand(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            _self_env: &[(String, String)],
            _self_stack: InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
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
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            _self_env: &[(String, String)],
            _self_stack: InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
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
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            self_env: &[(String, String)],
            _self_stack: InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
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
        ) -> Result<SchemaValue, ServiceRpcError> {
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

    #[test]
    async fn deferred_activation_fingerprint_mismatch_is_a_host_failure_not_rpc_result() {
        let persisted_fingerprint = AgentFingerprint(Uuid::from_u128(10));
        let live_fingerprint = AgentFingerprint(Uuid::from_u128(11));
        let rpc = Arc::new(FingerprintMismatchRpc {
            live_fingerprint,
            invoke_called: AtomicBool::new(false),
        });

        let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
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
                target_fingerprint: persisted_fingerprint,
                env: vec![],
                config: vec![],
            }),
            InvocationFreshnessDisposition::MayExist,
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
    async fn deferred_activation_create_demand_failure_is_a_host_failure_not_rpc_result() {
        let rpc = Arc::new(ActivationFailureRpc {
            invoke_called: AtomicBool::new(false),
        });

        let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
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
                target_fingerprint: AgentFingerprint(Uuid::from_u128(10)),
                env: vec![],
                config: vec![],
            }),
            InvocationFreshnessDisposition::MayExist,
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
                target_fingerprint: persisted_fingerprint,
                env: vec![],
                config: vec![],
            }),
            InvocationFreshnessDisposition::MayExist,
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
        });
        let replay_pending = WasmRpcTargetActivation::ReplayPending {
            target_fingerprint: persisted_fingerprint,
            env: env_from_wasm_rpc_new.clone(),
            config: vec![],
        };

        let result = spawn_rpc_task_with_retry::<crate::workerctx::default::Context>(
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
    fn known_fresh_requires_a_durably_recorded_live_host_call() {
        assert!(known_fresh_dispatch_allowed(
            true,
            true,
            false,
            PersistenceLevel::Smart,
        ));
        assert!(!known_fresh_dispatch_allowed(
            true,
            true,
            true,
            PersistenceLevel::Smart,
        ));
        assert!(!known_fresh_dispatch_allowed(
            true,
            true,
            false,
            PersistenceLevel::PersistNothing,
        ));
        assert!(!known_fresh_dispatch_allowed(
            false,
            true,
            false,
            PersistenceLevel::Smart,
        ));
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
}
