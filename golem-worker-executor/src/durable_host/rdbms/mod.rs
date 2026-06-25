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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::{HostFailureKind, InFunctionRetryHost};
use crate::durable_host::rdbms::serialized::RdbmsRequest;
use crate::durable_host::{
    DurabilityHost, DurableWorkerCtx, InternalRetryResult, RemoteTransactionHandler,
};
use crate::services::rdbms::{DbResult, DbRow, RdbmsType};
use crate::services::rdbms::{RdbmsError, RdbmsService, RdbmsTransactionStatus, RdbmsTypeService};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::oplog::HostPayloadPair;
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRdbmsRequest, HostRequestNoInput,
    HostResponseGolemRdbmsColumns, HostResponseGolemRdbmsRequest, HostResponseGolemRdbmsResult,
    HostResponseGolemRdbmsResultChunk, HostResponseGolemRdbmsRowCount,
};
use golem_common::model::retry_policy::RetryProperties;
use golem_common::model::{AgentId, OplogIndex, RdbmsPoolKey, RetryContext, TransactionId};
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::IoView;

pub mod ignite;
pub mod mysql;
pub mod postgres;
pub mod serialized;
pub mod types;

fn classify_rdbms_error(error: &RdbmsError) -> HostFailureKind {
    match error {
        RdbmsError::ConnectionFailure(_) => HostFailureKind::Transient,
        RdbmsError::QueryExecutionFailure(_) => HostFailureKind::Transient,
        RdbmsError::QueryParameterFailure(_) => HostFailureKind::Permanent,
        RdbmsError::QueryResponseFailure(_) => HostFailureKind::Permanent,
        RdbmsError::Other(_) => HostFailureKind::Transient,
    }
}

/// Builds the retry property bag for an RDBMS host call, populating `verb`,
/// `noun-uri`, `db-type` (e.g. `postgres`, `mysql`, `ignite2`), and the worker-level
/// enrichment so user-defined policies keyed on `db-type` can match.
fn rdbms_retry_properties<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    verb: &str,
    pool_key: &RdbmsPoolKey,
) -> RetryProperties {
    let mut properties = RetryContext::rdbms(verb, pool_key);
    ctx.state.enrich_retry_properties(&mut properties);
    properties
}

// Trait to map RdbmsType to the correct HostPayloadPair types for durability
pub trait RdbmsDurabilityPairs {
    type ConnExecute: HostPayloadPair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsRowCount>;
    type ConnQuery: HostPayloadPair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsResult>;
    type ConnQueryStream: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsRequest>;
    type TxnExecute: HostPayloadPair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsRowCount>;
    type TxnQuery: HostPayloadPair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsResult>;
    type TxnQueryStream: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsRequest>;
    type StreamGetColumns: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsColumns>;
    type StreamGetNext: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsResultChunk>;
}

/// A transaction request prepared up front (before its side effect): the pool key, the open
/// transaction handle, and the converted parameters, or the error that prevented preparing it.
type PreparedTransactionRequest<T> = Result<
    (
        RdbmsPoolKey,
        Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>,
        Vec<<T as RdbmsType>::DbValue>,
    ),
    RdbmsError,
>;

async fn open_db_connection<Ctx, T, E>(
    address: String,
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<Result<Resource<RdbmsConnection<T>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    E: From<RdbmsError>,
{
    ctx.observe_function_call(T::durability_connection_interface(), "open");

    let agent_id = ctx.state.owned_agent_id.agent_id.clone();
    let result = ctx
        .state
        .rdbms_service
        .rdbms_type_service()
        .create(&address, &agent_id)
        .await;

    match result {
        Ok(key) => {
            let entry = RdbmsConnection::new(key);
            let resource = ctx.as_wasi_view().table().push(entry)?;
            Ok(Ok(resource))
        }
        Err(error) => Ok(Err(error.into())),
    }
}

async fn begin_db_transaction<Ctx, T, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<Resource<RdbmsTransactionEntry<T>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Send + Sync + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    E: From<RdbmsError>,
{
    ctx.observe_function_call(T::durability_connection_interface(), "begin-transaction");

    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)?
        .pool_key
        .clone();

    let result = ctx
        .begin_transaction_function(RdbmsRemoteTransactionHandler::<T>::new(
            pool_key.clone(),
            ctx.state.owned_agent_id.agent_id.clone(),
            ctx.state.rdbms_service.clone(),
        ))
        .await;

    match result {
        Ok((begin_oplog_idx, transaction_state)) => {
            let entry = RdbmsTransactionEntry::new(pool_key, transaction_state, begin_oplog_idx);
            let resource = ctx.as_wasi_view().table().push(entry)?;
            Ok(Ok(resource))
        }
        Err(error) => Ok(Err(error.into())),
    }
}

