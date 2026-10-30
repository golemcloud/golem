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

use crate::durable_host::authorization::targets::{
    RdbmsEngine, TargetError, rdbms_sql_targets, rdbms_target,
};
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, DurableCallSession, NotCancellable, ResolvedCall,
};
use crate::durable_host::durability::{ClassifiedHostError, DurabilityHost, HostFailureKind};
use crate::durable_host::keyvalue::environment_owner;
use crate::durable_host::rdbms::serialized::RdbmsRequest;
use crate::durable_host::{
    DurableWorkerCtx, InternalRetryResult, LiveAuthorizationPermit, RemoteTransactionHandler,
};
use crate::services::HasWorker;
use crate::services::oplog::CommitLevel;
use crate::services::rdbms::{DbResult, DbRow, RdbmsType};
use crate::services::rdbms::{RdbmsError, RdbmsService, RdbmsTransactionStatus, RdbmsTypeService};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use async_trait::async_trait;
use golem_common::model::card::{
    ClassPermissionTarget, PermissionTarget, RdbmsResourcePattern, RdbmsVerb,
};
use golem_common::model::oplog::{DeniablePair, HostPayloadPair};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemRdbmsRequest, HostRequestNoInput,
    HostResponseGolemRdbmsColumns, HostResponseGolemRdbmsRequest, HostResponseGolemRdbmsResult,
    HostResponseGolemRdbmsResultChunk, HostResponseGolemRdbmsRowCount,
};
use golem_common::model::retry_policy::RetryProperties;
use golem_common::model::{AgentId, OplogIndex, RdbmsPoolKey, RetryContext, TransactionId};
use sqlx::ConnectOptions;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::IoView;

mod drop;
pub mod ignite;
pub mod mysql;
pub mod postgres;
pub mod serialized;
pub mod types;

/// Close resident transaction protocols before discarding a trapped Store's resource table.
/// This uses the guest Drop terminal, including its recorded outcome during replay; it
/// does not promise that the database rolled the transaction back.
pub(crate) async fn cleanup_trapped_transactions<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> anyhow::Result<()> {
    use crate::services::rdbms::{ignite::IgniteType, mysql::MysqlType, postgres::PostgresType};

    drop::cleanup_pending::<Ctx, MysqlType>(ctx).await?;
    drop::cleanup_pending::<Ctx, PostgresType>(ctx).await?;
    drop::cleanup_pending::<Ctx, IgniteType>(ctx).await?;

    enum Transaction {
        Mysql(Resource<RdbmsTransactionEntry<MysqlType>>),
        Postgres(Resource<RdbmsTransactionEntry<PostgresType>>),
        Ignite(Resource<RdbmsTransactionEntry<IgniteType>>),
    }

    let mut transactions = Vec::new();
    for entry in ctx.table().iter_mut() {
        if let Some(tx) = entry.downcast_ref::<RdbmsTransactionEntry<MysqlType>>() {
            transactions.push((tx.begin_index, Transaction::Mysql(tx.resource()?)));
        } else if let Some(tx) = entry.downcast_ref::<RdbmsTransactionEntry<PostgresType>>() {
            transactions.push((tx.begin_index, Transaction::Postgres(tx.resource()?)));
        } else if let Some(tx) = entry.downcast_ref::<RdbmsTransactionEntry<IgniteType>>() {
            transactions.push((tx.begin_index, Transaction::Ignite(tx.resource()?)));
        }
    }
    transactions.sort_by_key(|(begin, _)| *begin);
    for (begin, transaction) in transactions {
        if !ctx.state.transaction_protocols.contains_key(&begin) {
            continue;
        }
        match transaction {
            Transaction::Mysql(resource) => db_transaction_drop(ctx, resource).await?,
            Transaction::Postgres(resource) => db_transaction_drop(ctx, resource).await?,
            Transaction::Ignite(resource) => db_transaction_drop(ctx, resource).await?,
        }
    }
    anyhow::ensure!(
        ctx.state.transaction_protocols.is_empty(),
        "Trapped Store has a transaction protocol without a resource"
    );
    Ok(())
}

