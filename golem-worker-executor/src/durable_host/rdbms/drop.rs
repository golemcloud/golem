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

use super::*;
use crate::durable_host::TransactionProtocol;
use crate::durable_host::concurrent::{
    Resolution, ResolutionOutcome, continue_live_at_natural_tail_end_access,
};
use crate::durable_host::durability::{
    DurableCallTrapContext, TaskRetryContext, mark_durable_call_trap_context,
    try_trigger_host_trap_retry,
};
use crate::durable_host::tail_work::TailActivity;
use crate::services::oplog::OplogOps;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal;
use golem_common::model::oplog::{
    HostResponse, HostResponseGolemRdbmsTransactionTerminal, OplogEntry,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::{Accessor, HasData};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
enum TransactionDropResolution {
    Completed,
    Incomplete,
}

type DropResolution =
    Shared<BoxFuture<'static, Result<TransactionDropResolution, WorkerExecutorError>>>;

#[derive(Clone, Debug)]
struct SharedDropError(Arc<anyhow::Error>);

impl SharedDropError {
    fn new(begin: OplogIndex, error: anyhow::Error) -> Self {
        Self(Arc::new(mark_durable_call_trap_context(
            error,
            DurableCallTrapContext { retry_from: begin },
        )))
    }
}

impl std::fmt::Display for SharedDropError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for SharedDropError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

type DropCompletion = Shared<BoxFuture<'static, Result<(), SharedDropError>>>;

fn validate_drop_response(response: HostResponse) -> Result<(), WorkerExecutorError> {
    if matches!(
        response,
        HostResponse::GolemRdbmsTransactionTerminal(HostResponseGolemRdbmsTransactionTerminal {
            outcome: SerializableRdbmsTransactionTerminal::Drop(_),
        })
    ) {
        Ok(())
    } else {
        Err(WorkerExecutorError::runtime(
            "Transaction terminal does not match recorded Drop outcome",
        ))
    }
}

struct PendingTransactionDrop<T: RdbmsType + 'static> {
    tx: RdbmsTransactionEntry<T>,
    protocol: Option<TransactionProtocol>,
    resolution: DropResolution,
    completion: Option<DropCompletion>,
    resource_rep: u32,
}

pub(super) async fn cleanup_pending<Ctx, T>(ctx: &mut DurableWorkerCtx<Ctx>) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    let mut pending: Vec<_> = ctx
        .table()
        .iter_mut()
        .filter_map(|(_, entry)| {
            entry
                .downcast_ref::<PendingTransactionDrop<T>>()
                .map(|entry| {
                    (
                        entry.tx.begin_index,
                        entry.resource_rep,
                        entry.resolution.clone(),
                        entry.completion.clone(),
                    )
                })
        })
        .collect();
    pending.sort_by_key(|(begin, _, _, _)| *begin);
    for (begin, rep, resolution, completion) in pending {
        if let Some(completion) = completion {
            completion.await?;
            ctx.table()
                .delete(Resource::<PendingTransactionDrop<T>>::new_own(rep))?;
            continue;
        }
        match resolution
            .await
            .map_err(|error| SharedDropError::new(begin, error.into()))?
        {
            TransactionDropResolution::Completed => {
                ctx.table()
                    .delete(Resource::<PendingTransactionDrop<T>>::new_own(rep))?;
            }
            TransactionDropResolution::Incomplete => {
                let tx = ctx
                    .table()
                    .get(&Resource::<PendingTransactionDrop<T>>::new_borrow(rep))?
                    .tx
                    .clone();
                if !tx.persisted {
                    if let RdbmsTransactionState::Open(transaction) = &tx.state
                        && transaction.pre_rollback().await.is_ok()
                    {
                        let _ = transaction.rollback().await;
                    }
                    let _ = ctx
                        .state
                        .rdbms_service
                        .rdbms_type_service()
                        .cleanup_transaction(
                            &tx.pool_key,
                            &ctx.state.owned_agent_id.agent_id,
                            &tx.transaction_id(),
                        )
                        .await;
                    ctx.table()
                        .delete(Resource::<PendingTransactionDrop<T>>::new_own(rep))?;
                    continue;
                }
                // This is retained entity cleanup after physical Store disposal, not a
                // guest cleanup drain. Replayed resolutions report Incomplete only at the
                // shared cursor's natural tail; live drops already started at that tail.
                // Revalidate it before publishing live, without skipping sibling history.
                ctx.state
                    .replay_state
                    .await_natural_tail_end(None)
                    .await
                    .map_err(|error| SharedDropError::new(begin, error.into()))?;
                if !ctx.is_live() {
                    use crate::durable_host::concurrent::DurableContext;
                    ctx.publish_direct_replay_tail(begin)
                        .await
                        .map_err(|error| SharedDropError::new(begin, error.into()))?;
                }
                let pending = ctx
                    .table()
                    .delete(Resource::<PendingTransactionDrop<T>>::new_own(rep))?;
                let protocol = pending
                    .protocol
                    .ok_or_else(|| anyhow!("Incomplete transaction Drop has no protocol"))?;
                ctx.state.transaction_protocols.insert(begin, protocol);
                let resource = ctx.table().push(pending.tx)?;
                ctx.table().get_mut(&resource)?.resource_rep = Some(resource.rep());
                db_transaction_drop(ctx, resource)
                    .await
                    .map_err(|error| SharedDropError::new(begin, error))?;
            }
        }
    }
    Ok(())
}

