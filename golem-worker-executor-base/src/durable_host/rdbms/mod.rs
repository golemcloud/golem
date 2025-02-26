// Copyright 2024-2025 Golem Cloud
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

use crate::durable_host::rdbms::serialized::RdbmsRequest;
use crate::durable_host::DurableWorkerCtx;
use crate::services::rdbms::{Error as RdbmsError, RdbmsService, RdbmsTypeService};
use crate::services::rdbms::{RdbmsPoolKey, RdbmsType};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::base_model::OplogIndex;
use golem_common::model::WorkerId;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::WasiView;

pub mod mysql;
pub mod postgres;
pub mod serialized;
pub mod types;

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
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
{
    let query_stream_entry = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsResultStreamEntry<T>>(entry)
        .map_err(|e| RdbmsError::Other(e.to_string()))?
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
                        .map_err(|e| RdbmsError::Other(e.to_string()))?
                        .set_open(query_stream.clone());

                    Ok(query_stream)
                }
                Err(e) => Err(e),
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
}

#[derive(Clone)]
pub enum RdbmsTransactionState<T: RdbmsType + Clone + 'static> {
    New,
    Open(Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>),
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
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
{
    let transaction_entry = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsTransactionEntry<T>>(entry)
        .map_err(|e| RdbmsError::Other(e.to_string()))?
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
                        .map_err(|e| RdbmsError::Other(e.to_string()))?
                        .set_open(transaction.clone());

                    Ok((transaction_entry.pool_key, transaction))
                }
                Err(e) => Err(e),
            }
        }
        RdbmsTransactionState::Open(transaction) => Ok((transaction_entry.pool_key, transaction)),
    }
}

async fn db_connection_query<Ctx, T, P>(
    worker_id: &WorkerId,
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
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
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
                    .query(&pool_key, worker_id, &statement, params.clone())
                    .await;
                (
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
                    result,
                )
            }
            Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
        },
        Err(error) => (None, Err(RdbmsError::Other(error.to_string()))),
    }
}

async fn db_connection_execute<Ctx, T, P>(
    worker_id: &WorkerId,
    statement: String,
    params: Vec<P>,
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsConnection<T>>,
) -> (Option<RdbmsRequest<T>>, Result<u64, RdbmsError>)
where
    Ctx: WorkerCtx,
    T: RdbmsType + Clone + 'static,
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
{
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
                    .execute(&pool_key, worker_id, &statement, params.clone())
                    .await;
                (
                    Some(RdbmsRequest::<T>::new(pool_key, statement, params)),
                    result,
                )
            }
            Err(error) => (None, Err(RdbmsError::QueryParameterFailure(error))),
        },
        Err(error) => (None, Err(RdbmsError::Other(error.to_string()))),
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
        .map_err(|e| RdbmsError::Other(e.to_string()))?
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
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
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
                Err(e) => (None, Err(e)),
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
    dyn RdbmsService + Send + Sync: RdbmsTypeService<T>,
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
                Err(e) => (None, Err(e)),
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
        .map_err(|e| RdbmsError::Other(e.to_string()))?
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
        Ok(RdbmsTransactionState::Open(transaction)) => transaction.commit().await,
        Ok(_) => Ok(()),
        Err(e) => Err(RdbmsError::Other(e.to_string())),
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
        Ok(RdbmsTransactionState::Open(transaction)) => transaction.rollback().await,
        Ok(_) => Ok(()),
        Err(e) => Err(RdbmsError::Other(e.to_string())),
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

fn get_begin_oplog_index<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    handle: u32,
) -> anyhow::Result<OplogIndex> {
    let begin_oplog_idx = *ctx.state.open_function_table.get(&handle).ok_or_else(|| {
        anyhow!("No matching BeginRemoteWrite index was found for the open Rdbms request")
    })?;
    Ok(begin_oplog_idx)
}
