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

use crate::config::{SessionStoreConfig, WebSocketTransportLimits, WorkerServiceConfig};
use crate::custom_api::api_definition_lookup::{
    HttpApiDefinitionsLookup, RegistryServiceApiDefinitionsLookup,
};
use crate::custom_api::call_agent::CallAgentHandler;
use crate::custom_api::durable_streams::DurableStreamsHandler;
use crate::custom_api::oidc::handler::OidcHandler;
use crate::custom_api::oidc::session_store::{
    PostgresSessionStore, RedisSessionStore, SessionStore, SqliteSessionStore,
    migrated_postgres_pool, migrated_sqlite_pool,
};
use crate::custom_api::oidc::{DefaultIdentityProvider, IdentityProvider};
use crate::custom_api::request_handler::RequestHandler;
use crate::custom_api::route_resolver::RouteResolver;
use crate::custom_api::webhooks::WebhookCallbackHandler;
use crate::invocation_session_token::InvocationSessionTokenKeyring;
use crate::mcp::session::store::{
    McpSessionStore, PostgresMcpSessionStore, RedisMcpSessionStore, SqliteMcpSessionStore,
};
use crate::mcp::session::{
    McpPostgresStorageConfig, McpSessionConfig, McpSessionStorageConfig, McpSqliteStorageConfig,
};
use crate::mcp::{McpCapabilityLookup, RegistryServiceMcpCapabilityLookup};
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::agent_types::AgentTypeResolver;
use crate::service::auth::{AuthService, RemoteAuthService};
use crate::service::component::{ComponentService, RemoteComponentService};
use crate::service::limit::{LimitService, RemoteLimitService};
use crate::service::worker::{WorkerClient, WorkerExecutorWorkerClient, WorkerService};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::redis::RedisPool;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::clients::shard_manager::{GrpcShardManager, ShardManager};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::grpc::client::MultiTargetGrpcClient;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::service::routing_table::RoutingTableService;
use golem_service_base::storage::blob::s3::{
    ConfiguredS3BlobStorageFactory, S3BlobStorageFactory, S3StartupNamespace,
};
use golem_service_base::storage::blob::{
    BlobStorage, fs::FileSystemBlobStorage, memory::InMemoryBlobStorage, sqlite::SqliteBlobStorage,
};
use std::sync::Arc;
use tonic::codec::CompressionEncoding;

#[derive(Clone)]
pub struct Services {
    pub auth_service: Arc<dyn AuthService>,
    pub limit_service: Arc<dyn LimitService>,
    pub component_service: Arc<dyn ComponentService>,
    pub worker_service: Arc<WorkerService>,
    pub connect_websocket: WebSocketTransportLimits,
    pub request_handler: Arc<RequestHandler>,
    pub openapi_service: Arc<crate::custom_api::openapi::OpenApiService>,
    pub mcp_capability_lookup: Arc<dyn McpCapabilityLookup + Sync + Send + 'static>,
    pub registry_service: Arc<dyn RegistryService>,
    pub agent_resolution_cache: Arc<AgentResolutionCache>,
    pub agent_types: Arc<AgentTypeResolver>,
    pub route_resolver: Arc<RouteResolver>,
    pub identity_provider: Arc<dyn IdentityProvider>,
    pub session_store: Arc<dyn SessionStore>,
    pub mcp_session_store: Arc<dyn McpSessionStore>,
    pub invocation_session_token_keyring: Arc<InvocationSessionTokenKeyring>,
    pub invocation_session_config: crate::config::InvocationSessionConfig,
    pub mcp_config: crate::config::McpConfig,
    /// The installation's Golem issuer, which the registry keeps.
    pub golem_issuer: GolemIssuer,
}

impl Services {
    pub async fn new(
        config: &WorkerServiceConfig,
        golem_issuer: GolemIssuer,
    ) -> anyhow::Result<Self> {
        Self::new_with_blob_storage_factory(
            config,
            golem_issuer,
            ConfiguredS3BlobStorageFactory::default(),
        )
        .await
    }