pub(super) async fn detach_recorded_drop<Ctx: WorkerCtx, T: RdbmsType + 'static>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    tx: &mut RdbmsTransactionEntry<T>,
) -> anyhow::Result<()> {
    if !tx.persisted || !matches!(tx.state, RdbmsTransactionState::Reconstructed(_)) {
        return Ok(());
    }
    let Some(OplogEntry::End {
        response: Some(response),
        ..
    }) = ctx
        .state
        .replay_state
        .visible_terminal_entry(tx.begin_index)
        .await?
    else {
        return Ok(());
    };
    let response = ctx
        .state
        .oplog
        .download_payload(response)
        .await
        .map_err(anyhow::Error::msg)?;
    if matches!(
        response,
        HostResponse::GolemRdbmsTransactionTerminal(HostResponseGolemRdbmsTransactionTerminal {
            outcome: SerializableRdbmsTransactionTerminal::Drop(_),
        })
    ) {
        let mut protocol = ctx
            .state
            .transaction_protocols
            .remove(&tx.begin_index)
            .ok_or_else(|| anyhow!("Transaction protocol is not open"))?;
        let replay = protocol
            .replay_end
            .take()
            .ok_or_else(|| anyhow!("Recorded transaction Drop has no replay handle"))?;
        // The recorded rollback has no remaining live effect. Keep its resolver receiver so
        // cursor progress drains the End even if GC never drops this reconstructed resource.
        // Release the checkpoint blocker now: replay itself already prevents snapshots until
        // the cursor consumes that End. A later destructor still validates the actual terminal.
        tx.replay_drop = Some(Arc::new(std::sync::Mutex::new(Some(replay))));
    }
    Ok(())
}

pub(super) async fn drop_transaction<U, D, Ctx, T>(
    store: &Accessor<U, D>,
    get_ctx: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
    resource: Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<()>
where
    U: Send + 'static,
    D: HasData + ?Sized,
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    let (tx, protocol, suppressed, activity) = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        ctx.observe_function_call(T::durability_transaction_interface(), "drop");
        let tx = ctx.table().delete(resource)?;
        let protocol = if tx.persisted {
            ctx.state.transaction_protocols.remove(&tx.begin_index)
        } else {
            None
        };
        let suppressed = !tx.persisted || ctx.state.durability_is_suppressed();
        Ok::<_, anyhow::Error>((tx, protocol, suppressed, ctx.tail_work_tracker().activity()))
    })?;
    if matches!(tx.state, RdbmsTransactionState::Closed(_)) {
        return Ok(());
    }
    let recorded_drop = tx.replay_drop.is_some();
    let detached_replay = tx
        .replay_drop
        .as_ref()
        .and_then(|replay| replay.lock().unwrap().take());
    anyhow::ensure!(
        suppressed || protocol.is_some() || detached_replay.is_some(),
        "Transaction protocol is not open"
    );
    let mut protocol = protocol;
    let replay = protocol
        .as_mut()
        .and_then(|protocol| protocol.replay_end.take())
        .or(detached_replay);
    let replay = if let Some(handle) = replay {
        let resolution = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            let replay_state = ctx.state.replay_state.clone();
            let oplog = ctx.state.oplog.clone();
            let resolution_activity = ctx.tail_work_tracker().activity();
            async move {
                if !recorded_drop
                    && let Some(terminal) = replay_state
                        .visible_terminal_entry(handle.start_idx())
                        .await?
                {
                    let OplogEntry::End {
                        response: Some(response),
                        ..
                    } = terminal
                    else {
                        return Err(WorkerExecutorError::runtime(
                            "Invalid transaction scope terminal",
                        ));
                    };
                    let response = oplog
                        .download_payload(response)
                        .await
                        .map_err(WorkerExecutorError::runtime)?;
                    validate_drop_response(response)?;
                }
                match replay_state
                    .await_resolution_outcome_for_drop(handle, Some(&resolution_activity))
                    .await?
                {
                    ResolutionOutcome::Resolved(Resolution::Completed {
                        response: Some(response),
                        ..
                    }) => {
                        let response = oplog
                            .download_payload(response)
                            .await
                            .map_err(WorkerExecutorError::runtime)?;
                        validate_drop_response(response)?;
                        Ok(TransactionDropResolution::Completed)
                    }
                    ResolutionOutcome::Incomplete if !recorded_drop => {
                        Ok(TransactionDropResolution::Incomplete)
                    }
                    _ => Err(WorkerExecutorError::runtime(
                        "Invalid transaction scope terminal",
                    )),
                }
            }
            .boxed()
            .shared()
        });
        Some(resolution)
    } else {
        None
    };
    // Physical Store destruction cancels accessor tasks, but retains the context's table.
    // Keep the resolver, protocol and any started effect there until settlement finishes.
    let retained = store.with(|mut access| {
        let ctx = get_ctx(access.data_mut());
        let resource = ctx.table().push(PendingTransactionDrop {
            tx: tx.clone(),
            protocol,
            resolution: replay.clone().unwrap_or_else(|| {
                futures::future::ready(Ok(TransactionDropResolution::Incomplete))
                    .boxed()
                    .shared()
            }),
            completion: None,
            resource_rep: 0,
        })?;
        ctx.table().get_mut(&resource)?.resource_rep = resource.rep();
        Ok::<_, anyhow::Error>(resource)
    })?;
    let task = TransactionDrop {
        tx,
        replay,
        retained,
        suppressed,
        get_ctx,
        activity,
    };
    if task.replay.is_some() {
        // GC can release the handle before later guest calls that precede its recorded End.
        let begin = task.tx.begin_index;
        store
            .spawn(async move |store| task.run(store).await)
            .map_err(|error| {
                mark_durable_call_trap_context(
                    error.into(),
                    DurableCallTrapContext { retry_from: begin },
                )
            })?;
        Ok(())
    } else {
        let begin = task.tx.begin_index;
        task.finish(store).await.map_err(|error| {
            mark_durable_call_trap_context(error, DurableCallTrapContext { retry_from: begin })
        })
    }
}

