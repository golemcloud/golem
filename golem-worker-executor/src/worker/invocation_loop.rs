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

use crate::model::{ReadFileResult, TrapType};
use crate::services::events::Event;
use crate::services::oplog::{CommitLevel, OplogOps};
use crate::services::{HasEvents, HasOplog, HasWorker};
use crate::worker::invocation::{invoke_observed_and_traced, InvokeResult};
use crate::worker::{
    interpret_function_result, QueuedWorkerInvocation, RetryDecision, RunningWorker, Worker,
    WorkerCommand,
};
use crate::workerctx::{PublicWorkerIo, WorkerCtx};
use anyhow::anyhow;
use async_mutex::Mutex;
use drop_stream::DropStream;
use futures::channel::oneshot;
use futures::channel::oneshot::Sender;
use golem_common::model::agent::AgentId;
use golem_common::model::oplog::WorkerError;
use golem_common::model::{
    invocation_context::{AttributeValue, InvocationContextStack},
    GetFileSystemNodeResult, OplogIndex,
};
use golem_common::model::{
    ComponentFilePath, ComponentType, ComponentVersion, IdempotencyKey, OwnedWorkerId,
    TimestampedWorkerInvocation, WorkerId, WorkerInvocation,
};
use golem_common::retries::get_delay;
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_wasm::analysis::AnalysedFunctionResult;
use golem_wasm::Value;
use std::collections::VecDeque;
use std::ops::DerefMut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::RwLock;
use tracing::{debug, span, warn, Instrument, Level};
use wasmtime::component::Instance;
use wasmtime::{AsContext, Store};

/// Context of a running worker's invocation loop
pub struct InvocationLoop<Ctx: WorkerCtx> {
    pub receiver: UnboundedReceiver<WorkerCommand>,
    pub active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    pub owned_worker_id: OwnedWorkerId,
    pub parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    pub waiting_for_command: Arc<AtomicBool>,
    pub oom_retry_count: u32,
}

impl<Ctx: WorkerCtx> InvocationLoop<Ctx> {
    /// Runs the invocation loop of a running worker, responsible for processing incoming
    /// invocation and update commands one by one.
    ///
    /// The outer invocation loop consists of the following steps:
    ///
    /// - Creating the worker instance
    /// - Recovering the worker state
    /// - Processing incoming commands in the inner invocation loop
    /// - Suspending the worker
    /// - Process the retry decision
    pub async fn run(&mut self) {
        loop {
            debug!("Invocation queue loop creating the instance");

            let (instance, store) = if let Some((instance, store)) = self.create_instance().await {
                (instance, store)
            } else {
                // early return, can't retry a failed instance creation
                break;
            };

            debug!("Invocation queue loop preparing the instance");

            let mut final_decision = self.recover_instance_state(&instance, &store).await;

            if final_decision.is_none() {
                let mut inner_loop = InnerInvocationLoop {
                    receiver: &mut self.receiver,
                    active: self.active.clone(),
                    owned_worker_id: self.owned_worker_id.clone(),
                    parent: self.parent.clone(),
                    waiting_for_command: self.waiting_for_command.clone(),
                    instance: &instance,
                    store: &store,
                };
                final_decision = inner_loop.run().await;
            }

            self.suspend_worker(&store).await;

            match final_decision {
                None | Some(RetryDecision::None) => {
                    debug!("Invocation queue loop notifying parent about being stopped");
                    self.parent.stop_internal(true, None).await;
                    break;
                }
                Some(RetryDecision::Immediate) => {
                    debug!("Invocation queue loop triggering restart immediately");
                    continue;
                }
                Some(RetryDecision::Delayed(delay)) => {
                    debug!("Invocation queue loop sleeping for {delay:?} for delayed restart");
                    tokio::time::sleep(delay).await;
                    debug!("Invocation queue loop restarting after delay");
                    continue;
                }
                Some(RetryDecision::ReacquirePermits) => {
                    let delay = get_delay(self.parent.oom_retry_config(), self.oom_retry_count);
                    debug!("Invocation queue loop dropping memory permits and triggering restart with a delay of {delay:?}");
                    let _ = Worker::restart_on_oom(
                        self.parent.clone(),
                        true,
                        delay,
                        self.oom_retry_count + 1,
                    )
                    .await;
                    break;
                }
            }
        }
    }