async fn db_connection_durable_execute<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<u64, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let agent_id = ctx.state.owned_agent_id.agent_id.clone();
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)
        .map(|v| v.pool_key.clone());

    // The recorded request payload (pool key + statement + parameters) is derived from live,
    // side-effect-free inputs (`to_db_values` only reads the resource table), so it is computed
    // up front for the eager host-call `Start`. The actual SQL execution happens only on the live
    // / incomplete-replay completion path below.
    let prepared: Result<(RdbmsPoolKey, Vec<T::DbValue>), RdbmsError> = match pool_key {
        Ok(pool_key) => match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(db_params) => Ok((pool_key, db_params)),
            Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
        },
        Err(error) => Err(RdbmsError::other_response_failure(error)),
    };

    let request = HostRequestGolemRdbmsRequest {
        request: prepared.as_ref().ok().map(|(pool_key, db_params)| {
            RdbmsRequest::<T>::new(pool_key.clone(), statement.clone(), db_params.clone(), None)
                .into()
        }),
    };

    let mut handle = CallHandle::<T::ConnExecute, NotCancellable>::start(
        ctx,
        request,
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        let result = match &prepared {
            Ok((pool_key, db_params)) => {
                let properties = rdbms_retry_properties(ctx, "execute", pool_key);
                loop {
                    let result = ctx
                        .state
                        .rdbms_service
                        .deref()
                        .rdbms_type_service()
                        .execute(pool_key, &agent_id, &statement, db_params.clone())
                        .await;
                    match handle
                        .try_trigger_retry_or_loop_with_properties(
                            ctx,
                            &result,
                            classify_rdbms_error,
                            properties.clone(),
                        )
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            }
            Err(error) => Err(error.clone()),
        };

        let result = result.map_err(|e| e.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsRowCount { result })
            .await?
    };

    Ok(result.result.map_err(|e| RdbmsError::from(e).into()))
}

async fn db_connection_durable_query<Ctx, T, P, R, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<R, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    R: FromRdbmsValue<crate::services::rdbms::DbResult<T>>,
    E: From<RdbmsError>,
{
    let agent_id = ctx.state.owned_agent_id.agent_id.clone();
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)
        .map(|v| v.pool_key.clone());

    // See `db_connection_durable_execute`: the request payload is derived from side-effect-free
    // live inputs, so it is computed up front for the eager host-call `Start`; the query itself
    // runs only on the live / incomplete-replay completion path below.
    let prepared: Result<(RdbmsPoolKey, Vec<T::DbValue>), RdbmsError> = match pool_key {
        Ok(pool_key) => match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(db_params) => Ok((pool_key, db_params)),
            Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
        },
        Err(error) => Err(RdbmsError::other_response_failure(error)),
    };

    let request = HostRequestGolemRdbmsRequest {
        request: prepared.as_ref().ok().map(|(pool_key, db_params)| {
            RdbmsRequest::<T>::new(pool_key.clone(), statement.clone(), db_params.clone(), None)
                .into()
        }),
    };

    let mut handle = CallHandle::<T::ConnQuery, NotCancellable>::start(
        ctx,
        request,
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                CallReplayOutcome::Incomplete(live) => handle = live,
            }
        }

        let result = match &prepared {
            Ok((pool_key, db_params)) => {
                let properties = rdbms_retry_properties(ctx, "query", pool_key);
                loop {
                    let result = ctx
                        .state
                        .rdbms_service
                        .deref()
                        .rdbms_type_service()
                        .query(pool_key, &agent_id, &statement, db_params.clone())
                        .await;
                    match handle
                        .try_trigger_retry_or_loop_with_properties(
                            ctx,
                            &result,
                            classify_rdbms_error,
                            properties.clone(),
                        )
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            }
            Err(error) => Err(error.clone()),
        };

        let result = result.map(|result| result.into()).map_err(|e| e.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsResult { result })
            .await?
    };

    match result.result {
        Ok(result) => {
            let result: DbResult<T> = result
                .try_into()
                .map_err(|err| anyhow!("Invalid payload: {err}"))?;
            let result = FromRdbmsValue::from(result, ctx.as_wasi_view().table())
                .map_err(|e| RdbmsError::QueryResponseFailure(e).into());
            Ok(result)
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
    }
}