struct TransactionDrop<U, Ctx: WorkerCtx, T: RdbmsType + 'static> {
    tx: RdbmsTransactionEntry<T>,
    replay: Option<DropResolution>,
    retained: Resource<PendingTransactionDrop<T>>,
    suppressed: bool,
    get_ctx: fn(&mut U) -> &mut DurableWorkerCtx<Ctx>,
    activity: TailActivity,
}

impl<U, Ctx, T> TransactionDrop<U, Ctx, T>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    async fn run<D: HasData + ?Sized>(self, store: &Accessor<U, D>) -> wasmtime::Result<()> {
        let begin = self.tx.begin_index;
        self.finish(store)
            .await
            .map_err(|error| {
                mark_durable_call_trap_context(error, DurableCallTrapContext { retry_from: begin })
            })
            .map_err(wasmtime::Error::from_anyhow)
    }
}

impl<U, Ctx, T> TransactionDrop<U, Ctx, T>
where
    U: Send + 'static,
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    async fn finish<D: HasData + ?Sized>(self, store: &Accessor<U, D>) -> anyhow::Result<()> {
        let Self {
            tx,
            replay,
            retained,
            suppressed,
            get_ctx,
            activity,
        } = self;
        let begin = tx.begin_index;
        let (replay_state, oplog, worker, service, agent_id) = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            (
                ctx.state.replay_state.clone(),
                ctx.state.oplog.clone(),
                ctx.public_state.worker(),
                ctx.state.rdbms_service.clone(),
                ctx.state.owned_agent_id.agent_id.clone(),
            )
        });
        if let Some(resolution) = replay {
            match activity.park(resolution).await? {
                TransactionDropResolution::Completed => {
                    store.with(|mut access| get_ctx(access.data_mut()).table().delete(retained))?;
                    return Ok(());
                }
                TransactionDropResolution::Incomplete => {
                    // An absent terminal is not permission to discard intervening history.
                    continue_live_at_natural_tail_end_access(
                        store,
                        get_ctx,
                        &replay_state,
                        &activity,
                        "dropped RDBMS transaction",
                        Some(golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal::AtRecordedOplogEntry {
                            index: begin.into(),
                        }),
                    )
                    .await?;
                }
            }
        }
        let confirmed = replay_state
            .transaction_marker(begin, |e| e.is_rolled_back_remote_transaction(begin))
            .is_some();
        let prepared = replay_state
            .transaction_marker(begin, |e| e.is_pre_rollback_remote_transaction(begin))
            .is_some();
        anyhow::ensure!(
            suppressed || store.with(|mut access| get_ctx(access.data_mut()).is_live()),
            "Transaction rollback before live publication"
        );
        let mut retry = store.with(|mut access| {
            let ctx = get_ctx(access.data_mut());
            // The task may finish during a later guest invocation. Its retry point belongs
            // to this task and SharedDropError, not the guest's ambient retry point.
            TaskRetryContext {
                retry_point: begin,
                entity_parent_start_index: ctx.entity_parent_start_index(),
                environment_state_service: ctx.state.environment_state_service.clone(),
                environment_id: ctx.state.owned_agent_id.environment_id,
                default_retry_policy: golem_common::model::NamedRetryPolicy::default_from_config(
                    &ctx.state.config.retry,
                ),
                agent_config_retry_policies: ctx.state.agent_config_retry_policies(),
                runtime_retry_policy_mutations: ctx.state.runtime_retry_policy_mutations.clone(),
                max_in_function_retry_delay: ctx.state.config.max_in_function_retry_delay,
                current_retry_policy_state: None,
                retry_properties: rdbms_retry_properties(ctx, "transaction-status", &tx.pool_key),
                worker: worker.clone(),
            }
        });
        let checkpoint_worker = worker.clone();
        let completion = async move {
            let result = match &tx.state {
                RdbmsTransactionState::Open(transaction) => {
                    match transaction.pre_rollback().await {
                        Err(error) => Err(error),
                        Ok(()) => {
                            if !suppressed {
                                oplog
                                    .add(OplogEntry::pre_rollback_remote_transaction(begin))
                                    .await?;
                                worker
                                    .commit_oplog_and_update_state(CommitLevel::Always)
                                    .await?;
                            }
                            transaction.rollback().await
                        }
                    }
                }
                RdbmsTransactionState::Reconstructed(_) if confirmed => Ok(()),
                RdbmsTransactionState::Reconstructed(id) if prepared => {
                    let status = service
                        .rdbms_type_service()
                        .get_transaction_status(&tx.pool_key, &agent_id, id)
                        .await;
                    match status {
                        Ok(
                            RdbmsTransactionStatus::RolledBack | RdbmsTransactionStatus::NotFound,
                        ) => Ok(()),
                        Ok(RdbmsTransactionStatus::Committed) => Err(rdbms_transaction_lost()),
                        pending => {
                            let error = pending.err().unwrap_or_else(|| {
                                RdbmsError::ConnectionFailure(
                                    "Database transaction outcome is still pending".to_string(),
                                )
                            });
                            retry.current_retry_policy_state = worker
                                .get_last_known_status()
                                .await
                                .current_retry_state
                                .get(&begin)
                                .cloned();
                            let properties = retry.retry_properties.clone();
                            try_trigger_host_trap_retry(
                                &mut retry,
                                anyhow!(error.clone()),
                                properties,
                            )
                            .await?;
                            return Err(anyhow!(ClassifiedHostError {
                                kind: HostFailureKind::Permanent,
                                message: format!(
                                    "transaction status resolution exhausted at Start {begin}: {error}"
                                )
                            }));
                        }
                    }
                }
                RdbmsTransactionState::Reconstructed(_) => Err(rdbms_transaction_lost()),
                RdbmsTransactionState::Closed(_) => unreachable!(),
            };
            if !suppressed {
                crate::durable_host::calls::persist_transaction_terminal(
                    oplog.as_ref(),
                    worker.as_ref(),
                    begin,
                    SerializableRdbmsTransactionTerminal::Drop(result.clone().map_err(Into::into)),
                    (result.is_ok() && !confirmed)
                        .then(|| OplogEntry::rolled_back_remote_transaction(begin)),
                )
                .await?;
            }
            let _ = service
                .rdbms_type_service()
                .cleanup_transaction(&tx.pool_key, &agent_id, &tx.transaction_id())
                .await;
            Ok::<_, anyhow::Error>(())
        };
        let completion = completion
            .map(move |result| result.map_err(|error| SharedDropError::new(begin, error)))
            .boxed()
            .shared();
        store.with(|mut access| {
            get_ctx(access.data_mut())
                .table()
                .get_mut(&retained)
                .map(|pending| pending.completion = Some(completion.clone()))
        })?;
        completion.await?;
        store.with(|mut access| get_ctx(access.data_mut()).table().delete(retained))?;
        if !suppressed
            && store.with(|mut access| {
                get_ctx(access.data_mut())
                    .state
                    .at_clean_checkpoint_boundary()
            })
        {
            checkpoint_worker.checkpoint_status_mid_invocation().await;
        }
        Ok(())
    }
}