    /// Create the worker instance and publish an event about it
    async fn create_instance(&self) -> Option<(Instance, Mutex<Store<Ctx>>)> {
        match RunningWorker::create_instance(self.parent.clone()).await {
            Ok((instance, store)) => {
                self.parent.events().publish(Event::WorkerLoaded {
                    worker_id: self.owned_worker_id.worker_id(),
                    result: Ok(()),
                });
                Some((instance, store))
            }
            Err(err) => {
                warn!("Failed to start the worker: {err}");
                self.parent.events().publish(Event::WorkerLoaded {
                    worker_id: self.owned_worker_id.worker_id(),
                    result: Err(err.clone()),
                });
                self.parent.stop_internal(true, Some(err)).await;
                None
            }
        }
    }

    /// Prepares the instance for running by recovering its persisted state
    ///
    /// In case of failure to recover the state, it returns the retry decision to be used.
    async fn recover_instance_state(
        &self,
        instance: &Instance,
        store: &Mutex<Store<Ctx>>,
    ) -> Option<RetryDecision> {
        let mut store = store.lock().await;

        store.data().set_suspended();

        let span = span!(
            Level::INFO,
            "invocation",
            worker_id = %self.owned_worker_id.worker_id,
            agent_type = self.parent
                .agent_id
                .as_ref()
                .map(|id| id.agent_type.clone())
                .unwrap_or_else(|| "-".to_string()),
        );
        let prepare_result =
            Ctx::prepare_instance(&self.owned_worker_id.worker_id, instance, &mut *store)
                .instrument(span)
                .await;

        match prepare_result {
            Ok(decision) => {
                debug!("Recovery decision from prepare_instance: {decision:?}");
                decision
            }
            Err(err) => {
                warn!("Failed to start the worker: {err}");
                store.data().set_suspended();

                self.parent.stop_internal(true, Some(err)).await;
                Some(RetryDecision::None) // early return, we can't retry this
            }
        }
    }

    /// Suspends the worker after the invocation loop exited
    async fn suspend_worker(&self, store: &Mutex<Store<Ctx>>) {
        // Marking the worker as suspended
        store.lock().await.data().set_suspended();

        // Making sure all pending commits are flushed
        // Make sure all pending commits are done
        store
            .lock()
            .await
            .data()
            .get_public_state()
            .worker()
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await;
    }
}

struct InnerInvocationLoop<'a, Ctx: WorkerCtx> {
    receiver: &'a mut UnboundedReceiver<WorkerCommand>,
    active: Arc<RwLock<VecDeque<QueuedWorkerInvocation>>>,
    owned_worker_id: OwnedWorkerId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    waiting_for_command: Arc<AtomicBool>,
    instance: &'a Instance,
    store: &'a Mutex<Store<Ctx>>,
}