fn classify_rdbms_error(error: &RdbmsError) -> HostFailureKind {
    match error {
        RdbmsError::ConnectionFailure(_) => HostFailureKind::Transient,
        RdbmsError::QueryExecutionFailure(_) => HostFailureKind::Transient,
        RdbmsError::QueryParameterFailure(_) => HostFailureKind::Permanent,
        RdbmsError::QueryResponseFailure(_) => HostFailureKind::Permanent,
        RdbmsError::Other(_) => HostFailureKind::Transient,
        RdbmsError::ResourceLost(_) => HostFailureKind::Permanent,
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
    const ENGINE: RdbmsEngine;
    type ConnBeginTransaction: DeniablePair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsRowCount>;
    type ConnExecute: DeniablePair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsRowCount>;
    type ConnQuery: DeniablePair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsResult>;
    type ConnQueryStream: DeniablePair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsRequest>;
    type TxnExecute: DeniablePair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsRowCount>;
    type TxnQuery: DeniablePair<Req = HostRequestGolemRdbmsRequest, Resp = HostResponseGolemRdbmsResult>;
    type TxnQueryStream: DeniablePair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsRequest>;
    type StreamGetColumns: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsColumns>;
    type StreamGetNext: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseGolemRdbmsResultChunk>;
}

const RDBMS_PERMISSION_DENIED: &str = "RDBMS permission denied";

async fn database_and_schema(
    engine: RdbmsEngine,
    pool_key: &RdbmsPoolKey,
) -> Result<(String, String), String> {
    match engine {
        RdbmsEngine::Mysql => {
            let options = sqlx::mysql::MySqlConnectOptions::from_url(&pool_key.address)
                .map_err(|error| error.to_string())?;
            let database = options.get_database().unwrap_or_default().to_string();
            Ok((database.clone(), database))
        }
        RdbmsEngine::Postgres => {
            let address = pool_key.address.clone();
            // SQLx also resolves query overrides, environment defaults and password files.
            let database = tokio::task::spawn_blocking(move || {
                let options = sqlx::postgres::PgConnectOptions::from_url(&address)
                    .map_err(|error| error.to_string())?;
                Ok::<_, String>(
                    options
                        .get_database()
                        .unwrap_or_else(|| options.get_username())
                        .to_string(),
                )
            })
            .await
            .map_err(|error| error.to_string())??;
            Ok((database, "public".to_string()))
        }
        // Ignite's thin-client URL has no database path. Its SQL namespace starts at PUBLIC.
        RdbmsEngine::Ignite => Ok(("default".to_string(), "PUBLIC".to_string())),
    }
}

async fn authorize_sql<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    pool_key: &RdbmsPoolKey,
    statement: &str,
) -> anyhow::Result<Result<LiveAuthorizationPermit, String>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
{
    let (database, schema) = match database_and_schema(T::ENGINE, pool_key).await {
        Ok(database_and_schema) => database_and_schema,
        Err(error) => {
            return Ok(Err(format!(
                "{RDBMS_PERMISSION_DENIED}: cannot determine connection database: {error}"
            )));
        }
    };
    let targets = match rdbms_sql_targets(
        environment_owner(ctx),
        T::ENGINE,
        &database,
        &schema,
        statement,
    ) {
        Ok(targets) => targets,
        Err(TargetError::SqlNotStaticallyExtractable(reason)) => {
            let target = PermissionTarget::Rdbms(ClassPermissionTarget {
                verb: Some(RdbmsVerb::Unanalyzed),
                owner: environment_owner(ctx),
                resource: RdbmsResourcePattern::Database {
                    database: database.clone(),
                },
            });
            return Ok(ctx
                .authorize_live_permissions(&[target])
                .await?
                .map_err(|error| {
                    format!(
                        "{RDBMS_PERMISSION_DENIED}: cannot safely determine SQL targets: {reason}; \
                         unsupported SQL requires an explicitly assigned rdbms unanalyzed grant \
                         on database({database}): {error}"
                    )
                }));
        }
        Err(error) => {
            return Ok(Err(format!(
                "{RDBMS_PERMISSION_DENIED}: cannot safely determine SQL targets: {error}"
            )));
        }
    };
    Ok(ctx
        .authorize_live_permissions(&targets)
        .await?
        .map_err(|error| format!("{RDBMS_PERMISSION_DENIED}: {error}")))
}