async fn db_connection_durable_query_stream<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<Resource<RdbmsResultStreamEntry<T>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let begin_index = ctx
        .begin_durable_function(
            &DurableFunctionType::WriteRemoteBatched(None),
            "golem::rdbms::db-connection::query-stream",
        )
        .await?;
    let mut handle = CallHandle::<T::ConnQueryStream, NotCancellable>::start(
        ctx,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemoteBatched(Some(begin_index)),
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let conn_pool_key = ctx
            .as_wasi_view()
            .table()
            .get::<RdbmsConnection<T>>(entry)
            .map(|v| v.pool_key.clone());
        let result = db_connection_query_stream(statement, params, ctx, entry);
        let pool_key_for_props = result
            .as_ref()
            .map(|r| r.pool_key.clone())
            .ok()
            .or_else(|| conn_pool_key.ok());
        if let Some(pool_key) = pool_key_for_props.as_ref() {
            let properties = rdbms_retry_properties(ctx, "query-stream", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result.map(|request| request.into()).map_err(|e| e.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsRequest { request: result })
            .await?
    };
    match result.request {
        Ok(request) => {
            let entry = RdbmsResultStreamEntry::new(
                request
                    .try_into()
                    .map_err(|err| anyhow!("Invalid payload: {err}"))?,
                RdbmsResultStreamState::New,
                None,
                begin_index,
            );
            let resource = ctx.as_wasi_view().table().push(entry)?;
            Ok(Ok(resource))
        }
        Err(error) => {
            ctx.end_durable_function(
                &DurableFunctionType::WriteRemoteBatched(None),
                begin_index,
                false,
            )
            .await?;

            Ok(Err(RdbmsError::from(error).into()))
        }
    }
}

async fn db_connection_drop<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: Resource<RdbmsConnection<T>>,
) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    ctx.observe_function_call(T::durability_connection_interface(), "drop");
    let agent_id = ctx.state.owned_agent_id.agent_id.clone();
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(&entry)?
        .pool_key
        .clone();

    let _ = ctx
        .state
        .rdbms_service
        .rdbms_type_service()
        .remove(&pool_key, &agent_id)
        .await;

    ctx.as_wasi_view()
        .table()
        .delete::<RdbmsConnection<T>>(entry)?;
    Ok(())
}

async fn db_result_stream_durable_get_columns<Ctx, T, R>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<Vec<R>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<T::DbColumn>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let durable_function_type = if is_db_query_stream_in_transaction(ctx, entry)? {
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx))
    } else {
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx))
    };

    let mut handle = CallHandle::<T::StreamGetColumns, NotCancellable>::start(
        ctx,
        HostRequestNoInput {},
        durable_function_type,
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let pool_key = ctx
            .as_wasi_view()
            .table()
            .get::<RdbmsResultStreamEntry<T>>(entry)
            .map(|v| v.request.pool_key.clone());
        let query_stream = get_db_query_stream(ctx, entry).await;
        let result = match query_stream {
            Ok(query_stream) => query_stream.deref().get_columns().await,
            Err(error) => Err(error),
        };
        if let Ok(pool_key) = pool_key.as_ref() {
            let properties = rdbms_retry_properties(ctx, "stream-get-columns", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result
            .map(|columns| columns.into_iter().map(|c| c.into()).collect())
            .map_err(|err| err.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsColumns { result })
            .await?
    };

    match result.result {
        Ok(columns) => {
            let columns = columns
                .into_iter()
                .map(|c| c.try_into())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| anyhow!("Invalid payload: {err}"))?;
            let result = columns
                .into_iter()
                .map(|r| FromRdbmsValue::from(r, ctx.as_wasi_view().table()))
                .collect::<Result<Vec<R>, String>>()
                .map_err(|e| anyhow!(RdbmsError::QueryResponseFailure(e)))?;
            Ok(result)
        }
        Err(error) => Err(anyhow!(RdbmsError::from(error))),
    }
}