impl<Ctx: WorkerCtx> InnerInvocationLoop<'_, Ctx> {
    /// The inner invocation loop started when the worker instance state is fully restored
    /// and the worker is ready to take invocations.
    ///
    /// This loop exits when the unbounded message queue owned by the RunningWorker is dropped,
    /// or when an error occurs in one of the command handlers.
    ///
    /// The inner loop only runs if the retry decision coming from `recover_instance_state` is `None`,
    /// meaning there were no errors during the instance preparation. The inner loop can override this
    /// decision in the following way:
    /// - If it returns `RetryDecision::None`, it means it is not possible to retry the outer loop and the whole invocation loop should be stopped.
    /// - Otherwise it returns either `None` if there were no errors, otherwise the retry decision coming from the
    ///   underlying retry logic.
    ///
    /// The outer loop should either break or use the returned retry decision after the inner loop quits.
    pub async fn run(&mut self) -> Option<RetryDecision> {
        debug!("Invocation queue loop started");

        let mut final_decision = None;

        // Exits when RunningWorker is dropped
        self.waiting_for_command.store(true, Ordering::Release);
        while let Some(cmd) = self.receiver.recv().await {
            self.waiting_for_command.store(false, Ordering::Release);
            let outcome = match cmd {
                WorkerCommand::Invocation => {
                    let message = self.active.write().await.pop_front();

                    if let Some(message) = message {
                        self.invocation(message).await
                    } else {
                        CommandOutcome::Continue
                    }
                }
                WorkerCommand::ResumeReplay => self.resume_replay().await,
                WorkerCommand::Interrupt(kind) => self.interrupt(kind).await,
            };
            match outcome {
                CommandOutcome::BreakOuterLoop => {
                    final_decision = Some(RetryDecision::None);
                    break;
                }
                CommandOutcome::BreakInnerLoop(decision) => {
                    final_decision = Some(decision);
                    break;
                }
                CommandOutcome::Continue => {}
            }

            self.waiting_for_command.store(true, Ordering::Release);
        }
        self.waiting_for_command.store(false, Ordering::Release);

        debug!(final_decision = ?final_decision, "Invocation queue loop finished");

        final_decision
    }

    /// Resumes an interrupted replay process
    ///
    /// Returns `CommandOutcome` if this fails and the invocation loop should be stopped.
    /// Otherwise, it returns the new retry decision to be used by the outer invocation loop.
    async fn resume_replay(&self) -> CommandOutcome {
        let mut store = self.store.lock().await;

        let resume_replay_result = Ctx::resume_replay(&mut *store, self.instance, true).await;

        match resume_replay_result {
            Ok(None) => CommandOutcome::Continue,
            Ok(Some(decision)) => CommandOutcome::BreakInnerLoop(decision),
            Err(err) => {
                warn!("Failed to resume replay: {err}");
                store.data().set_suspended();

                self.parent.stop_internal(true, Some(err)).await;
                CommandOutcome::BreakOuterLoop
            }
        }
    }

    /// Performs a queued invocation on the worker
    ///
    /// The queued invocations are grouped into "external" invocations, that are observable by the users
    /// in the worker's invocation queue, oplog, etc., and some internal invocations that we use for
    /// concurrency control.
    async fn invocation(&mut self, message: QueuedWorkerInvocation) -> CommandOutcome {
        let mut store = self.store.lock().await;
        let store = store.deref_mut();

        let mut invocation = Invocation {
            owned_worker_id: self.owned_worker_id.clone(),
            parent: self.parent.clone(),
            instance: self.instance,
            store,
        };
        invocation.process(message).await
    }

    /// Performs an interrupt request
    async fn interrupt(&self, kind: InterruptKind) -> CommandOutcome {
        match kind {
            InterruptKind::Restart | InterruptKind::Jump => {
                CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
            }
            _ => CommandOutcome::BreakInnerLoop(RetryDecision::None),
        }
    }
}

/// Context for performing one `QueuedWorkerInvocation`
///
/// The most important part of is that unlike the `InnerInvocationLoop`, it holds a locked
/// mutable reference to the instance `Store`. The instance mutex is held for the whole duration
/// of performing an invocation.
struct Invocation<'a, Ctx: WorkerCtx> {
    owned_worker_id: OwnedWorkerId,
    parent: Arc<Worker<Ctx>>, // parent must not be dropped until the invocation_loop is running
    instance: &'a Instance,
    store: &'a mut Store<Ctx>,
}

