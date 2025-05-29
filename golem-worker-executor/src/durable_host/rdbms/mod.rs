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
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
use crate::services::rdbms::{Error as RdbmsError, RdbmsService, RdbmsTypeService};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsType};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::base_model::OplogIndex;
use golem_common::model::oplog::DurableFunctionType;
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
    T: RdbmsType + Clone + 'static,
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
    T: RdbmsType + Clone + 'static,
    E: From<RdbmsError>,
{
    let interface = get_db_connection_interface::<T>();
    ctx.observe_function_call(interface.as_str(), "begin-transaction");

    let begin_oplog_index = ctx
        .begin_durable_function(&DurableFunctionType::WriteRemoteBatched(None))
        .await?;

    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(entry)?
        .pool_key
        .clone();

    let entry = RdbmsTransactionEntry::new(pool_key, RdbmsTransactionState::New);
    let resource = ctx.as_wasi_view().table().push(entry)?;
    let handle = resource.rep();
    ctx.state
        .open_function_table
        .insert(handle, begin_oplog_index);
    Ok(Ok(resource))
}

async fn db_connection_durable_execute<Ctx, T, P, E>(
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> anyhow::Result<Result<u64, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
    T: RdbmsType + Clone + 'static,
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<T::DbColumn>,
{
    let interface = get_db_result_stream_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<Vec<T::DbColumn>, SerializableError>::new(
        ctx,
        interface.leak(),
        "get-columns",
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<crate::services::rdbms::DbRow<T::DbValue>>,
{
    let interface = get_db_result_stream_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<
        Option<Vec<crate::services::rdbms::DbRow<T::DbValue>>>,
        SerializableError,
    >::new(
        ctx,
        interface.leak(),
        "get-next",
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
    )
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
    T: RdbmsType + Clone + 'static,
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
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
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
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
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
    E: From<RdbmsError>,
{
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<(), SerializableError>::new(
        ctx,
        interface.leak(),
        "rollback",
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let result = db_transaction_rollback(ctx, entry).await;
        durability.persist(ctx, (), result).await
    } else {
        durability.replay(ctx).await
    };

    end_durable_function_if_open(ctx, handle).await?;

    Ok(result.map_err(|e| e.into()))
}

async fn db_transaction_durable_commit<Ctx, T, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<Result<(), E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + bincode::Encode + bincode::Decode + 'static,
    E: From<RdbmsError>,
{
    let interface = get_db_transaction_interface::<T>();
    let handle = entry.rep();
    let begin_oplog_idx = get_begin_oplog_index(ctx, handle)?;
    let durability = Durability::<(), SerializableError>::new(
        ctx,
        interface.leak(),
        "commit",
        DurableFunctionType::WriteRemoteBatched(Some(begin_oplog_idx)),
    )
    .await?;

    let result = if durability.is_live() {
        let result = db_transaction_commit(ctx, entry).await;
        durability.persist(ctx, (), result).await
    } else {
        durability.replay(ctx).await
    };

    end_durable_function_if_open(ctx, handle).await?;

    Ok(result.map_err(|e| e.into()))
}

async fn db_transaction_drop<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: Resource<RdbmsTransactionEntry<T>>,
) -> anyhow::Result<()>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + 'static,
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

    end_durable_function_if_open(ctx, handle).await?;

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
pub struct RdbmsResultStreamEntry<T: RdbmsType + Clone + 'static> {
    request: RdbmsRequest<T>,
    state: RdbmsResultStreamState<T>,
    transaction_handle: Option<u32>,
}

impl<T: RdbmsType + Clone + 'static> RdbmsResultStreamEntry<T> {
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
pub enum RdbmsResultStreamState<T: RdbmsType + Clone + 'static> {
    New,
    Open(Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>),
}

async fn get_db_query_stream<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> Result<Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>, RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + 'static,
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
                        get_db_transaction(ctx, &Resource::new_own(transaction_handle)).await?;
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
pub struct RdbmsTransactionEntry<T: RdbmsType + Clone + 'static> {
    pool_key: RdbmsPoolKey,
    state: RdbmsTransactionState<T>,
}

impl<T: RdbmsType + Clone + 'static> RdbmsTransactionEntry<T> {
    fn new(pool_key: RdbmsPoolKey, state: RdbmsTransactionState<T>) -> Self {
        Self { pool_key, state }
    }

    fn set_open(&mut self, value: Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>) {
        self.state = RdbmsTransactionState::Open(value);
    }

    fn set_closed(&mut self) {
        self.state = RdbmsTransactionState::Closed;
    }
}

#[derive(Clone)]
pub enum RdbmsTransactionState<T: RdbmsType + Clone + 'static> {
    New,
    Open(Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>),
    Closed,
}

async fn get_db_transaction<Ctx, T>(
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
    T: RdbmsType + Clone + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    let transaction_entry = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map_err(RdbmsError::other_response_failure)?
        .clone();

    match transaction_entry.state {
        RdbmsTransactionState::New => {
            let transaction = ctx
                .state
                .rdbms_service
                .deref()
                .rdbms_type_service()
                .begin_transaction(
                    &transaction_entry.pool_key,
                    &ctx.state.owned_worker_id.worker_id,
                )
                .await;
            match transaction {
                Ok(transaction) => {
                    ctx.as_wasi_view()
                        .table()
                        .get_mut::<RdbmsTransactionEntry<T>>(entry)
                        .map_err(RdbmsError::other_response_failure)?
                        .set_open(transaction.clone());

                    Ok((transaction_entry.pool_key, transaction))
                }
                Err(error) => Err(error),
            }
        }
        RdbmsTransactionState::Open(transaction) => Ok((transaction_entry.pool_key, transaction)),
        RdbmsTransactionState::Closed => {
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
    T: RdbmsType + Clone + 'static,
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
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
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
    T: RdbmsType + Clone + 'static,
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
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
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
    T: RdbmsType + Clone + 'static,
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
        Ok(params) => Ok(RdbmsRequest::<T>::new(pool_key, statement, params)),
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
    T: RdbmsType + Clone + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => {
            let transaction = get_db_transaction(ctx, entry).await;
            match transaction {
                Ok((pool_key, transaction)) => {
                    let result = transaction.query(&statement, params.clone()).await;
                    (
                        Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
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
    T: RdbmsType + Clone + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => {
            let transaction = get_db_transaction(ctx, entry).await;
            match transaction {
                Ok((pool_key, transaction)) => {
                    let result = transaction.execute(&statement, params.clone()).await;
                    (
                        Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
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
    T: RdbmsType + Clone + 'static,
    T::DbValue: FromRdbmsValue<P>,
{
    let pool_key = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map_err(RdbmsError::other_response_failure)?
        .pool_key
        .clone();

    match to_db_values::<T, P>(params, ctx.as_wasi_view().table()) {
        Ok(params) => Ok(RdbmsRequest::<T>::new(pool_key, statement, params)),
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
    }
}

async fn db_transaction_commit<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + 'static,
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

async fn db_transaction_rollback<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
) -> Result<(), RdbmsError>
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + 'static,
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

fn get_begin_oplog_index<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<OplogIndex> {
    let begin_oplog_idx = *ctx.state.open_function_table.get(&handle).ok_or_else(|| {
        anyhow!("No matching BeginRemoteWrite index was found for the open Rdbms request")
    })?;
    Ok(begin_oplog_idx)
}
