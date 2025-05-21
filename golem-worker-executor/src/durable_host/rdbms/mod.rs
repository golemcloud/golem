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

use crate::durable_host::rdbms::serialized::RdbmsRequest;
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, RemoteTransactionHandler};
use crate::services::rdbms::{
    Error as RdbmsError, RdbmsService, RdbmsTransactionId, RdbmsTransactionStatus, RdbmsTypeService,
};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsType};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::WorkerId;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::IoView;

pub mod mysql;
pub mod postgres;
pub mod serialized;
pub mod types;

fn get_db_connection_interface<T: RdbmsType>() -> String {
    format!("rdbms::{}::db-connection", T::default())
}

fn get_db_transaction_interface<T: RdbmsType>() -> String {
    format!("rdbms::{}::db-transaction", T::default())
}

fn get_db_result_stream_interface<T: RdbmsType>() -> String {
    format!("rdbms::{}::db-result-stream", T::default())
}

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
    let interface = get_db_connection_interface::<T>();
    ctx.observe_function_call(interface.as_str(), "open");

    let worker_id = ctx.state.owned_worker_id.worker_id.clone();
    let result = ctx
        .state
        .rdbms_service
        .rdbms_type_service()
        .create(&address, &worker_id)
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
    let interface = get_db_connection_interface::<T>();
    ctx.observe_function_call(interface.as_str(), "begin-transaction");

    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)?
        .pool_key
        .clone();

    let result = ctx
        .state
        .begin_transaction_function(
            &DurableFunctionType::WriteRemoteTransaction(None),
            RdbmsRemoteTransactionHandler::<T>::new(
                pool_key.clone(),
                ctx.state.owned_worker_id.worker_id.clone(),
                ctx.state.rdbms_service.clone(),
            ),
        )
        .await;

    match result {
        Ok((begin_oplog_idx, transaction_state)) => {
            let entry = RdbmsTransactionEntry::new(pool_key, transaction_state);
            let resource = ctx.as_wasi_view().table().push(entry)?;
            let handle = resource.rep();
            ctx.state
                .open_function_table
                .insert(handle, begin_oplog_idx);
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
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let interface = get_db_connection_interface::<T>();
    let durability = Durability::<u64, SerializableError>::new(
        ctx,
        interface.leak(),
        "execute",
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = if durability.is_live() {
        let (input, result) = db_connection_execute(statement, params, ctx, entry).await;
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };

    Ok(result.map_err(|e| e.into()))
}

async fn db_connection_durable_query<Ctx, T, P, R, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<R, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    R: FromRdbmsValue<crate::services::rdbms::DbResult<T>>,
    E: From<RdbmsError>,
{
    let interface = get_db_connection_interface::<T>();
    let durability = Durability::<crate::services::rdbms::DbResult<T>, SerializableError>::new(
        ctx,
        interface.leak(),
        "query",
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = if durability.is_live() {
        let (input, result) = db_connection_query(statement, params, ctx, entry).await;
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };

    match result {
        Ok(result) => {
            let result = FromRdbmsValue::from(result, ctx.as_wasi_view().table())
                .map_err(|e| RdbmsError::QueryResponseFailure(e).into());
            Ok(result)
        }
        Err(error) => Ok(Err(error.into())),
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
    T: RdbmsType + 'static,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let interface = get_db_connection_interface::<T>();
    let begin_oplog_idx = ctx
        .begin_durable_function(&DurableFunctionType::WriteRemoteBatched(None))
        .await?;
    let durability = Durability::<RdbmsRequest<T>, SerializableError>::new(
        ctx,
        interface.leak(),
        "query-stream",
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let result = db_connection_query_stream(statement, params, ctx, entry);
        let input = result.clone().ok();
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };
    match result {
        Ok(request) => {
            let entry = RdbmsResultStreamEntry::new(request, RdbmsResultStreamState::New, None);
            let resource = ctx.as_wasi_view().table().push(entry)?;
            let handle = resource.rep();
            ctx.state
                .open_function_table
                .insert(handle, begin_oplog_idx);
            Ok(Ok(resource))
        }
        Err(error) => {
            ctx.end_durable_function(
                &DurableFunctionType::WriteRemoteBatched(None),
                begin_oplog_idx,
                false,
            )
            .await?;

            Ok(Err(error.into()))
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
    let interface = get_db_connection_interface::<T>();
    ctx.observe_function_call(interface.as_str(), "drop");
    let worker_id = ctx.state.owned_worker_id.worker_id.clone();
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
        .remove(&pool_key, &worker_id);

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
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<T::DbColumn>,
{
    let interface = get_db_result_stream_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;

    let durable_function_type = if is_db_query_stream_in_transaction(ctx, entry)? {
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx))
    } else {
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx))
    };

    let durability = Durability::<Vec<T::DbColumn>, SerializableError>::new(
        ctx,
        interface.leak(),
        "get-columns",
        durable_function_type,
    )
    .await?;

    let result = if durability.is_live() {
        let query_stream = get_db_query_stream(ctx, entry).await;
        let result = match query_stream {
            Ok(query_stream) => query_stream.deref().get_columns().await,
            Err(error) => Err(error),
        };
        durability.persist(ctx, (), result).await
    } else {
        durability.replay(ctx).await
    };

    match result {
        Ok(columns) => {
            let result = columns
                .into_iter()
                .map(|r| FromRdbmsValue::from(r, ctx.as_wasi_view().table()))
                .collect::<Result<Vec<R>, String>>()
                .map_err(|e| anyhow!(RdbmsError::QueryResponseFailure(e)))?;
            Ok(result)
        }
        Err(error) => Err(anyhow!(error)),
    }
}

async fn db_result_stream_durable_get_next<Ctx, T, R>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<Option<Vec<R>>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<crate::services::rdbms::DbRow<T::DbValue>>,
{
    let interface = get_db_result_stream_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;

    let durable_function_type = if is_db_query_stream_in_transaction(ctx, entry)? {
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx))
    } else {
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx))
    };

    let durability = Durability::<
        Option<Vec<crate::services::rdbms::DbRow<T::DbValue>>>,
        SerializableError,
    >::new(ctx, interface.leak(), "get-next", durable_function_type)
    .await?;

    let result = if durability.is_live() {
        let query_stream = get_db_query_stream(ctx, entry).await;
        let result = match query_stream {
            Ok(query_stream) => query_stream.deref().get_next().await,
            Err(error) => Err(error),
        };
        durability.persist(ctx, (), result).await
    } else {
        durability.replay(ctx).await
    };

    match result {
        Ok(rows) => {
            let rows = match rows {
                Some(rows) => {
                    let result = rows
                        .into_iter()
                        .map(|r| FromRdbmsValue::from(r, ctx.as_wasi_view().table()))
                        .collect::<Result<Vec<R>, String>>()
                        .map_err(|e| anyhow!(RdbmsError::QueryResponseFailure(e)))?;
                    Some(result)
                }
                None => None,
            };
            Ok(rows)
        }
        Err(error) => Err(anyhow!(error)),
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
    let interface = get_db_result_stream_interface::<T>();
    ctx.observe_function_call(interface.as_str(), "drop");

    let handle = entry.rep();
    let entry = ctx
        .as_wasi_view()
        .table()
        .delete::<RdbmsResultStreamEntry<T>>(entry)?;

    if entry.transaction_handle.is_none() {
        end_durable_function_if_open(ctx, handle).await?;
    } else {
        ctx.state.open_function_table.remove(&handle);
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
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    R: FromRdbmsValue<crate::services::rdbms::DbResult<T>>,
    E: From<RdbmsError>,
{
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<crate::services::rdbms::DbResult<T>, SerializableError>::new(
        ctx,
        interface.leak(),
        "query",
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let (input, result) = db_transaction_query(statement, params, ctx, entry).await;
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };

    match result {
        Ok(result) => {
            let result = FromRdbmsValue::from(result, ctx.as_wasi_view().table())
                .map_err(|e| RdbmsError::QueryResponseFailure(e).into());
            Ok(result)
        }
        Err(error) => Ok(Err(error.into())),
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
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<u64, SerializableError>::new(
        ctx,
        interface.leak(),
        "execute",
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let (input, result) = db_transaction_execute(statement, params, ctx, entry).await;
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };

    Ok(result.map_err(|e| e.into()))
}

async fn db_transaction_durable_query_stream<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<Resource<RdbmsResultStreamEntry<T>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let handle = entry.rep();
    let interface = get_db_transaction_interface::<T>();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<RdbmsRequest<T>, SerializableError>::new(
        ctx,
        interface.leak(),
        "query-stream",
        DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let result = db_transaction_query_stream(statement, params, ctx, entry);
        let input = result.clone().ok();
        durability.persist(ctx, input, result).await
    } else {
        durability.replay(ctx).await
    };
    match result {
        Ok(request) => {
            let entry =
                RdbmsResultStreamEntry::new(request, RdbmsResultStreamState::New, Some(handle));
            let resource = ctx.as_wasi_view().table().push(entry)?;
            let handle = resource.rep();
            ctx.state
                .open_function_table
                .insert(handle, begin_oplog_idx);
            Ok(Ok(resource))
        }
        Err(error) => Ok(Err(error.into())),
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
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;

    let pre_result = if ctx.durable_execution_state().is_live {
        let result = db_transaction_pre_rollback(ctx, entry).await;
        match result {
            Ok(_) => {
                ctx.state
                    .pre_rollback_transaction_function(
                        &DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
                    )
                    .await?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    };

    match pre_result {
        Ok(_) => {
            let durability = Durability::<(), SerializableError>::new(
                ctx,
                interface.leak(),
                "rollback",
                DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let result = db_transaction_rollback(ctx, entry).await;
                durability.persist(ctx, (), result).await
            } else {
                durability.replay(ctx).await
            };

            rolled_back_durable_transaction_if_open(ctx, handle).await?;

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
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;

    let pre_result = if ctx.durable_execution_state().is_live {
        let result = db_transaction_pre_commit(ctx, entry).await;
        match result {
            Ok(_) => {
                ctx.state
                    .pre_commit_transaction_function(&DurableFunctionType::WriteRemoteTransaction(
                        Some(begin_oplog_idx),
                    ))
                    .await?;
                Ok(())
            }
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    };

    match pre_result {
        Ok(_) => {
            let durability = Durability::<(), SerializableError>::new(
                ctx,
                interface.leak(),
                "commit",
                DurableFunctionType::WriteRemoteTransaction(Some(begin_oplog_idx)),
            )
            .await?;

            let result = if durability.is_live() {
                let result = db_transaction_commit(ctx, entry).await;
                durability.persist(ctx, (), result).await
            } else {
                durability.replay(ctx).await
            };

            commited_durable_transaction_if_open(ctx, handle).await?;

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
{
    let interface = get_db_transaction_interface::<T>();

    ctx.observe_function_call(interface.as_str(), "drop");

    let handle = entry.rep();
    let entry = ctx
        .as_wasi_view()
        .table()
        .delete::<RdbmsTransactionEntry<T>>(entry)?;

    if let RdbmsTransactionState::Open(transaction) = entry.state {
        let _ = transaction.rollback_if_open().await;
    }

    rolled_back_durable_transaction_if_open(ctx, handle).await?;

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
}

impl<T: RdbmsType + 'static> RdbmsResultStreamEntry<T> {
    fn new(
        request: RdbmsRequest<T>,
        state: RdbmsResultStreamState<T>,
        transaction_handle: Option<u32>,
    ) -> Self {
        Self {
            request,
            state,
            transaction_handle,
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
                            &ctx.state.owned_worker_id.worker_id,
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
}

impl<T: RdbmsType + 'static> RdbmsTransactionEntry<T> {
    fn new(pool_key: RdbmsPoolKey, state: RdbmsTransactionState<T>) -> Self {
        Self { pool_key, state }
    }

    fn set_closed(&mut self) {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => {
                self.state = RdbmsTransactionState::Closed(transaction.deref().transaction_id())
            }
            RdbmsTransactionState::Closed(_) => (),
        }
    }

    fn transaction_id(&self) -> RdbmsTransactionId {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => transaction.deref().transaction_id(),
            RdbmsTransactionState::Closed(id) => id.clone(),
        }
    }
}

#[derive(Clone)]
pub enum RdbmsTransactionState<T: RdbmsType + 'static> {
    Open(Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>),
    Closed(RdbmsTransactionId),
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

async fn db_connection_query<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> (
    Option<RdbmsRequest<T>>,
    Result<crate::services::rdbms::DbResult<T>, RdbmsError>,
)
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    let worker_id = ctx.state.owned_worker_id.worker_id.clone();
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)
        .map(|v| v.pool_key.clone());

    match pool_key {
        Ok(pool_key) => match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(params) => {
                let result = ctx
                    .state
                    .rdbms_service
                    .deref()
                    .rdbms_type_service()
                    .query(&pool_key, &worker_id, &statement, params.clone())
                    .await;
                (
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params, None)),
                    result,
                )
            }
            Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
        },
        Err(error) => (None, Err(RdbmsError::other_response_failure(error))),
    }
}

async fn db_connection_execute<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> (Option<RdbmsRequest<T>>, Result<u64, RdbmsError>)
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    let worker_id = ctx.state.owned_worker_id.worker_id.clone();

    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)
        .map(|v| v.pool_key.clone());

    match pool_key {
        Ok(pool_key) => match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
            Ok(params) => {
                let result = ctx
                    .state
                    .rdbms_service
                    .deref()
                    .rdbms_type_service()
                    .execute(&pool_key, &worker_id, &statement, params.clone())
                    .await;
                (
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params, None)),
                    result,
                )
            }
            Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
        },
        Err(error) => (None, Err(RdbmsError::other_response_failure(error))),
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

async fn db_transaction_query<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> (
    Option<RdbmsRequest<T>>,
    Result<crate::services::rdbms::DbResult<T>, RdbmsError>,
)
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => {
            let transaction = get_db_transaction(ctx, entry);
            match transaction {
                Ok((pool_key, transaction)) => {
                    let result = transaction.query(&statement, params.clone()).await;
                    (
                        Some(RdbmsRequest::<T>::new(
                            pool_key,
                            statement,
                            params,
                            Some(transaction.transaction_id()),
                        )),
                        result,
                    )
                }
                Err(error) => (None, Err(error)),
            }
        }
        Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
    }
}