impl<Ctx: WorkerCtx> Invocation<'_, Ctx> {
    /// Process a queued worker invocation
    async fn process(&mut self, message: QueuedWorkerInvocation) -> CommandOutcome {
        match message {
            QueuedWorkerInvocation::External {
                invocation,
                canceled,
            } => {
                if !canceled {
                    self.external_invocation(invocation).await
                } else {
                    CommandOutcome::Continue
                }
            }
            QueuedWorkerInvocation::GetFileSystemNode { path, sender } => {
                self.get_file_system_node(path, sender).await;
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::ReadFile { path, sender } => {
                self.read_file(path, sender).await;
                CommandOutcome::Continue
            }
            QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                let _ = sender.send(Ok(()));
                CommandOutcome::Continue
            }
        }
    }

    /// Process an external queued worker invocation - this is either an exported function invocation
    /// or a manual update request (which involves invoking the exported save-snapshot functions, so
    /// it is a special case of the exported function invocation).
    async fn external_invocation(&mut self, inner: TimestampedWorkerInvocation) -> CommandOutcome {
        match inner.invocation {
            WorkerInvocation::ExportedFunction {
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            } => {
                // Need to check if the same idempotency key has already been processed and then ignore this entry.
                let has_result = {
                    let invocation_results = self.parent.invocation_results.read().await;
                    invocation_results.contains_key(&idempotency_key)
                };
                if !has_result {
                    self.invoke_exported_function(
                        invocation_context,
                        idempotency_key,
                        full_function_name,
                        function_input,
                    )
                    .await
                } else {
                    debug!("Skipping enqueued invocation with idempotency key {idempotency_key} as it already has a result");
                    CommandOutcome::Continue
                }
            }
            WorkerInvocation::ManualUpdate { target_version } => {
                self.manual_update(target_version).await
            }
        }
    }

    /// Invokes an exported function on the worker
    async fn invoke_exported_function(
        &mut self,
        invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
    ) -> CommandOutcome {
        let span = span!(
            Level::INFO,
            "invocation",
            worker_id = %self.owned_worker_id.worker_id,
            agent_type = self.parent
                .agent_id
                .as_ref()
                .map(|id| id.agent_type.clone())
                .unwrap_or_else(|| "-".to_string()),
            %idempotency_key,
            function = full_function_name
        );

        self.invoke_exported_function_inner(
            invocation_context,
            idempotency_key,
            full_function_name,
            function_input,
        )
        .instrument(span)
        .await
    }

    /// Invokes an exported function on the worker
    ///
    /// The inner implementation of `invoke_exported_function` to be instrumented with a span.
    async fn invoke_exported_function_inner(
        &mut self,
        invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        full_function_name: String,
        function_input: Vec<Value>,
    ) -> CommandOutcome {
        let result = self
            .invoke_exported_function_with_context(
                invocation_context,
                idempotency_key,
                &full_function_name,
                &function_input,
            )
            .await;

        match result {
            Ok(InvokeResult::Succeeded {
                output,
                consumed_fuel,
            }) => {
                self.exported_function_invocation_finished(
                    full_function_name,
                    &function_input,
                    output,
                    consumed_fuel,
                )
                .await
            }
            _ => self.exported_function_invocation_failed(result).await,
        }
    }

    /// Sets the necessary contextual information on the worker and performs the actual
    /// invocation.
    async fn invoke_exported_function_with_context(
        &mut self,
        mut invocation_context: InvocationContextStack,
        idempotency_key: IdempotencyKey,
        full_function_name: &str,
        function_input: &[Value],
    ) -> Result<InvokeResult, WorkerExecutorError> {
        self.store
            .data_mut()
            .set_current_idempotency_key(idempotency_key.clone())
            .await;

        let component_metadata = self.store.data().component_metadata().metadata.clone();

        Self::extend_invocation_context(
            &mut invocation_context,
            &idempotency_key,
            full_function_name,
            &self.owned_worker_id.worker_id(),
            &self.parent.agent_id,
        );

        let (local_span_ids, inherited_span_ids) = invocation_context.span_ids();
        self.store
            .data_mut()
            .set_current_invocation_context(invocation_context)
            .await?;

        if let Some(idempotency_key) = self.store.data().get_current_idempotency_key().await {
            self.store
                .data()
                .get_public_state()
                .worker()
                .store_invocation_resuming(&idempotency_key)
                .await;
        }

        let result = invoke_observed_and_traced(
            full_function_name.to_string(),
            function_input.to_owned(),
            self.store,
            self.instance,
            &component_metadata,
            true,
        )
        .await;

        // We are removing the spans introduced by the invocation. Not calling `finish_span` here,
        // as it would add FinishSpan oplog entries without corresponding StartSpan ones. Instead,
        // the oplog processor should assume that spans implicitly created by ExportedFunctionInvoked
        // are finished at ExportedFunctionCompleted.
        for span_id in local_span_ids {
            self.store.data_mut().remove_span(&span_id)?;
        }
        for span_id in inherited_span_ids {
            self.store.data_mut().remove_span(&span_id)?;
        }

        result
    }

    /// The logic handling a successfully finished worker invocation
    ///
    /// Successful here means that the invocation function returned with
    /// `InvokeResult::Succeeded`. As the returned values get further processing,
    /// the whole invocation can still fail during that.
    async fn exported_function_invocation_finished(
        &mut self,
        full_function_name: String,
        function_input: &Vec<Value>,
        output: Option<Value>,
        consumed_fuel: i64,
    ) -> CommandOutcome {
        let component_metadata = self.store.as_context().data().component_metadata();

        let function_results = component_metadata
            .metadata
            .find_function(&full_function_name);

        match function_results {
            Ok(Some(invokable_function)) => {
                let function_results = invokable_function.analysed_export.result.clone();

                match self
                    .exported_function_invocation_finished_with_type(
                        full_function_name,
                        function_input,
                        output,
                        consumed_fuel,
                        function_results,
                    )
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        self.store
                            .data_mut()
                            .on_invocation_failure(&TrapType::Error {
                                error: WorkerError::Unknown(error.to_string()),
                                retry_from: OplogIndex::INITIAL,
                            })
                            .await;
                        CommandOutcome::BreakInnerLoop(RetryDecision::None)
                    }
                }
            }

            Ok(None) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(&TrapType::Error {
                        error: WorkerError::InvalidRequest("Function not found".to_string()),
                        retry_from: OplogIndex::INITIAL,
                    })
                    .await;
                CommandOutcome::BreakInnerLoop(RetryDecision::None)
            }

            Err(err) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(&TrapType::Error {
                        error: WorkerError::InvalidRequest(format!(
                            "Failed analysing function: {err}"
                        )),
                        retry_from: OplogIndex::INITIAL,
                    })
                    .await;
                CommandOutcome::BreakInnerLoop(RetryDecision::None)
            }
        }
    }

    /// The inner logic of handling a successfully finished worker invocation,
    /// with the function's expected result type already known
    async fn exported_function_invocation_finished_with_type(
        &mut self,
        full_function_name: String,
        function_input: &Vec<Value>,
        output: Option<Value>,
        consumed_fuel: i64,
        function_result: Option<AnalysedFunctionResult>,
    ) -> Result<CommandOutcome, WorkerExecutorError> {
        let result = interpret_function_result(output, function_result).map_err(|e| {
            WorkerExecutorError::ValueMismatch {
                details: e.join(", "),
            }
        });

        match result {
            Ok(result) => {
                self.store
                    .data_mut()
                    .on_invocation_success(
                        &full_function_name,
                        function_input,
                        consumed_fuel,
                        result,
                    )
                    .await?;

                if self.store.data().component_metadata().component_type == ComponentType::Ephemeral
                {
                    Ok(CommandOutcome::BreakInnerLoop(RetryDecision::None))
                } else {
                    Ok(CommandOutcome::Continue)
                }
            }
            Err(error) => {
                let trap_type = TrapType::from_error::<Ctx>(&anyhow!(error), OplogIndex::INITIAL);

                self.store
                    .data_mut()
                    .on_invocation_failure(&trap_type)
                    .await;
                Ok(CommandOutcome::BreakInnerLoop(RetryDecision::None))
            }
        }
    }

    /// The logic handling a worker invocation that did not succeed.
    async fn exported_function_invocation_failed(
        &mut self,
        result: Result<InvokeResult, WorkerExecutorError>,
    ) -> CommandOutcome {
        let trap_type = match result {
            Ok(invoke_result) => invoke_result.as_trap_type::<Ctx>(),
            Err(error) => Some(TrapType::from_error::<Ctx>(
                &anyhow!(error),
                OplogIndex::INITIAL,
            )),
        };
        let decision = match trap_type {
            Some(trap_type) => {
                self.store
                    .data_mut()
                    .on_invocation_failure(&trap_type)
                    .await
            }
            None => RetryDecision::None,
        };

        CommandOutcome::BreakInnerLoop(decision)
    }

    /// Try to perform the save-snapshot step of a manual update on the worker
    async fn manual_update(&mut self, target_version: ComponentVersion) -> CommandOutcome {
        let span = span!(
            Level::INFO,
            "manual_update",
            worker_id = %self.owned_worker_id.worker_id,
            target_version = %target_version,
            agent_type = self.parent
                .agent_id
                .as_ref()
                .map(|id| id.agent_type.clone())
                .unwrap_or_else(|| "-".to_string()),
        );

        self.manual_update_inner(target_version)
            .instrument(span)
            .await
    }

    /// The inner implementation of the manual update command
    async fn manual_update_inner(&mut self, target_version: ComponentVersion) -> CommandOutcome {
        let _idempotency_key = {
            let ctx = self.store.data_mut();
            let idempotency_key = IdempotencyKey::fresh();
            ctx.set_current_idempotency_key(idempotency_key.clone())
                .await;
            idempotency_key
        };
        let component_metadata = self.store.data().component_metadata().metadata.clone();

        match component_metadata.save_snapshot() {
            Ok(Some(save_snapshot)) => {
                self.store.data_mut().begin_call_snapshotting_function();

                let result = invoke_observed_and_traced(
                    save_snapshot.name.to_string(),
                    vec![],
                    self.store,
                    self.instance,
                    &component_metadata,
                    true,
                )
                    .await;
                self.store.data_mut().end_call_snapshotting_function();

                match result {
                    Ok(InvokeResult::Succeeded { output, .. }) => {
                        if let Some(bytes) = Self::decode_snapshot_result(output) {
                            match self
                                .store
                                .data()
                                .get_public_state()
                                .oplog()
                                .create_snapshot_based_update_description(target_version, &bytes)
                                .await
                            {
                                Ok(update_description) => {
                                    // Enqueue the update
                                    self.parent.enqueue_update(update_description).await;

                                    // Reactivate the worker
                                    CommandOutcome::BreakInnerLoop(RetryDecision::Immediate)
                                    // Stop processing the queue to avoid race conditions
                                }
                                Err(error) => {
                                    self.fail_update(
                                        target_version,
                                        format!(
                                            "failed to store the snapshot for manual update: {error}"
                                        ),
                                    )
                                        .await
                                }
                            }
                        } else {
                            self.fail_update(
                                target_version,
                                "failed to get a snapshot for manual update: invalid snapshot result"
                                    .to_string(),
                            )
                                .await
                        }
                    }
                    Ok(InvokeResult::Failed { error, .. }) => {
                        let stderr = self
                            .store
                            .data()
                            .get_public_state()
                            .event_service()
                            .get_last_invocation_errors();
                        let error = error.to_string(&stderr);
                        self.fail_update(
                            target_version,
                            format!("failed to get a snapshot for manual update: {error}"),
                        )
                            .await
                    }
                    Ok(InvokeResult::Exited { .. }) => {
                        self.fail_update(
                            target_version,
                            "failed to get a snapshot for manual update: it called exit"
                                .to_string(),
                        )
                            .await
                    }
                    Ok(InvokeResult::Interrupted { interrupt_kind, .. }) => {
                        self.fail_update(
                            target_version,
                            format!(
                                "failed to get a snapshot for manual update: {interrupt_kind:?}"
                            ),
                        )
                            .await
                    }
                    Err(error) => {
                        self.fail_update(
                            target_version,
                            format!("failed to get a snapshot for manual update: {error:?}"),
                        )
                            .await
                    }
                }
            }
            Ok(None) => {
                self.fail_update(
                    target_version,
                    "failed to get a snapshot for manual update: save-snapshot is not exported"
                        .to_string(),
                )
                    .await
            }
            Err(error) => {
                self.fail_update(
                    target_version,
                    format!("failed to get a snapshot for manual update: error while finding the exported save-snapshot function: {error}"),
                )
                    .await
            }
        }
    }

    /// Performs a directory listing command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    async fn get_file_system_node(
        &self,
        path: ComponentFilePath,
        sender: Sender<Result<GetFileSystemNodeResult, WorkerExecutorError>>,
    ) {
        let result = self.store.data().get_file_system_node(&path).await;
        let _ = sender.send(result);
    }

    /// Performs a read file command on the worker's file system
    ///
    /// These are threaded through the invocation loop to make sure they are not accessing the file system concurrently with invocations
    /// that may modify them.
    async fn read_file(
        &self,
        path: ComponentFilePath,
        sender: Sender<Result<ReadFileResult, WorkerExecutorError>>,
    ) {
        let result = self.store.data().read_file(&path).await;
        match result {
            Ok(ReadFileResult::Ok(stream)) => {
                // special case. We need to wait until the stream is consumed to avoid corruption
                //
                // This will delay processing of the next invocation and is quite unfortunate.
                // A possible improvement would be to check whether we are on a copy-on-write filesystem
                // if yes, we can make a cheap copy of the file here and serve the read from that copy.

                let (latch, latch_receiver) = oneshot::channel();
                let drop_stream = DropStream::new(stream, || latch.send(()).unwrap());
                let _ = sender.send(Ok(ReadFileResult::Ok(Box::pin(drop_stream))));
                latch_receiver.await.unwrap();
            }
            other => {
                let _ = sender.send(other);
            }
        };
    }

    /// Records an attempted worker update as failed
    async fn fail_update(&self, target_version: ComponentVersion, error: String) -> CommandOutcome {
        self.store
            .data()
            .on_worker_update_failed(target_version, Some(error))
            .await;
        CommandOutcome::Continue
    }

    /// Attempts to interpret the save snapshot result as a byte vector
    fn decode_snapshot_result(value: Option<Value>) -> Option<Vec<u8>> {
        if let Some(Value::List(bytes)) = value {
            let mut result = Vec::new();
            for value in bytes {
                if let Value::U8(byte) = value {
                    result.push(byte);
                } else {
                    return None;
                }
            }
            Some(result)
        } else {
            None
        }
    }

    /// Extends the invocation context with a new span containing information about the invocation
    fn extend_invocation_context(
        invocation_context: &mut InvocationContextStack,
        idempotency_key: &IdempotencyKey,
        full_function_name: &str,
        worker_id: &WorkerId,
        agent_id: &Option<AgentId>,
    ) {
        let invocation_span = invocation_context.spans.first().start_span(None);
        invocation_span.set_attribute(
            "name".to_string(),
            AttributeValue::String("invoke-exported-function".to_string()),
        );
        invocation_span.set_attribute(
            "idempotency_key".to_string(),
            AttributeValue::String(idempotency_key.to_string()),
        );
        invocation_span.set_attribute(
            "function_name".to_string(),
            AttributeValue::String(full_function_name.to_string()),
        );
        invocation_span.set_attribute(
            "worker_id".to_string(),
            AttributeValue::String(worker_id.to_string()),
        );
        if let Some(agent_id) = agent_id {
            invocation_span.set_attribute(
                "agent_type".to_string(),
                AttributeValue::String(agent_id.agent_type.clone()),
            );
            invocation_span.set_attribute(
                "agent_parameters".to_string(),
                AttributeValue::String(agent_id.parameters.to_string()),
            )
        }
        invocation_context.push(invocation_span);
    }
}

/// Outcome of processing a single command within the inner invocation loop
#[derive(Debug)]
enum CommandOutcome {
    /// Break from both the inner and outer loops, there is no way to retry anything
    BreakOuterLoop,
    /// Break from the inner loop, setting the retry decision for the outer loop
    BreakInnerLoop(RetryDecision),
    /// Continue processing in the inner loop
    Continue,
}