async fn db_result_stream_durable_get_next<Ctx, T, R>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<Option<Vec<R>>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<crate::services::rdbms::DbRow<T::DbValue>>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let durable_function_type = if is_db_query_stream_in_transaction(ctx, entry)? {
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx))
    } else {
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx))
    };

    let mut handle = CallHandle::<T::StreamGetNext, NotCancellable>::start(
        ctx,
        HostRequestNoInput {},
        durable_function_type,
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let pool_key = ctx
            .as_wasi_view()
            .table()
            .get::<RdbmsResultStreamEntry<T>>(entry)
            .map(|v| v.request.pool_key.clone());
        let query_stream = get_db_query_stream(ctx, entry).await;
        let result = match query_stream {
            Ok(query_stream) => query_stream.deref().get_next().await,
            Err(error) => Err(error),
        };
        if let Ok(pool_key) = pool_key.as_ref() {
            let properties = rdbms_retry_properties(ctx, "stream-get-next", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result
            .map(|chunk| {
                chunk.map(|rows| {
                    rows.into_iter()
                        .map(|row| row.values.into_iter().map(|v| v.into()).collect())
                        .collect()
                })
            })
            .map_err(|err| err.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsResultChunk { result })
            .await?
    };

    match result.result {
        Ok(rows) => {
            let rows = match rows {
                Some(rows) => {
                    let result = rows
                        .into_iter()
                        .map(|serialized_values| {
                            let row = DbRow {
                                values: serialized_values
                                    .into_iter()
                                    .map(|v| v.try_into())
                                    .collect::<Result<Vec<_>, _>>()?,
                            };
                            FromRdbmsValue::from(row, ctx.as_wasi_view().table())
                        })
                        .collect::<Result<Vec<R>, String>>()
                        .map_err(|e| anyhow!(RdbmsError::QueryResponseFailure(e)))?;
                    Some(result)
                }
                None => None,
            };
            Ok(rows)
        }
        Err(error) => Err(anyhow!(RdbmsError::from(error))),
    }
}

async fn db_result_stream_drop<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    ctx.observe_function_call(T::durability_result_stream_interface(), "drop");

    let entry = ctx
        .as_wasi_view()
        .table()
        .delete::<RdbmsResultStreamEntry<T>>(entry)?;

    if entry.transaction_handle.is_none() {
        ctx.end_durable_function(
            &DurableFunctionType::WriteRemoteBatched(None),
            entry.begin_index,
            false,
        )
        .await?;
    }

    Ok(())
}

