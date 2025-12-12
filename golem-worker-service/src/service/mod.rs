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

pub mod auth;
pub mod component;
pub mod limit;
pub mod worker;

use self::auth::{AuthService, RemoteAuthService};
use self::component::RemoteComponentService;
use self::limit::{LimitService, RemoteLimitService};
use self::worker::WorkerService;
use crate::config::{GatewaySessionStorageConfig, WorkerServiceConfig};
use crate::gateway_execution::api_definition_lookup::{
    HttpApiDefinitionsLookup, RegistryServiceApiDefinitionsLookup,
};
use crate::gateway_execution::auth_call_back_binding_handler::{
    AuthCallBackBindingHandler, DefaultAuthCallBackBindingHandler,
};
use crate::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use crate::gateway_execution::gateway_http_input_executor::GatewayHttpInputExecutor;
use crate::gateway_execution::gateway_session_store::{
    GatewaySessionStore, RedisGatewaySession, RedisGatewaySessionExpiration, SqliteGatewaySession,
    SqliteGatewaySessionExpiration,
};
use crate::gateway_execution::http_handler_binding_handler::HttpHandlerBindingHandler;
use crate::gateway_execution::route_resolver::RouteResolver;
use crate::gateway_execution::GatewayWorkerRequestExecutor;
use crate::gateway_security::DefaultIdentityProvider;
use crate::service::component::ComponentService;
use crate::service::worker::{WorkerClient, WorkerExecutorWorkerClient};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::model::RetryConfig;
use golem_common::redis::RedisPool;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::service::routing_table::{RoutingTableService, RoutingTableServiceDefault};
use golem_service_base::storage::blob::BlobStorage;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;

#[derive(Clone)]
pub struct Services {
    pub auth_service: Arc<dyn AuthService>,
    pub limit_service: Arc<dyn LimitService>,
    pub component_service: Arc<dyn ComponentService>,
    pub worker_service: Arc<WorkerService>,
    pub gateway_http_input_executor: Arc<GatewayHttpInputExecutor>,
}

impl Services {
    pub async fn new(config: &WorkerServiceConfig) -> Result<Self, String> {
        let registry_service_client: Arc<dyn RegistryService> =
            Arc::new(GrpcRegistryService::new(&config.registry_service));

        let auth_service: Arc<dyn AuthService> = Arc::new(RemoteAuthService::new(
            registry_service_client.clone(),
            &config.auth_service,
        ));

        let gateway_session_store: Arc<dyn GatewaySessionStore> =
            match &config.gateway_session_storage {
                GatewaySessionStorageConfig::Redis(redis_config) => {
                    let redis = RedisPool::configured(redis_config)
                        .await
                        .map_err(|e| e.to_string())?;

                    let gateway_session_with_redis =
                        RedisGatewaySession::new(redis, RedisGatewaySessionExpiration::default());

                    Arc::new(gateway_session_with_redis)
                }

                GatewaySessionStorageConfig::Sqlite(sqlite_config) => {
                    let pool = SqlitePool::configured(sqlite_config)
                        .await
                        .map_err(|e| e.to_string())?;

                    let gateway_session_with_sqlite =
                        SqliteGatewaySession::new(pool, SqliteGatewaySessionExpiration::default())
                            .await?;

                    Arc::new(gateway_session_with_sqlite)
                }
            };

        let blob_storage: Arc<dyn BlobStorage> = match &config.blob_storage {
            BlobStorageConfig::S3(config) => Arc::new(
                golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await,
            ),
            BlobStorageConfig::LocalFileSystem(config) => Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            BlobStorageConfig::Sqlite(sqlite) => {
                let pool = SqlitePool::configured(sqlite)
                    .await
                    .map_err(|e| format!("Failed to create sqlite pool: {e}"))?;
                Arc::new(
                    golem_service_base::storage::blob::sqlite::SqliteBlobStorage::new(pool.clone())
                        .await
                        .map_err(|e| e.to_string())?,
                )
            }
            BlobStorageConfig::InMemory(_) => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let component_service: Arc<dyn ComponentService> = Arc::new(RemoteComponentService::new(
            registry_service_client.clone(),
            &config.component_service,
        ));

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let limit_service: Arc<dyn LimitService> =
            Arc::new(RemoteLimitService::new(registry_service_client.clone()));

        let routing_table_service: Arc<dyn RoutingTableService> = Arc::new(
            RoutingTableServiceDefault::new(config.routing_table.clone()),
        );

        let worker_executor_clients = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            GrpcClientConfig {
                // TODO
                retries_on_unavailable: RetryConfig {
                    max_attempts: 0, // we want to invalidate the routing table asap
                    min_delay: Duration::from_millis(100),
                    max_delay: Duration::from_secs(2),
                    multiplier: 2.0,
                    max_jitter_factor: Some(0.15),
                },
                connect_timeout: Duration::from_secs(10),
            },
        );

        let worker_client: Arc<dyn WorkerClient> = Arc::new(WorkerExecutorWorkerClient::new(
            worker_executor_clients.clone(),
            config.worker_executor_retries.clone(),
            routing_table_service.clone(),
        ));

        let worker_service: Arc<WorkerService> = Arc::new(WorkerService::new(
            component_service.clone(),
            auth_service.clone(),
            limit_service.clone(),
            worker_client.clone(),
        ));

        let gateway_worker_request_executor: Arc<GatewayWorkerRequestExecutor> = Arc::new(
            GatewayWorkerRequestExecutor::new(worker_service.clone(), component_service.clone()),
        );

        let file_server_binding_handler: Arc<FileServerBindingHandler> =
            Arc::new(FileServerBindingHandler::new(
                component_service.clone(),
                initial_component_files_service.clone(),
                worker_service.clone(),
            ));

        let auth_call_back_binding_handler: Arc<dyn AuthCallBackBindingHandler> =
            Arc::new(DefaultAuthCallBackBindingHandler::new(
                gateway_session_store.clone(),
                identity_provider.clone(),
            ));

        let http_handler_binding_handler: Arc<HttpHandlerBindingHandler> = Arc::new(
            HttpHandlerBindingHandler::new(gateway_worker_request_executor.clone()),
        );

        let api_definition_lookup_service: Arc<dyn HttpApiDefinitionsLookup> = Arc::new(
            RegistryServiceApiDefinitionsLookup::new(registry_service_client.clone()),
        );

        let route_resolver = Arc::new(RouteResolver::new(
            &config.route_resolver,
            api_definition_lookup_service.clone(),
        ));

        let gateway_http_input_executor: Arc<GatewayHttpInputExecutor> =
            Arc::new(GatewayHttpInputExecutor::new(
                route_resolver.clone(),
                gateway_worker_request_executor.clone(),
                file_server_binding_handler.clone(),
                auth_call_back_binding_handler.clone(),
                http_handler_binding_handler.clone(),
                gateway_session_store.clone(),
                identity_provider.clone(),
            ));

        Ok(Self {
            auth_service,
            limit_service,
            component_service,
            worker_service,
            gateway_http_input_executor,
        })
    }
}
