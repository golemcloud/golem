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

pub mod component;
pub mod worker;
pub mod worker_request_executor;

use golem_service_base::config::BlobStorageConfig;
use golem_service_base::service::initial_component_files::InitialComponentFilesService;
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::sqlite::SqlitePool;
use golem_worker_service_base::gateway_execution::file_server_binding_handler::DefaultFileServerBindingHandler;
use golem_worker_service_base::gateway_execution::file_server_binding_handler::FileServerBindingHandler;
use worker_request_executor::UnauthorisedWorkerRequestExecutor;

use golem_worker_service_base::gateway_api_definition::http::{
    CompiledHttpApiDefinition, HttpApiDefinition,
};

use golem_service_base::auth::{DefaultNamespace, EmptyAuthCtx};
use golem_worker_service_base::app_config::{GatewaySessionStorageConfig, WorkerServiceBaseConfig};

use golem_worker_service_base::gateway_execution::api_definition_lookup::{
    ApiDefinitionsLookup, HttpApiDefinitionLookup,
};
use golem_worker_service_base::gateway_execution::GatewayWorkerRequestExecutor;
use golem_worker_service_base::repo::api_definition;
use golem_worker_service_base::repo::api_deployment;
use golem_worker_service_base::service::component::RemoteComponentService;
use golem_worker_service_base::service::gateway::api_definition::{
    ApiDefinitionService, ApiDefinitionServiceDefault,
};
use golem_worker_service_base::service::gateway::api_definition_validator::ApiDefinitionValidatorService;
use golem_worker_service_base::service::gateway::http_api_definition_validator::HttpApiDefinitionValidator;
use golem_worker_service_base::service::worker::WorkerServiceDefault;

use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::client::{GrpcClientConfig, MultiTargetGrpcClient};
use golem_common::model::RetryConfig;

use golem_common::config::DbConfig;
use golem_common::redis::RedisPool;
use golem_service_base::db;
use golem_worker_service_base::gateway_execution::gateway_session::{
    GatewaySession, RedisGatewaySession, RedisGatewaySessionExpiration, SqliteGatewaySession,
    SqliteGatewaySessionExpiration,
};
use golem_worker_service_base::gateway_request::http_request::InputHttpRequest;
use golem_worker_service_base::gateway_security::DefaultIdentityProvider;
use golem_worker_service_base::repo::security_scheme::{DbSecuritySchemeRepo, SecuritySchemeRepo};
use golem_worker_service_base::service::gateway::api_deployment::{
    ApiDeploymentService, ApiDeploymentServiceDefault,
};
use golem_worker_service_base::service::gateway::security_scheme::{
    DefaultSecuritySchemeService, SecuritySchemeService,
};
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;

#[derive(Clone)]
pub struct Services {
    pub worker_service: worker::WorkerService,
    pub component_service: component::ComponentService,
    pub security_scheme_service: Arc<dyn SecuritySchemeService<DefaultNamespace> + Sync + Send>,
    pub definition_service:
        Arc<dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
    pub deployment_service:
        Arc<dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send>,
    pub http_definition_lookup_service: Arc<
        dyn ApiDefinitionsLookup<
                InputHttpRequest,
                ApiDefinition = CompiledHttpApiDefinition<DefaultNamespace>,
            > + Sync
            + Send,
    >,
    pub worker_to_http_service:
        Arc<dyn GatewayWorkerRequestExecutor<DefaultNamespace> + Sync + Send>,
    pub gateway_session_store: Arc<dyn GatewaySession + Sync + Send>,
    pub api_definition_validator_service:
        Arc<dyn ApiDefinitionValidatorService<HttpApiDefinition> + Sync + Send>,
    pub fileserver_binding_handler:
        Arc<dyn FileServerBindingHandler<DefaultNamespace> + Sync + Send>,
}