    async fn new_with_blob_storage_factory(
        config: &WorkerServiceConfig,
        golem_issuer: GolemIssuer,
        blob_storage_factory: impl S3BlobStorageFactory,
    ) -> anyhow::Result<Self> {
        config.http_session.validate().map_err(anyhow::Error::msg)?;
        config.validate_keep_alive().map_err(anyhow::Error::msg)?;
        config.mcp.session.validate().map_err(anyhow::Error::msg)?;
        config
            .worker_executor
            .validate()
            .map_err(anyhow::Error::msg)?;
        config.blob_storage.validate()?;
        let blob_storage: Arc<dyn BlobStorage> = match &config.blob_storage {
            BlobStorageConfig::S3(config) => {
                blob_storage_factory
                    .create(config.clone(), &[S3StartupNamespace::InitialAgentFiles])
                    .await?
            }
            BlobStorageConfig::LocalFileSystem(config) => {
                Arc::new(FileSystemBlobStorage::new(&config.root).await?)
            }
            BlobStorageConfig::Sqlite(config) => {
                Arc::new(SqliteBlobStorage::new(SqlitePool::configured(config).await?).await?)
            }
            BlobStorageConfig::InMemory(_) => Arc::new(InMemoryBlobStorage::new()),
            BlobStorageConfig::KVStoreSqlite(_) => {
                anyhow::bail!("Worker-service does not support KVStoreSqlite blob storage")
            }
        };
        let initial_files = Arc::new(InitialAgentFilesService::new(blob_storage));
        let invocation_session_token_keyring = Arc::new(
            InvocationSessionTokenKeyring::new(&config.invocation_session_tokens)
                .map_err(anyhow::Error::msg)?,
        );
        let registry_service_client: Arc<dyn RegistryService> =
            Arc::new(GrpcRegistryService::new(&config.registry_service));

        let auth_service: Arc<dyn AuthService> = Arc::new(RemoteAuthService::new(
            registry_service_client.clone(),
            &config.auth_service,
        ));

        let component_service: Arc<dyn ComponentService> = Arc::new(RemoteComponentService::new(
            registry_service_client.clone(),
            &config.component_service,
        ));

        let limit_service: Arc<dyn LimitService> =
            Arc::new(RemoteLimitService::new(registry_service_client.clone()));

        let shard_manager_service: Arc<dyn ShardManager> =
            Arc::new(GrpcShardManager::new(&config.shard_manager));

        let routing_table_service: Arc<RoutingTableService> = Arc::new(RoutingTableService::new(
            config.routing_table.clone(),
            shard_manager_service,
        ));

        let worker_executor_clients = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel, max_message_size| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
                    .max_decoding_message_size(max_message_size)
                    .max_encoding_message_size(max_message_size)
            },
            config.worker_executor.client.clone(),
        );

        let worker_client: Arc<dyn WorkerClient> = Arc::new(WorkerExecutorWorkerClient::new(
            worker_executor_clients.clone(),
            config.worker_executor.retries.clone(),
            routing_table_service.clone(),
            config.worker_executor.max_concurrent_oplog_entry_reads,
        ));

        let agent_resolution_cache = Arc::new(AgentResolutionCache::new(
            registry_service_client.clone(),
            config.agent_resolution_cache.max_capacity,
            config.agent_resolution_cache.ttl,
            config.agent_resolution_cache.eviction_period,
        ));

        let agent_types = Arc::new(AgentTypeResolver::new(
            registry_service_client.clone(),
            config.agent_resolution_cache.max_capacity,
            config.agent_resolution_cache.ttl,
            config.agent_resolution_cache.eviction_period,
        ));

        let worker_service: Arc<WorkerService> = Arc::new(WorkerService::new(
            component_service.clone(),
            auth_service.clone(),
            limit_service.clone(),
            worker_client.clone(),
            agent_resolution_cache.clone(),
            agent_types.clone(),
            golem_issuer.clone(),
        ));

        let api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup> = Arc::new(
            RegistryServiceApiDefinitionsLookup::new(registry_service_client.clone()),
        );

        let route_resolver = Arc::new(RouteResolver::new(
            &config.route_resolver,
            api_definition_lookup_service.clone(),
        ));

        let mcp_capability_lookup = Arc::new(RegistryServiceMcpCapabilityLookup::new(
            registry_service_client.clone(),
        ));

        let call_agent_handler = Arc::new(CallAgentHandler::new(worker_service.clone()));
        let durable_streams_handler = Arc::new(DurableStreamsHandler::new(
            worker_service.clone(),
            call_agent_handler.clone(),
            &config.durable_streams,
        ));

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let (session_store, mcp_session_store) = session_stores(
            &config.gateway_session_storage,
            &config.mcp.session,
            &MigratedConnections,
        )
        .await?;

        let oidc_handler = Arc::new(OidcHandler::new(
            session_store.clone(),
            identity_provider.clone(),
        ));

        let webhook_callback_handler = Arc::new(WebhookCallbackHandler::new(
            worker_service.clone(),
            config.webhook_callback_handler.hmac_key.0.clone(),
        ));

        let openapi_service = Arc::new(crate::custom_api::openapi::OpenApiService::new(
            worker_service.clone(),
        ));
        let request_handler = Arc::new(RequestHandler::new(
            route_resolver.clone(),
            call_agent_handler.clone(),
            durable_streams_handler,
            oidc_handler.clone(),
            webhook_callback_handler.clone(),
            worker_service.clone(),
            config.http_session.clone(),
            initial_files,
            openapi_service.clone(),
            config.body_limits.http_api,
        ));

        Ok(Self {
            auth_service,
            limit_service,
            component_service,
            worker_service,
            connect_websocket: config.connect_websocket.clone(),
            request_handler,
            openapi_service,
            mcp_capability_lookup,
            registry_service: registry_service_client,
            agent_resolution_cache,
            agent_types,
            route_resolver,
            identity_provider,
            session_store,
            mcp_session_store,
            invocation_session_token_keyring,
            invocation_session_config: config.invocation_session.clone(),
            mcp_config: config.mcp.clone(),
            golem_issuer,
        })
    }
}