async fn authorize_transaction<Ctx, T>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    pool_key: &RdbmsPoolKey,
) -> anyhow::Result<Result<LiveAuthorizationPermit, String>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
{
    let (database, _) = match database_and_schema(T::ENGINE, pool_key).await {
        Ok(database_and_schema) => database_and_schema,
        Err(error) => {
            return Ok(Err(format!(
                "{RDBMS_PERMISSION_DENIED}: cannot determine connection database: {error}"
            )));
        }
    };
    let targets = [
        rdbms_target(
            environment_owner(ctx),
            RdbmsVerb::Query,
            &database,
            "*",
            "*",
        ),
        rdbms_target(
            environment_owner(ctx),
            RdbmsVerb::Mutate,
            &database,
            "*",
            "*",
        ),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>();
    let targets = match targets {
        Ok(targets) => targets,
        Err(error) => {
            return Ok(Err(format!(
                "{RDBMS_PERMISSION_DENIED}: cannot construct database wildcard: {error}"
            )));
        }
    };
    Ok(ctx
        .authorize_live_permissions(&targets)
        .await?
        .map_err(|error| format!("{RDBMS_PERMISSION_DENIED}: {error}")))
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

    // Opening is deliberately parse-only. The service (and therefore a socket/pool/client) is
    // created after the first statement has been extracted and admitted. Transactions require an
    // admitted wildcard statement set at begin time; ordinary connections remain fully lazy.
    let result = RdbmsPoolKey::from(&address).map_err(RdbmsError::ConnectionFailure);

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
    T: RdbmsType + RdbmsDurabilityPairs + Send + Sync + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    E: From<RdbmsError>,
{
    let begun = DurableCallSession::<T::ConnBeginTransaction, NotCancellable>::begin(
        ctx,
        DurableFunctionType::ReadLocal,
    )
    .await?;
    let (admission, pool_key, _authorization_permit) = match begun.resolve(ctx).await? {
        ResolvedCall::Live(begun) => {
            let pool_key = ctx
                .as_wasi_view()
                .table()
                .get::<RdbmsConnection<T>>(entry)?
                .pool_key
                .clone();
            match authorize_transaction::<Ctx, T>(ctx, &pool_key).await? {
                Ok(permit) => {
                    let admission = begun
                        .start_live(ctx, HostRequestNoInput {})
                        .await?
                        .complete(ctx, HostResponseGolemRdbmsRowCount { result: Ok(0) })
                        .await?;
                    (admission, Some(pool_key), Some(permit))
                }
                Err(denial) => (
                    begun
                        .start_live_denied(ctx, HostRequestNoInput {}, denial)
                        .await?,
                    Some(pool_key),
                    None,
                ),
            }
        }
        ResolvedCall::Replay(handle) => {
            let admission = match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(response) => response,
                CallReplayOutcome::Incomplete(live) => {
                    live.complete(ctx, HostResponseGolemRdbmsRowCount { result: Ok(0) })
                        .await?
                }
            };
            (admission, None, None)
        }
    };
    if let Err(error) = admission.result {
        return Ok(Err(RdbmsError::from(error).into()));
    }
    let pool_key = match pool_key {
        Some(pool_key) => pool_key,
        None => ctx
            .as_wasi_view()
            .table()
            .get::<RdbmsConnection<T>>(entry)?
            .pool_key
            .clone(),
    };

    let result = ctx
        .begin_transaction_function(RdbmsRemoteTransactionHandler::<T>::new(
            pool_key.clone(),
            ctx.state.owned_agent_id.agent_id.clone(),
            ctx.state.rdbms_service.clone(),
        ))
        .await;

    match result {
        Ok((begin_oplog_idx, transaction_state)) => {
            if matches!(transaction_state, RdbmsTransactionState::Open(_)) {
                ctx.as_wasi_view()
                    .table()
                    .get_mut::<RdbmsConnection<T>>(entry)?
                    .initialized = true;
            }
            let mut entry = RdbmsTransactionEntry::new(
                pool_key,
                transaction_state,
                begin_oplog_idx,
                !ctx.state.durability_is_suppressed(),
            );
            drop::detach_recorded_drop(ctx, &mut entry).await?;
            let resource = ctx.as_wasi_view().table().push(entry)?;
            ctx.table().get_mut(&resource)?.resource_rep = Some(resource.rep());
            Ok(Ok(resource))
        }
        Err(error) => match error.downcast::<RdbmsError>() {
            Ok(error) => Ok(Err(error.into())),
            Err(error) => Err(error),
        },
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

    let begun = DurableCallSession::<T::ConnExecute, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = 'result: {
        let (mut handle, _authorization_permit) = match begun.resolve(ctx).await? {
            ResolvedCall::Live(begun) => {
                let request = HostRequestGolemRdbmsRequest {
                    request: prepared.as_ref().ok().map(|(pool_key, db_params)| {
                        RdbmsRequest::<T>::new(
                            pool_key.clone(),
                            statement.clone(),
                            db_params.clone(),
                            None,
                        )
                        .into()
                    }),
                };
                let permit = match prepared.as_ref() {
                    Ok((pool_key, _)) => {
                        match authorize_sql::<Ctx, T>(ctx, pool_key, &statement).await? {
                            Ok(permit) => Some(permit),
                            Err(denial) => {
                                break 'result begun
                                    .start_live_denied(ctx, request, denial)
                                    .await?;
                            }
                        }
                    }
                    Err(_) => None,
                };
                (begun.start_live(ctx, request).await?, permit)
            }
            ResolvedCall::Replay(handle) => match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                CallReplayOutcome::Incomplete(live) => (live, None),
            },
        };

        let result = match &prepared {
            Ok((pool_key, db_params)) => {
                let connection = ctx
                    .state
                    .rdbms_service
                    .rdbms_type_service()
                    .create(pool_key.address.as_str(), &agent_id)
                    .await;
                match connection {
                    Err(error) => Err(error),
                    Ok(_) => {
                        ctx.as_wasi_view()
                            .table()
                            .get_mut::<RdbmsConnection<T>>(entry)?
                            .initialized = true;
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

    let begun = DurableCallSession::<T::ConnQuery, NotCancellable>::begin(
        ctx,
        DurableFunctionType::WriteRemote,
    )
    .await?;

    let result = 'result: {
        let (mut handle, _authorization_permit) = match begun.resolve(ctx).await? {
            ResolvedCall::Live(begun) => {
                let request = HostRequestGolemRdbmsRequest {
                    request: prepared.as_ref().ok().map(|(pool_key, db_params)| {
                        RdbmsRequest::<T>::new(
                            pool_key.clone(),
                            statement.clone(),
                            db_params.clone(),
                            None,
                        )
                        .into()
                    }),
                };
                let permit = match prepared.as_ref() {
                    Ok((pool_key, _)) => {
                        match authorize_sql::<Ctx, T>(ctx, pool_key, &statement).await? {
                            Ok(permit) => Some(permit),
                            Err(denial) => {
                                break 'result begun
                                    .start_live_denied(ctx, request, denial)
                                    .await?;
                            }
                        }
                    }
                    Err(_) => None,
                };
                (begun.start_live(ctx, request).await?, permit)
            }
            ResolvedCall::Replay(handle) => match handle.replay(ctx).await? {
                CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                CallReplayOutcome::Incomplete(live) => (live, None),
            },
        };

        let result = match &prepared {
            Ok((pool_key, db_params)) => {
                let connection = ctx
                    .state
                    .rdbms_service
                    .rdbms_type_service()
                    .create(pool_key.address.as_str(), &agent_id)
                    .await;
                match connection {
                    Err(error) => Err(error),
                    Ok(_) => {
                        ctx.as_wasi_view()
                            .table()
                            .get_mut::<RdbmsConnection<T>>(entry)?
                            .initialized = true;
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
    dyn RdbmsService: RdbmsTypeService<T>,
    T::DbValue: FromRdbmsValue<P>,
    E: From<RdbmsError>,
{
    let prepared = db_connection_query_stream(statement.clone(), params, ctx, entry);
    let mut authorization = None;
    let mut handle =
        DurableCallSession::<T::ConnQueryStream, NotCancellable>::start_with_options_and_authorize(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                incomplete_repair_authorized_by_scope: true,
                ..Default::default()
            },
            async |ctx| {
                authorization = match prepared.as_ref() {
                    Ok(request) => {
                        Some(authorize_sql::<Ctx, T>(ctx, &request.pool_key, &statement).await?)
                    }
                    Err(_) => None,
                };
                Ok::<_, anyhow::Error>(
                    authorization
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .cloned(),
                )
            },
        )
        .await?;
    let begin_index = handle.start_index();
    let reconstructed = !handle.is_live();
    if handle.is_live()
        && handle.is_persisted()
        && let Err(error) = ctx
            .public_state
            .worker()
            .commit_oplog_and_update_state(CommitLevel::Always)
            .await
    {
        handle.abandon_for_trap();
        return Err(error.into());
    }

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsRequest {
                        request: Err(rdbms_stream_lost().into()),
                    },
                )
                .await?;
        }

        if let Some(Err(error)) = authorization {
            break 'result handle
                .complete(
                    ctx,
                    HostResponseGolemRdbmsRequest {
                        request: Err(RdbmsError::QueryExecutionFailure(error).into()),
                    },
                )
                .await?;
        }

        let result = match &prepared {
            Ok(request) => match <dyn RdbmsService as RdbmsTypeService<T>>::rdbms_type_service(
                ctx.state.rdbms_service.deref(),
            )
            .create(
                request.pool_key.address.as_str(),
                &ctx.state.owned_agent_id.agent_id,
            )
            .await
            {
                Ok(_) => {
                    ctx.as_wasi_view()
                        .table()
                        .get_mut::<RdbmsConnection<T>>(entry)?
                        .initialized = true;
                    Ok(request.clone())
                }
                Err(error) => Err(error),
            },
            Err(error) => Err(error.clone()),
        };
        let pool_key_for_props = result
            .as_ref()
            .map(|r| r.pool_key.clone())
            .ok()
            .or_else(|| {
                prepared
                    .as_ref()
                    .ok()
                    .map(|request| request.pool_key.clone())
            });
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
                if reconstructed {
                    RdbmsResultStreamState::Lost
                } else {
                    RdbmsResultStreamState::New
                },
                None,
                begin_index,
            );
            let resource = ctx.as_wasi_view().table().push(entry)?;
            Ok(Ok(resource))
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
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
    let (pool_key, initialized) = ctx
        .as_wasi_view()
        .table()
        .get::<RdbmsConnection<T>>(&entry)
        .map(|connection| (connection.pool_key.clone(), connection.initialized))?;

    if initialized {
        let _ = ctx
            .state
            .rdbms_service
            .rdbms_type_service()
            .remove(&pool_key, &agent_id)
            .await;
    }

    ctx.as_wasi_view()
        .table()
        .delete::<RdbmsConnection<T>>(entry)?;
    Ok(())
}

async fn db_result_stream_durable_get_columns<Ctx, T, R, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<Result<Vec<R>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<T::DbColumn>,
    E: From<RdbmsError>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let mut handle = DurableCallSession::<T::StreamGetColumns, NotCancellable>::start_with_options(
        ctx,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemote,
        AccessClaimOptions {
            parent_start_index: Some(begin_oplog_idx),
            incomplete_repair_authorized_by_scope: true,
            ..Default::default()
        },
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsColumns {
                        result: Err(rdbms_stream_lost().into()),
                    },
                )
                .await?;
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
        if is_db_query_stream_in_transaction(ctx, entry)?
            || matches!(ctx.table().get(entry)?.state, RdbmsResultStreamState::Lost)
        {
            // Transaction children cannot retry independently; lost cursors cannot resume.
        } else if let Ok(pool_key) = pool_key.as_ref() {
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
            Ok(Ok(result))
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
    }
}

async fn db_result_stream_durable_get_next<Ctx, T, R, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsResultStreamEntry<T>>,
) -> anyhow::Result<Result<Option<Vec<R>>, E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + RdbmsDurabilityPairs + 'static,
    dyn RdbmsService: RdbmsTypeService<T>,
    R: FromRdbmsValue<crate::services::rdbms::DbRow<T::DbValue>>,
    E: From<RdbmsError>,
{
    let begin_oplog_idx = ctx.table().get(entry)?.begin_index;

    let mut handle = DurableCallSession::<T::StreamGetNext, NotCancellable>::start_with_options(
        ctx,
        HostRequestNoInput {},
        DurableFunctionType::WriteRemote,
        AccessClaimOptions {
            parent_start_index: Some(begin_oplog_idx),
            incomplete_repair_authorized_by_scope: true,
            ..Default::default()
        },
    )
    .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsResultChunk {
                        result: Err(rdbms_stream_lost().into()),
                    },
                )
                .await?;
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
        if is_db_query_stream_in_transaction(ctx, entry)?
            || matches!(ctx.table().get(entry)?.state, RdbmsResultStreamState::Lost)
        {
            // Transaction children cannot retry independently; lost cursors cannot resume.
        } else if let Ok(pool_key) = pool_key.as_ref() {
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
            Ok(Ok(rows))
        }
        Err(error) => Ok(Err(RdbmsError::from(error).into())),
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

    ctx.as_wasi_view()
        .table()
        .delete::<RdbmsResultStreamEntry<T>>(entry)?;

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

    let db_params = to_db_values::<T, P>(params, ctx.as_wasi_view().table());
    let tx = ctx.table().get(entry)?;
    let request = HostRequestGolemRdbmsRequest {
        request: db_params.as_ref().ok().map(|params| {
            RdbmsRequest::<T>::new(
                tx.pool_key.clone(),
                statement.clone(),
                params.clone(),
                Some(tx.transaction_id()),
            )
            .into()
        }),
    };
    let prepared: PreparedTransactionRequest<T> = match db_params {
        Ok(db_params) => match get_db_transaction(ctx, entry) {
            Ok((pool_key, transaction)) => Ok((pool_key, transaction, db_params)),
            Err(error) => Err(error),
        },
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
    };

    let mut authorization = None;
    let handle =
        DurableCallSession::<T::TxnQuery, NotCancellable>::start_with_options_and_authorize(
            ctx,
            request,
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                parent_start_index: Some(begin_oplog_idx),
                incomplete_repair_authorized_by_scope: true,
                ..Default::default()
            },
            async |ctx| {
                let pool_key = ctx.table().get(entry)?.pool_key.clone();
                authorization = Some(authorize_sql::<Ctx, T>(ctx, &pool_key, &statement).await?);
                Ok::<_, anyhow::Error>(
                    authorization
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .cloned(),
                )
            },
        )
        .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsResult {
                        result: Err(rdbms_transaction_lost().into()),
                    },
                )
                .await?;
        }

        if let Some(Err(error)) = authorization {
            break 'result handle
                .complete(
                    ctx,
                    HostResponseGolemRdbmsResult {
                        result: Err(RdbmsError::QueryExecutionFailure(error).into()),
                    },
                )
                .await?;
        }

        let result = match &prepared {
            Ok((_pool_key, transaction, db_params)) => {
                transaction.query(&statement, db_params.clone()).await
            }
            Err(error) => Err(error.clone()),
        };
        // Statements belong to this transaction; retrying them independently can repeat effects.
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

    let db_params = to_db_values::<T, P>(params, ctx.as_wasi_view().table());
    let tx = ctx.table().get(entry)?;
    let request = HostRequestGolemRdbmsRequest {
        request: db_params.as_ref().ok().map(|params| {
            RdbmsRequest::<T>::new(
                tx.pool_key.clone(),
                statement.clone(),
                params.clone(),
                Some(tx.transaction_id()),
            )
            .into()
        }),
    };
    let prepared: PreparedTransactionRequest<T> = match db_params {
        Ok(db_params) => match get_db_transaction(ctx, entry) {
            Ok((pool_key, transaction)) => Ok((pool_key, transaction, db_params)),
            Err(error) => Err(error),
        },
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
    };

    let mut authorization = None;
    let handle =
        DurableCallSession::<T::TxnExecute, NotCancellable>::start_with_options_and_authorize(
            ctx,
            request,
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                parent_start_index: Some(begin_oplog_idx),
                incomplete_repair_authorized_by_scope: true,
                ..Default::default()
            },
            async |ctx| {
                let pool_key = ctx.table().get(entry)?.pool_key.clone();
                authorization = Some(authorize_sql::<Ctx, T>(ctx, &pool_key, &statement).await?);
                Ok::<_, anyhow::Error>(
                    authorization
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .cloned(),
                )
            },
        )
        .await?;

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsRowCount {
                        result: Err(rdbms_transaction_lost().into()),
                    },
                )
                .await?;
        }

        if let Some(Err(error)) = authorization {
            break 'result handle
                .complete(
                    ctx,
                    HostResponseGolemRdbmsRowCount {
                        result: Err(RdbmsError::QueryExecutionFailure(error).into()),
                    },
                )
                .await?;
        }

        let result = match &prepared {
            Ok((_pool_key, transaction, db_params)) => {
                transaction.execute(&statement, db_params.clone()).await
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
    let prepared = db_transaction_query_stream(statement.clone(), params, ctx, entry);
    let mut authorization = None;
    let handle =
        DurableCallSession::<T::TxnQueryStream, NotCancellable>::start_with_options_and_authorize(
            ctx,
            HostRequestNoInput {},
            DurableFunctionType::WriteRemote,
            AccessClaimOptions {
                parent_start_index: Some(begin_oplog_idx),
                incomplete_repair_authorized_by_scope: true,
                ..Default::default()
            },
            async |ctx| {
                authorization = match prepared.as_ref() {
                    Ok(request) => {
                        Some(authorize_sql::<Ctx, T>(ctx, &request.pool_key, &statement).await?)
                    }
                    Err(_) => None,
                };
                Ok::<_, anyhow::Error>(
                    authorization
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .cloned(),
                )
            },
        )
        .await?;
    let stream_parent = handle.start_index();
    let reconstructed = !handle.is_live();

    let result = 'result: {
        if !handle.is_live() {
            break 'result handle
                .replay_or_persist_failure(
                    ctx,
                    HostResponseGolemRdbmsRequest {
                        request: Err(rdbms_transaction_lost().into()),
                    },
                )
                .await?;
        }

        if let Some(Err(error)) = authorization {
            break 'result handle
                .complete(
                    ctx,
                    HostResponseGolemRdbmsRequest {
                        request: Err(RdbmsError::QueryExecutionFailure(error).into()),
                    },
                )
                .await?;
        }

        let result = if matches!(
            ctx.table().get(entry)?.state,
            RdbmsTransactionState::Reconstructed(_)
        ) {
            Err(rdbms_transaction_lost())
        } else {
            prepared.clone()
        };
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
                if reconstructed {
                    RdbmsResultStreamState::Lost
                } else {
                    RdbmsResultStreamState::New
                },
                Some(entry_rep),
                stream_parent,
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
    db_transaction_terminal(ctx, entry, TransactionTerminal::Rollback).await
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
    db_transaction_terminal(ctx, entry, TransactionTerminal::Commit).await
}

#[derive(Clone, Copy, PartialEq)]
enum TransactionTerminal {
    Commit,
    Rollback,
    Drop,
}

async fn db_transaction_terminal<Ctx, T, E>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    entry: &Resource<RdbmsTransactionEntry<T>>,
    terminal: TransactionTerminal,
) -> anyhow::Result<Result<(), E>>
where
    Ctx: WorkerCtx,
    T: RdbmsType + 'static,
    E: From<RdbmsError>,
    dyn RdbmsService: RdbmsTypeService<T>,
{
    use golem_common::model::oplog::OplogEntry;
    use golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal as Outcome;
    let tx = ctx.table().get(entry)?.clone();
    if matches!(tx.state, RdbmsTransactionState::Closed(_)) {
        return Ok(Ok(()));
    }
    anyhow::ensure!(
        tx.replay_drop.is_none(),
        "Transaction terminal does not match recorded Drop outcome"
    );
    let begin = tx.begin_index;
    // Resolve protocol markers by the scope owner, independent of interleaved transactions.
    let pre = ctx
        .state
        .replay_state
        .transaction_marker(begin, |e| {
            if terminal == TransactionTerminal::Commit {
                e.is_pre_commit_remote_transaction(begin)
            } else {
                e.is_pre_rollback_remote_transaction(begin)
            }
        })
        .is_some();
    let confirmed = ctx
        .state
        .replay_state
        .transaction_marker(begin, |e| {
            if terminal == TransactionTerminal::Commit {
                e.is_committed_remote_transaction(begin)
            } else {
                e.is_rolled_back_remote_transaction(begin)
            }
        })
        .is_some();
    if let Some(outcome) = ctx.replay_transaction_terminal(begin).await? {
        let result = match (terminal, outcome) {
            (TransactionTerminal::Commit, Outcome::Commit(r))
            | (TransactionTerminal::Rollback, Outcome::Rollback(r))
            | (TransactionTerminal::Drop, Outcome::Drop(r)) => r,
            _ => {
                return Err(anyhow!(
                    "Transaction terminal does not match recorded outcome"
                ));
            }
        };
        ctx.table().get_mut(entry)?.set_closed();
        return Ok(result.map_err(|e| RdbmsError::from(e).into()));
    }
    let result = match tx.state {
        RdbmsTransactionState::Reconstructed(_) if confirmed => Ok(()),
        RdbmsTransactionState::Reconstructed(ref id) if pre => {
            let status = ctx
                .state
                .rdbms_service
                .rdbms_type_service()
                .get_transaction_status(&tx.pool_key, &ctx.state.owned_agent_id.agent_id, id)
                .await;
            match status {
                Err(error) => {
                    return retry_transaction_resolution(ctx, begin, &tx.pool_key, error).await;
                }
                Ok(RdbmsTransactionStatus::InProgress) | Ok(RdbmsTransactionStatus::NotFound)
                    if terminal == TransactionTerminal::Commit =>
                {
                    return retry_transaction_resolution(
                        ctx,
                        begin,
                        &tx.pool_key,
                        RdbmsError::ConnectionFailure(
                            "Database transaction outcome is not known".to_string(),
                        ),
                    )
                    .await;
                }
                Ok(RdbmsTransactionStatus::InProgress) => {
                    return retry_transaction_resolution(
                        ctx,
                        begin,
                        &tx.pool_key,
                        RdbmsError::ConnectionFailure(
                            "Database transaction outcome is still pending".to_string(),
                        ),
                    )
                    .await;
                }
                Ok(RdbmsTransactionStatus::Committed)
                    if terminal == TransactionTerminal::Commit =>
                {
                    Ok(())
                }
                Ok(RdbmsTransactionStatus::RolledBack | RdbmsTransactionStatus::NotFound)
                    if terminal != TransactionTerminal::Commit =>
                {
                    Ok(())
                }
                _ => Err(rdbms_transaction_lost()),
            }
        }
        RdbmsTransactionState::Reconstructed(_) => Err(rdbms_transaction_lost()),
        RdbmsTransactionState::Open(ref transaction) => {
            let prepared = if terminal == TransactionTerminal::Commit {
                transaction.pre_commit().await
            } else {
                transaction.pre_rollback().await
            };
            match prepared {
                Err(error) => Err(error),
                Ok(()) => {
                    if terminal == TransactionTerminal::Commit {
                        ctx.pre_commit_transaction_function(begin).await?;
                        match transaction.commit().await {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                return retry_transaction_resolution(
                                    ctx,
                                    begin,
                                    &tx.pool_key,
                                    error,
                                )
                                .await;
                            }
                        }
                    } else {
                        ctx.pre_rollback_transaction_function(begin).await?;
                        transaction.rollback().await
                    }
                }
            }
        }
        RdbmsTransactionState::Closed(_) => unreachable!(),
    };
    let marker = if result.is_ok() && !confirmed {
        Some(if terminal == TransactionTerminal::Commit {
            OplogEntry::committed_remote_transaction(begin)
        } else {
            OplogEntry::rolled_back_remote_transaction(begin)
        })
    } else {
        None
    };
    let recorded = result.clone().map_err(Into::into);
    let outcome = match terminal {
        TransactionTerminal::Commit => Outcome::Commit(recorded),
        TransactionTerminal::Rollback => Outcome::Rollback(recorded),
        TransactionTerminal::Drop => Outcome::Drop(recorded),
    };
    ctx.finish_transaction_terminal(begin, outcome, marker)
        .await?;
    ctx.table().get_mut(entry)?.set_closed();
    let _ = db_transaction_cleanup(ctx, entry).await;
    Ok(result.map_err(Into::into))
}