async fn db_transaction_durable_query<Ctx, T, P, R, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<R, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    R: FromRdbmsValue<crate::services::rdbms::DbResult<T>>,
    E: From<RdbmsError>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let txn_pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|v| v.pool_key.clone())
        .ok();

    // Build the request payload (pool key + statement + parameters + transaction id) up front,
    // before the query side effect, mirroring `db_transaction_query`'s request construction. The
    // transaction id is read from the open transaction handle without executing the query.
    let prepared: PreparedTransactionRequest<T> =
        match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(db_params) => match get_db_transaction(ctx, entry) {
                Ok((pool_key, transaction)) => Ok((pool_key, transaction, db_params)),
                Err(error) => Err(error),
            },
            Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
        };

    let request = HostRequestGolemRdbmsRequest {
        request: prepared
            .as_ref()
            .ok()
            .map(|(pool_key, transaction, db_params)| {
                RdbmsRequest::<T>::new(
                    pool_key.clone(),
                    statement.clone(),
                    db_params.clone(),
                    Some(transaction.transaction_id()),
                )
                .into()
            }),
    };

    let mut handle = CallHandle::<T::TxnQuery, NotCancellable>::start(
        ctx,
        request,
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let result = match &prepared {
            Ok((_pool_key, transaction, db_params)) => {
                transaction.query(&statement, db_params.clone()).await
            }
            Err(error) => Err(error.clone()),
        };
        let pool_key_for_props = prepared
            .as_ref()
            .ok()
            .map(|(pool_key, _, _)| pool_key.clone())
            .or_else(|| txn_pool_key.clone());
        if let Some(pool_key) = pool_key_for_props.as_ref() {
            let properties = rdbms_retry_properties(ctx, "query", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result.map(|result| result.into()).map_err(|err| err.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsResult { result })
            .await?
    };

    match result.result {
        Ok(result) => {
            let result = result
                .try_into()
                .map_err(|err| anyhow!("Invalid payload: {err}"))?;
            let result = FromRdbmsValue::from(result, ctx.as_wasi_view().table())
                .map_err(|e| RdbmsError::QueryResponseFailure(e).into());
            Ok(result)
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
    }
}

async fn db_transaction_durable_execute<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<u64, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let txn_pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|v| v.pool_key.clone())
        .ok();

    // Build the request payload up front, before the execute side effect, mirroring
    // `db_transaction_execute`'s request construction (transaction id read without executing).
    let prepared: PreparedTransactionRequest<T> =
        match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(db_params) => match get_db_transaction(ctx, entry) {
                Ok((pool_key, transaction)) => Ok((pool_key, transaction, db_params)),
                Err(error) => Err(error),
            },
            Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
        };

    let request = HostRequestGolemRdbmsRequest {
        request: prepared
            .as_ref()
            .ok()
            .map(|(pool_key, transaction, db_params)| {
                RdbmsRequest::<T>::new(
                    pool_key.clone(),
                    statement.clone(),
                    db_params.clone(),
                    Some(transaction.transaction_id()),
                )
                .into()
            }),
    };

    let mut handle = CallHandle::<T::TxnExecute, NotCancellable>::start(
        ctx,
        request,
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let result = match &prepared {
            Ok((_pool_key, transaction, db_params)) => {
                transaction.execute(&statement, db_params.clone()).await
            }
            Err(error) => Err(error.clone()),
        };
        let pool_key_for_props = prepared
            .as_ref()
            .ok()
            .map(|(pool_key, _, _)| pool_key.clone())
            .or_else(|| txn_pool_key.clone());
        if let Some(pool_key) = pool_key_for_props.as_ref() {
            let properties = rdbms_retry_properties(ctx, "execute", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result.map_err(|e| e.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsRowCount { result })
            .await?
    };

    Ok(result.result.map_err(|e| RdbmsError::from(e).into()))
}

async fn db_transaction_durable_query_stream<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<Resource<RdbmsResultStreamEntry<T>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let entry_rep = entry.rep();
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;
    let mut handle = CallHandle::<T::TxnQueryStream, NotCancellable>::start(
        ctx,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle.replay_expecting_completion(ctx).await?;
        }

        let txn_pool_key = ctx
            .as_wasi_view()
            .table()
            .get::<RdbmsTransactionEntry<T>>(entry)
            .map(|v| v.pool_key.clone());
        let result = db_transaction_query_stream(statement, params, ctx, entry);
        let pool_key_for_props = result
            .as_ref()
            .map(|r| r.pool_key.clone())
            .ok()
            .or_else(|| txn_pool_key.ok());
        if let Some(pool_key) = pool_key_for_props.as_ref() {
            let properties = rdbms_retry_properties(ctx, "query-stream", pool_key);
            handle
                .try_trigger_retry_with_properties(ctx, &result, classify_rdbms_error, properties)
                .await?;
        } else {
            handle
                .try_trigger_retry(ctx, &result, classify_rdbms_error)
                .await?;
        }

        let result = result.map(|request| request.into()).map_err(|e| e.into());
        handle
            .complete(ctx, HostResponseGolemRdbmsRequest { request: result })
            .await?
    };
    match result.request {
        Ok(request) => {
            let entry = RdbmsResultStreamEntry::new(
                request
                    .try_into()
                    .map_err(|e| anyhow!("Invalid payload: {e}"))?,
                RdbmsResultStreamState::New,
                Some(entry_rep),
                begin_oplog_idx,
            );
            let resource = ctx.as_wasi_view().table().push(entry)?;
            Ok(Ok(resource))
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
    }
}

async fn db_transaction_durable_rollback<Ctx, T, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<(), E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    E: From<RdbmsError>,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    ctx.observe_function_call(T::durability_transaction_interface(), "rollback");

    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let pre_result = if ctx.durable_execution_state().is_live {
        db_transaction_pre_rollback(ctx, entry).await
    } else {
        Ok(())
    };

    if pre_result.is_ok() {
        ctx.pre_rollback_transaction_function(begin_oplog_idx)
            .await?;
    }

    match pre_result {
        Ok(_) => {
            let result = if ctx.durable_execution_state().is_live {
                db_transaction_rollback(ctx, entry).await
            } else {
                Ok(())
            };

            if result.is_ok() {
                ctx.rolled_back_transaction_function(begin_oplog_idx)
                    .await?;
            }

            if ctx.durable_execution_state().is_live {
                let _ = db_transaction_cleanup(ctx, entry).await;
            }

            Ok(result.map_err(|e| e.into()))
        }
        Err(error) => Ok(Err(error.into())),
    }
}