/// Opens the backend connections of the session stores. Database pools hold the session store
/// tables.
#[async_trait]
trait StoreConnector: Send + Sync {
    async fn redis(&self, config: &RedisConfig) -> anyhow::Result<RedisPool>;
    async fn sqlite(&self, config: &DbSqliteConfig) -> anyhow::Result<SqlitePool>;
    async fn postgres(&self, config: &DbPostgresConfig) -> anyhow::Result<PostgresPool>;
}

struct MigratedConnections;

#[async_trait]
#[golem_async_trait::region_simple]
impl StoreConnector for MigratedConnections {
    async fn redis(&self, config: &RedisConfig) -> anyhow::Result<RedisPool> {
        Ok(RedisPool::configured(config).await?)
    }

    async fn sqlite(&self, config: &DbSqliteConfig) -> anyhow::Result<SqlitePool> {
        migrated_sqlite_pool(config).await
    }

    async fn postgres(&self, config: &DbPostgresConfig) -> anyhow::Result<PostgresPool> {
        migrated_postgres_pool(config).await
    }
}

/// The gateway session store and the MCP session store. The MCP store uses the gateway store's
/// pool when its effective connection equals the gateway's.
async fn session_stores(
    gateway: &SessionStoreConfig,
    mcp: &McpSessionConfig,
    connect: &dyn StoreConnector,
) -> anyhow::Result<(Arc<dyn SessionStore>, Arc<dyn McpSessionStore>)> {
    let mcp = mcp.effective_session_storage(gateway);
    match gateway {
        SessionStoreConfig::Redis(inner) => {
            let redis = connect.redis(&inner.redis_config).await?;
            let session_store = Arc::new(RedisSessionStore::new(
                redis.clone(),
                fred::types::Expiration::EX(inner.pending_login_expiration.as_secs().try_into()?),
            ));
            let mcp_session_store = match &mcp {
                McpSessionStorageConfig::Redis(config) => {
                    let shared = (*config == inner.redis_config).then_some(redis);
                    redis_mcp_session_store(config, shared, connect).await?
                }
                other => unshared_mcp_session_store(other, connect).await?,
            };
            Ok((session_store, mcp_session_store))
        }
        SessionStoreConfig::Sqlite(inner) => {
            let pool = connect.sqlite(&inner.sqlite_config).await?;
            let session_store = Arc::new(SqliteSessionStore::new(
                pool.clone(),
                inner.pending_login_expiration,
                inner.cleanup_interval,
            )?);
            let mcp_session_store = match &mcp {
                McpSessionStorageConfig::Sqlite(config) => {
                    let shared = (config.sqlite_config == inner.sqlite_config).then_some(pool);
                    sqlite_mcp_session_store(config, shared, connect).await?
                }
                other => unshared_mcp_session_store(other, connect).await?,
            };
            Ok((session_store, mcp_session_store))
        }
        SessionStoreConfig::Postgres(inner) => {
            let postgres_config = inner.postgres_config_with_schema();
            let pool = connect.postgres(&postgres_config).await?;
            let session_store = Arc::new(PostgresSessionStore::new(
                pool.clone(),
                inner.pending_login_expiration,
                inner.cleanup_interval,
            )?);
            let mcp_session_store = match &mcp {
                McpSessionStorageConfig::Postgres(config) => {
                    let shared =
                        (config.postgres_config_with_schema() == postgres_config).then_some(pool);
                    postgres_mcp_session_store(config, shared, connect).await?
                }
                other => unshared_mcp_session_store(other, connect).await?,
            };
            Ok((session_store, mcp_session_store))
        }
    }
}