impl Services {
    pub async fn new(config: &WorkerServiceBaseConfig) -> Result<Services, String> {
        let routing_table_service: Arc<
            dyn golem_service_base::service::routing_table::RoutingTableService + Send + Sync,
        > = Arc::new(
            golem_service_base::service::routing_table::RoutingTableServiceDefault::new(
                config.routing_table.clone(),
            ),
        );

        let worker_executor_grpc_clients = MultiTargetGrpcClient::new(
            "worker_executor",
            |channel| {
                WorkerExecutorClient::new(channel)
                    .send_compressed(CompressionEncoding::Gzip)
                    .accept_compressed(CompressionEncoding::Gzip)
            },
            GrpcClientConfig {
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

        let component_service: component::ComponentService = {
            let config = &config.component_service;
            let uri = config.uri();
            let retry_config = config.retries.clone();

            Arc::new(RemoteComponentService::new(uri, retry_config))
        };

        let worker_service: worker::WorkerService = Arc::new(WorkerServiceDefault::new(
            worker_executor_grpc_clients.clone(),
            config.worker_executor_retries.clone(),
            routing_table_service.clone(),
        ));

        let worker_to_http_service: Arc<
            dyn GatewayWorkerRequestExecutor<DefaultNamespace> + Sync + Send,
        > = Arc::new(UnauthorisedWorkerRequestExecutor::new(
            worker_service.clone(),
        ));

        let gateway_session_store: Arc<dyn GatewaySession + Sync + Send> =
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

        let (api_definition_repo, api_deployment_repo, security_scheme_repo) = match config
            .db
            .clone()
        {
            DbConfig::Postgres(c) => {
                let db_pool = db::create_postgres_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
                    Arc::new(api_definition::LoggedApiDefinitionRepo::new(
                        api_definition::DbApiDefinitionRepo::new(db_pool.clone().into()),
                    ));
                let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
                    Arc::new(api_deployment::LoggedDeploymentRepo::new(
                        api_deployment::DbApiDeploymentRepo::new(db_pool.clone().into()),
                    ));

                let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> =
                    Arc::new(DbSecuritySchemeRepo::new(db_pool.clone().into()));

                (
                    api_definition_repo,
                    api_deployment_repo,
                    security_scheme_repo,
                )
            }
            DbConfig::Sqlite(c) => {
                let db_pool = db::create_sqlite_pool(&c)
                    .await
                    .map_err(|e| e.to_string())?;
                let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
                    Arc::new(api_definition::LoggedApiDefinitionRepo::new(
                        api_definition::DbApiDefinitionRepo::new(db_pool.clone().into()),
                    ));
                let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
                    Arc::new(api_deployment::LoggedDeploymentRepo::new(
                        api_deployment::DbApiDeploymentRepo::new(db_pool.clone().into()),
                    ));

                let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> =
                    Arc::new(DbSecuritySchemeRepo::new(db_pool.clone().into()));

                (
                    api_definition_repo,
                    api_deployment_repo,
                    security_scheme_repo,
                )
            }
        };

        let blob_storage: Arc<dyn BlobStorage + Sync + Send> = match &config.blob_storage {
            BlobStorageConfig::S3(config) => Arc::new(
                golem_service_base::storage::blob::s3::S3BlobStorage::new(config.clone()).await,
            ),
            BlobStorageConfig::LocalFileSystem(config) => Arc::new(
                golem_service_base::storage::blob::fs::FileSystemBlobStorage::new(&config.root)
                    .await?,
            ),
            BlobStorageConfig::Sqlite(sqlite) => {
                let pool = SqlitePool::configured(sqlite)
                    .await
                    .map_err(|e| format!("Failed to create sqlite pool: {}", e))?;
                Arc::new(
                    golem_service_base::storage::blob::sqlite::SqliteBlobStorage::new(pool.clone())
                        .await?,
                )
            }
            BlobStorageConfig::InMemory => {
                Arc::new(golem_service_base::storage::blob::memory::InMemoryBlobStorage::new())
            }
            _ => {
                return Err("Unsupported blob storage configuration".to_string());
            }
        };

        let initial_component_files_service: Arc<InitialComponentFilesService> =
            Arc::new(InitialComponentFilesService::new(blob_storage.clone()));

        let fileserver_binding_handler: Arc<
            dyn FileServerBindingHandler<DefaultNamespace> + Sync + Send,
        > = Arc::new(DefaultFileServerBindingHandler::new(
            component_service.clone(),
            initial_component_files_service.clone(),
            worker_service.clone(),
        ));

        let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let security_scheme_service = Arc::new(DefaultSecuritySchemeService::new(
            security_scheme_repo,
            identity_provider,
        ));

        let definition_service: Arc<
            dyn ApiDefinitionService<EmptyAuthCtx, DefaultNamespace> + Sync + Send,
        > = Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_deployment_repo.clone(),
            security_scheme_service.clone(),
            api_definition_validator_service.clone(),
        ));

        let deployment_service: Arc<
            dyn ApiDeploymentService<EmptyAuthCtx, DefaultNamespace> + Sync + Send,
        > = Arc::new(ApiDeploymentServiceDefault::new(
            api_deployment_repo.clone(),
            api_definition_repo.clone(),
            component_service.clone(),
        ));

        let http_definition_lookup_service =
            Arc::new(HttpApiDefinitionLookup::new(deployment_service.clone()));

        Ok(Services {
            worker_service,
            definition_service,
            security_scheme_service,
            deployment_service,
            http_definition_lookup_service,
            worker_to_http_service,
            component_service,
            api_definition_validator_service,
            fileserver_binding_handler,
            gateway_session_store,
        })
    }
}