async fn db_transaction_durable_commit<Ctx, T, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<(), E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    E: From<RdbmsError>,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    ctx.observe_function_call(T::durability_transaction_interface(), "commit");

    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let pre_result = if ctx.durable_execution_state().is_live {
        db_transaction_pre_commit(ctx, entry).await
    } else {
        Ok(())
    };

    if pre_result.is_ok() {
        ctx.pre_commit_transaction_function(begin_oplog_idx).await?;
    }

    match pre_result {
        Ok(_) => {
            let result = if ctx.durable_execution_state().is_live {
                db_transaction_commit(ctx, entry).await
            } else {
                Ok(())
            };

            if result.is_ok() {
                ctx.committed_transaction_function(begin_oplog_idx).await?;
            }

            if ctx.durable_execution_state().is_live {
                let _ = db_transaction_cleanup(ctx, entry).await;
            }

            Ok(result.map_err(|e| e.into()))
        }
        Err(error) => Ok(Err(error.into())),
    }
}

async fn db_transaction_drop<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    ctx.observe_function_call(T::durability_transaction_interface(), "drop");

    let entry = ctx
        .as_wasi_view()
        .table()
        .delete::<RdbmsTransactionEntry<T>>(entry)?;

    if ctx.durable_execution_state().is_live {
        if let RdbmsTransactionState::Open(transaction) = entry.state {
            ctx.pre_rollback_transaction_function(entry.begin_index)
                .await?;

            let _ = transaction.rollback_if_open().await;

            ctx.rolled_back_transaction_function(entry.begin_index)
                .await?;

            let _ = ctx
                .state
                .rdbms_service
                .deref()
                .rdbms_type_service()
                .cleanup_transaction(
                    &entry.pool_key,
                    &ctx.owned_agent_id.agent_id,
                    &transaction.transaction_id(),
                )
                .await;
        }
    } else {
        let pre_rollback = ctx
            .state
            .replay_state
            .try_get_oplog_entry(|e| e.is_pre_rollback_remote_transaction(entry.begin_index))
            .await?;

        if pre_rollback.is_some() {
            let rolled_back = ctx
                .state
                .replay_state
                .try_get_oplog_entry(|e| e.is_rolled_back_remote_transaction(entry.begin_index))
                .await?;

            if rolled_back.is_some() {
                // The rollback was recorded in a previous incarnation. FU4: the scope `End` was
                // folded into the resolver in `begin_transaction_function`, so close the durable
                // scope by awaiting it (FU5 repairs a crash-split half-pair by appending the missing
                // `End` live) instead of a positional read. Otherwise the begin index would dangle in
                // `active_durable_scopes` and mis-parent later `Start` entries.
                ctx.close_durable_scope_replay(entry.begin_index).await?;
            } else {
                // Crashed after `PreRollbackRemoteTransaction` but before the rollback was recorded.
                // `begin_transaction_function` already confirmed the external rollback (otherwise it
                // would have restarted), and consuming the pre-marker exhausted the replay, so finish
                // the durable rollback record live now. This writes the marker plus its scope `End`
                // and closes the durable scope, mirroring the explicit rollback path.
                ctx.rolled_back_transaction_function(entry.begin_index)
                    .await?;
            }
        }
    }

    Ok(())
}

pub struct RdbmsConnection<T: RdbmsType> {
    pool_key: RdbmsPoolKey,
    _owner: PhantomData<T>,
}

impl<T: RdbmsType> RdbmsConnection<T> {
    fn new(pool_key: RdbmsPoolKey) -> Self {
        Self {
            pool_key,
            _owner: PhantomData,
        }
    }
}

#[derive(Clone)]
pub struct RdbmsResultStreamEntry<T: RdbmsType + 'static> {
    request: RdbmsRequest<T>,
    state: RdbmsResultStreamState<T>,
    transaction_handle: Option<u32>,
    begin_index: OplogIndex,
}

impl<T: RdbmsType + 'static> RdbmsResultStreamEntry<T> {
    fn new(
        request: RdbmsRequest<T>,
        state: RdbmsResultStreamState<T>,
        transaction_handle: Option<u32>,
        begin_index: OplogIndex,
    ) -> Self {
        Self {
            request,
            state,
            transaction_handle,
            begin_index,
        }
    }

    fn set_open(
        &mut self,
        value: Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>,
    ) {
        self.state = RdbmsResultStreamState::Open(value);
    }
}

#[derive(Clone)]
pub enum RdbmsResultStreamState<T: RdbmsType + 'static> {
    New,
    Open(Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>),
}

fn is_db_query_stream_in_transaction<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<bool>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let transaction_handle = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsResultStreamEntry<T>>(entry)?
        .transaction_handle;
    Ok(transaction_handle.is_some())
}