async fn db_transaction_execute<Ctx, T, P>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> (Option<RdbmsRequest<T>>, Result<u64, RdbmsError>)
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => {
            let transaction = get_db_transaction(ctx, entry);
            match transaction {
                Ok((pool_key, transaction)) => {
                    let result = transaction.execute(&statement, params.clone()).await;
                    (
                        Some(RdbmsRequest::<T>::new(
                            pool_key,
                            statement,
                            params,
                            Some(transaction.transaction_id()),
                        )),
                        result,
                    )
                }
                Err(error) => (None, Err(error)),
            }
        }
        Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
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

async fn db_transaction_pre_commit<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
{
    let state = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| e.state.clone());

    match state {
        Ok(RdbmsTransactionState::Open(transaction)) => transaction.pre_commit().await,
        Ok(_) => Ok(()),
        Err(error) => Err(RdbmsError::other_response_failure(error)),
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
    let state = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| e.state.clone());

    match state {
        Ok(RdbmsTransactionState::Open(transaction)) => {
            transaction.commit().await?;
            ctx.as_wasi_view()
                .table()
                .get_mut::<RdbmsTransactionEntry<T>>(entry)
                .map_err(RdbmsError::other_response_failure)?
                .set_closed();
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) => Err(RdbmsError::other_response_failure(error)),
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
    let state = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| e.state.clone());

    match state {
        Ok(RdbmsTransactionState::Open(transaction)) => transaction.pre_rollback().await,
        Ok(_) => Ok(()),
        Err(error) => Err(RdbmsError::other_response_failure(error)),
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
    let state = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map(|e| e.state.clone());

    match state {
        Ok(RdbmsTransactionState::Open(transaction)) => {
            transaction.rollback().await?;
            ctx.as_wasi_view()
                .table()
                .get_mut::<RdbmsTransactionEntry<T>>(entry)
                .map_err(RdbmsError::other_response_failure)?
                .set_closed();
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) => Err(RdbmsError::other_response_failure(error)),
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
            let worker_id = ctx.state.owned_worker_id.worker_id.clone();
            ctx.state
                .rdbms_service
                .rdbms_type_service()
                .cleanup_transaction(&pool_key, &worker_id, &transaction_id)
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

async fn end_durable_function_if_open<Ctx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<Option<OplogIndex>>
where
    Ctx: WorkerCtx,
{
    let begin_oplog_idx = ctx.state.open_function_table.get(&handle).cloned();
    if let Some(begin_oplog_idx) = begin_oplog_idx {
        ctx.end_durable_function(
            &DurableFunctionType::WriteRemoteBatched(None),
            begin_oplog_idx,
            false,
        )
        .await?;
        ctx.state.open_function_table.remove(&handle);

        Ok(Some(begin_oplog_idx))
    } else {
        Ok(None)
    }
}

async fn commited_durable_transaction_if_open<Ctx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<Option<OplogIndex>>
where
    Ctx: WorkerCtx,
{
    let begin_oplog_idx = ctx.state.open_function_table.get(&handle).cloned();
    if let Some(begin_oplog_idx) = begin_oplog_idx {
        ctx.state
            .commited_transaction_function(
                &DurableFunctionType::WriteRemoteTransaction(None),
                begin_oplog_idx,
            )
            .await?;
        ctx.state.open_function_table.remove(&handle);

        Ok(Some(begin_oplog_idx))
    } else {
        Ok(None)
    }
}

async fn rolled_back_durable_transaction_if_open<Ctx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<Option<OplogIndex>>
where
    Ctx: WorkerCtx,
{
    let begin_oplog_idx = ctx.state.open_function_table.get(&handle).cloned();
    if let Some(begin_oplog_idx) = begin_oplog_idx {
        ctx.state
            .rolled_back_transaction_function(
                &DurableFunctionType::WriteRemoteTransaction(None),
                begin_oplog_idx,
            )
            .await?;
        ctx.state.open_function_table.remove(&handle);

        Ok(Some(begin_oplog_idx))
    } else {
        Ok(None)
    }
}

fn get_begin_oplog_index<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<OplogIndex> {
    let begin_oplog_idx = *ctx.state.open_function_table.get(&handle).ok_or_else(|| {
        anyhow!("No matching BeginRemoteWrite index was found for the open Rdbms request")
    })?;
    Ok(begin_oplog_idx)
}

struct RdbmsRemoteTransactionHandler<T: RdbmsType> {
    pool_key: RdbmsPoolKey,
    worker_id: WorkerId,
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
        worker_id: WorkerId,
        rdbms_service: Arc<dyn RdbmsService>,
    ) -> Self {
        Self {
            pool_key,
            worker_id,
            rdbms_service,
            _owner: PhantomData,
        }
    }

    async fn get_transaction_status(
        &self,
        transaction_id: &str,
    ) -> Result<RdbmsTransactionStatus, RdbmsError> {
        self.rdbms_service
            .rdbms_type_service()
            .get_transaction_status(
                &self.pool_key,
                &self.worker_id,
                &RdbmsTransactionId::new(transaction_id),
            )
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
    async fn create_new(&self) -> Result<(String, RdbmsTransactionState<T>), RdbmsError> {
        let transaction = self
            .rdbms_service
            .deref()
            .rdbms_type_service()
            .begin_transaction(&self.pool_key, &self.worker_id)
            .await?;

        let transaction_id = transaction.transaction_id();

        Ok((
            transaction_id.to_string(),
            RdbmsTransactionState::Open(transaction),
        ))
    }

    async fn create_replay(
        &self,
        transaction_id: &str,
    ) -> Result<(String, RdbmsTransactionState<T>), RdbmsError> {
        Ok((
            transaction_id.to_string(),
            RdbmsTransactionState::Closed(RdbmsTransactionId::new(transaction_id)),
        ))
    }

    async fn is_committed(&self, transaction_id: &str) -> Result<bool, RdbmsError> {
        let transaction_status = self.get_transaction_status(transaction_id).await?;
        Ok(transaction_status == RdbmsTransactionStatus::Committed)
    }

    async fn is_rolled_back(&self, transaction_id: &str) -> Result<bool, RdbmsError> {
        let transaction_status = self.get_transaction_status(transaction_id).await?;
        Ok(transaction_status == RdbmsTransactionStatus::RolledBack)
    }

    async fn cleanup(&self, transaction_id: &str) -> Result<(), RdbmsError> {
        self.rdbms_service
            .deref()
            .rdbms_type_service()
            .cleanup_transaction(
                &self.pool_key,
                &self.worker_id,
                &RdbmsTransactionId::new(transaction_id),
            )
            .await
    }
}