async fn unshared_mcp_session_store(
    config: &McpSessionStorageConfig,
    connect: &dyn StoreConnector,
) -> anyhow::Result<Arc<dyn McpSessionStore>> {
    match config {
        McpSessionStorageConfig::Redis(config) => {
            redis_mcp_session_store(config, None, connect).await
        }
        McpSessionStorageConfig::Sqlite(config) => {
            sqlite_mcp_session_store(config, None, connect).await
        }
        McpSessionStorageConfig::Postgres(config) => {
            postgres_mcp_session_store(config, None, connect).await
        }
    }
}

async fn redis_mcp_session_store(
    config: &RedisConfig,
    shared: Option<RedisPool>,
    connect: &dyn StoreConnector,
) -> anyhow::Result<Arc<dyn McpSessionStore>> {
    let redis = match shared {
        Some(redis) => redis,
        None => connect.redis(config).await?,
    };
    Ok(Arc::new(RedisMcpSessionStore::new(redis)))
}

async fn sqlite_mcp_session_store(
    config: &McpSqliteStorageConfig,
    shared: Option<SqlitePool>,
    connect: &dyn StoreConnector,
) -> anyhow::Result<Arc<dyn McpSessionStore>> {
    let pool = match shared {
        Some(pool) => pool,
        None => connect.sqlite(&config.sqlite_config).await?,
    };
    Ok(Arc::new(SqliteMcpSessionStore::new(
        pool,
        config.cleanup_interval,
    )))
}

async fn postgres_mcp_session_store(
    config: &McpPostgresStorageConfig,
    shared: Option<PostgresPool>,
    connect: &dyn StoreConnector,
) -> anyhow::Result<Arc<dyn McpSessionStore>> {
    let pool = match shared {
        Some(pool) => pool,
        None => {
            connect
                .postgres(&config.postgres_config_with_schema())
                .await?
        }
    };
    Ok(Arc::new(PostgresMcpSessionStore::new(
        pool,
        config.cleanup_interval,
    )))
}

#[cfg(test)]
mod tests;