async fn get_db_query_stream<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> Result<Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>, RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    let query_stream_entry = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsResultStreamEntry<T>>(entry)
        .map_err(RdbmsError::other_response_failure)?
        .clone();

    match query_stream_entry.state {
        RdbmsResultStreamState::New => {
            let query_stream = match query_stream_entry.transaction_handle {
                Some(transaction_handle) => {
                    let (_, transaction) =
                        get_db_transaction(ctx, &Resource::new_own(transaction_handle))?;
                    transaction
                        .query_stream(
                            &query_stream_entry.request.statement,
                            query_stream_entry.request.params,
                        )
                        .await
                }
                None => {
                    ctx.state
                        .rdbms_service
                        .deref()
                        .rdbms_type_service()
                        .query_stream(
                            &query_stream_entry.request.pool_key,
                            &ctx.state.owned_agent_id.agent_id,
                            &query_stream_entry.request.statement,
                            query_stream_entry.request.params,
                        )
                        .await
                }
            };
            match query_stream {
                Ok(query_stream) => {
                    ctx.as_wasi_view()
                        .table()
                        .get_mut::<RdbmsResultStreamEntry<T>>(entry)
                        .map_err(RdbmsError::other_response_failure)?
                        .set_open(query_stream.clone());

                    Ok(query_stream)
                }
                Err(error) => Err(error),
            }
        }
        RdbmsResultStreamState::Open(query_stream) => Ok(query_stream),
    }
}

#[derive(Clone)]
pub struct RdbmsTransactionEntry<T: RdbmsType + 'static> {
    pool_key: RdbmsPoolKey,
    state: RdbmsTransactionState<T>,
    begin_index: OplogIndex,
}

impl<T: RdbmsType + 'static> RdbmsTransactionEntry<T> {
    fn new(
        pool_key: RdbmsPoolKey,
        state: RdbmsTransactionState<T>,
        begin_index: OplogIndex,
    ) -> Self {
        Self {
            pool_key,
            state,
            begin_index,
        }
    }

    fn set_closed(&mut self) {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => {
                self.state = RdbmsTransactionState::Closed(transaction.deref().transaction_id())
            }
            RdbmsTransactionState::Closed(_) => (),
        }
    }

    fn transaction_id(&self) -> TransactionId {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => transaction.deref().transaction_id(),
            RdbmsTransactionState::Closed(id) => id.clone(),
        }
    }
}

#[derive(Clone)]
pub enum RdbmsTransactionState<T: RdbmsType + 'static> {
    Open(Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>),
    Closed(TransactionId),
}

fn get_db_transaction<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<
    (
        RdbmsPoolKey,
        Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>,
    ),
    RdbmsError,
>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let transaction_entry = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map_err(RdbmsError::other_response_failure)?
        .clone();

    match transaction_entry.state {
        RdbmsTransactionState::Open(transaction) => Ok((transaction_entry.pool_key, transaction)),
        RdbmsTransactionState::Closed(_) => {
            Err(RdbmsError::other_response_failure("Transaction is closed"))
        }
    }
}

fn db_connection_query_stream<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> Result<RdbmsRequest<T>, RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    T::DbValue: FromRdbmsValue<P>,
{
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)
        .map_err(RdbmsError::other_response_failure)?
        .pool_key
        .clone();

    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => Ok(RdbmsRequest::<T>::new(pool_key, statement, params, None)),
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
    }
}

fn db_transaction_query_stream<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<RdbmsRequest<T>, RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    T::DbValue: FromRdbmsValue<P>,
{
    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => {
            let (pool_key, transaction) = get_db_transaction(ctx, entry)?;
            Ok(RdbmsRequest::<T>::new(
                pool_key,
                statement,
                params,
                Some(transaction.transaction_id()),
            ))
        }
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
    }
}

fn get_db_transaction_state<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<RdbmsTransactionState<T>, RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    ctx.as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| e.state.clone())
        .map_err(RdbmsError::other_response_failure)
}

async fn db_transaction_pre_commit<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let state = get_db_transaction_state(ctx, entry)?;
    match state {
        RdbmsTransactionState::Open(transaction) => transaction.pre_commit().await,
        _ => Ok(()),
    }
}

async fn db_transaction_commit<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let state = get_db_transaction_state(ctx, entry)?;
    match state {
        RdbmsTransactionState::Open(transaction) => {
            let result = transaction.commit().await;
            ctx.as_wasi_view()
                .table()
                .get_mut::<RdbmsTransactionEntry<T>>(entry)
                .map_err(RdbmsError::other_response_failure)?
                .set_closed();
            result
        }
        _ => Ok(()),
    }
}