async fn retry_transaction_resolution<Ctx: WorkerCtx, R>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    begin: OplogIndex,
    pool_key: &RdbmsPoolKey,
    error: RdbmsError,
) -> anyhow::Result<R> {
    // A failed status observation is not a transaction outcome. Leave the scope incomplete,
    // even if policy gives up, so another recovery can still discover a committed transaction.
    ctx.state.current_retry_point = begin;
    let properties = rdbms_retry_properties(ctx, "transaction-status", pool_key);
    match ctx
        .try_trigger_retry(anyhow!(error.clone()), properties)
        .await
    {
        Err(retry) => {
            if !ctx.is_live() {
                use crate::durable_host::concurrent::DurableContext;
                ctx.publish_direct_replay_tail(begin).await?;
            }
            Err(retry)
        }
        Ok(()) => {
            if !ctx.is_live() {
                ctx.state.replay_state.abandon_replay_for_failure().await?;
                ctx.state
                    .authoritative_replay_failure
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Err(anyhow!(ClassifiedHostError {
                kind: HostFailureKind::Permanent,
                message: format!(
                    "transaction status resolution exhausted at Start {begin}: {error}"
                ),
            }))
        }
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
    let _: Result<(), RdbmsError> =
        db_transaction_terminal(ctx, &entry, TransactionTerminal::Drop).await?;
    ctx.table().delete(entry)?;
    Ok(())
}

pub struct RdbmsConnection<T: RdbmsType> {
    pool_key: RdbmsPoolKey,
    initialized: bool,
    _owner: PhantomData<T>,
}

impl<T: RdbmsType> RdbmsConnection<T> {
    fn new(pool_key: RdbmsPoolKey) -> Self {
        Self {
            pool_key,
            initialized: false,
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
    Lost,
    Open(Arc<dyn crate::services::rdbms::DbResultStream<T> + Send + Sync>),
}

fn rdbms_stream_lost() -> RdbmsError {
    RdbmsError::ResourceLost("Database result stream was lost during reconstruction".to_string())
}

fn rdbms_transaction_lost() -> RdbmsError {
    RdbmsError::ResourceLost("Database transaction was lost during reconstruction".to_string())
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
        RdbmsResultStreamState::Lost => Err(rdbms_stream_lost()),
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
    persisted: bool,
    replay_drop:
        Option<Arc<std::sync::Mutex<Option<crate::durable_host::concurrent::ReplayCallHandle>>>>,
    // ResourceTable iteration exposes values without handles. Retain the handle so
    // host cleanup can invoke the typed Drop path without allocating another slot.
    resource_rep: Option<u32>,
}

impl<T: RdbmsType + 'static> RdbmsTransactionEntry<T> {
    fn new(
        pool_key: RdbmsPoolKey,
        state: RdbmsTransactionState<T>,
        begin_index: OplogIndex,
        persisted: bool,
    ) -> Self {
        Self {
            pool_key,
            state,
            begin_index,
            persisted,
            replay_drop: None,
            resource_rep: None,
        }
    }

    fn resource(&self) -> anyhow::Result<Resource<Self>> {
        Ok(Resource::new_own(self.resource_rep.ok_or_else(|| {
            anyhow!("Transaction resource handle was not installed")
        })?))
    }

    fn set_closed(&mut self) {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => {
                self.state = RdbmsTransactionState::Closed(transaction.deref().transaction_id())
            }
            RdbmsTransactionState::Closed(_) => (),
            RdbmsTransactionState::Reconstructed(id) => {
                self.state = RdbmsTransactionState::Closed(id.clone())
            }
        }
    }

    fn transaction_id(&self) -> TransactionId {
        match &self.state {
            RdbmsTransactionState::Open(transaction) => transaction.deref().transaction_id(),
            RdbmsTransactionState::Closed(id) => id.clone(),
            RdbmsTransactionState::Reconstructed(id) => id.clone(),
        }
    }
}

#[derive(Clone)]
pub enum RdbmsTransactionState<T: RdbmsType + 'static> {
    Open(Arc<dyn crate::services::rdbms::DbTransaction<T> + Send + Sync>),
    /// Recorded outputs are replayable, but there is no resident connection. A prepared
    /// commit may still have succeeded; its outcome must be resolved separately.
    Reconstructed(TransactionId),
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
        RdbmsTransactionState::Reconstructed(_) => Err(rdbms_transaction_lost()),
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
            let transaction = ctx
                .table()
                .get(entry)
                .map_err(RdbmsError::other_response_failure)?;
            Ok(RdbmsRequest::<T>::new(
                transaction.pool_key.clone(),
                statement,
                params,
                Some(transaction.transaction_id()),
            ))
        }
        Err(error) => Err(RdbmsError::QueryParameterFailure(error)),
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
            RdbmsTransactionState::Reconstructed(transaction_id.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::oplog::payload::types::SerializableRdbmsError;
    use test_r::test;

    #[test]
    fn resource_loss_is_typed_and_permanent() {
        for error in [rdbms_transaction_lost(), rdbms_stream_lost()] {
            assert_eq!(classify_rdbms_error(&error), HostFailureKind::Permanent);
            let serialized: SerializableRdbmsError = error.clone().into();
            assert!(matches!(
                serialized,
                SerializableRdbmsError::ResourceLost(_)
            ));
            assert_eq!(RdbmsError::from(serialized), error);
            assert!(matches!(
                crate::preview2::golem::rdbms::postgres::Error::from(error),
                crate::preview2::golem::rdbms::postgres::Error::ResourceLost(_)
            ));
        }
        assert_eq!(
            classify_rdbms_error(&RdbmsError::ConnectionFailure("unavailable".into())),
            HostFailureKind::Transient
        );
    }
}