async fn db_transaction_pre_rollback<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let state = get_db_transaction_state(ctx, entry)?;
    match state {
        RdbmsTransactionState::Open(transaction) => transaction.pre_rollback().await,
        _ => Ok(()),
    }
}

async fn db_transaction_rollback<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let state = get_db_transaction_state(ctx, entry)?;
    match state {
        RdbmsTransactionState::Open(transaction) => {
            let result = transaction.rollback().await;
            ctx.as_wasi_view()
                .table()
                .get_mut::<RdbmsTransactionEntry<T>>(entry)
                .map_err(RdbmsError::other_response_failure)?
                .set_closed();
            result
        }
        _ => Ok(()),
    }
}

async fn db_transaction_cleanup<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    let result = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| (e.pool_key.clone(), e.transaction_id()));

    match result {
        Ok((pool_key, transaction_id)) => {
            let agent_id = ctx.state.owned_agent_id.agent_id.clone();
            ctx.state
                .rdbms_service
                .rdbms_type_service()
                .cleanup_transaction(&pool_key, &agent_id, &transaction_id)
                .await
        }
        Err(error) => Err(RdbmsError::other_response_failure(error)),
    }
}

trait FromRdbmsValue<T>: Sized {
    fn from(value: T, resource_table: &mut ResourceTable) -> Result<Self, String>;
}

fn to_db_values<T, P>(
    values: Vec<P>,
    resource_table: &mut ResourceTable,
) -> Result<Vec<T::DbValue>, String>
where
    T: RdbmsType + 'static,
    T::DbValue: FromRdbmsValue<P>,
{
    let mut result: Vec<T::DbValue> = Vec::with_capacity(values.len());
    for value in values {
        let v: T::DbValue = FromRdbmsValue::from(value, resource_table)?;
        result.push(v);
    }
    Ok(result)
}

struct RdbmsRemoteTransactionHandler<T: RdbmsType> {
    pool_key: RdbmsPoolKey,
    agent_id: AgentId,
    rdbms_service: Arc<dyn RdbmsService>,
    _owner: PhantomData<T>,
}

impl<T> RdbmsRemoteTransactionHandler<T>
where
    T: RdbmsType + Send + Sync + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    fn new(
        pool_key: RdbmsPoolKey,
        agent_id: AgentId,
        rdbms_service: Arc<dyn RdbmsService>,
    ) -> Self {
        Self {
            pool_key,
            agent_id,
            rdbms_service,
            _owner: PhantomData,
        }
    }

    async fn get_transaction_status(
        &self,
        transaction_id: &TransactionId,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        self.rdbms_service
            .rdbms_type_service()
            .get_transaction_status(&self.pool_key, &self.agent_id, transaction_id)
            .await
    }
}

#[async_trait]
impl<T> RemoteTransactionHandler<RdbmsTransactionState<T>, RdbmsError>
    for RdbmsRemoteTransactionHandler<T>
where
    T: RdbmsType + Send + Sync + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    async fn create_new(&self) -> Result<(TransactionId, RdbmsTransactionState<T>), RdbmsError> {
        let transaction = self
            .rdbms_service
            .deref()
            .rdbms_type_service()
            .begin_transaction(&self.pool_key, &self.agent_id)
            .await?;

        let transaction_id = transaction.transaction_id();

        Ok((transaction_id, RdbmsTransactionState::Open(transaction)))
    }

    async fn create_replay(
        &self,
        transaction_id: &TransactionId,
    ) -> Result<(TransactionId, RdbmsTransactionState<T>), RdbmsError> {
        Ok((
            transaction_id.clone(),
            RdbmsTransactionState::Closed(transaction_id.clone()),
        ))
    }

    async fn is_committed(&self, transaction_id: &TransactionId) -> Result<bool, RdbmsError> {
        let transaction_status = self.get_transaction_status(transaction_id).await?;
        Ok(transaction_status == RdbmsTransactionStatus::Committed)
    }

    async fn is_rolled_back(&self, transaction_id: &TransactionId) -> Result<bool, RdbmsError> {
        let transaction_status = self.get_transaction_status(transaction_id).await?;
        // if transaction is not found, it is considered as rolled back
        Ok(transaction_status == RdbmsTransactionStatus::RolledBack
            || transaction_status == RdbmsTransactionStatus::NotFound)
    }
}
