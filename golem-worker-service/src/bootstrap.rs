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

use crate::config::{SessionStoreConfig, WorkerServiceConfig};
use crate::custom_api::api_definition_lookup::{
    HttpApiDefinitionsLookup, RegistryServiceApiDefinitionsLookup,
};
use crate::custom_api::call_agent::CallAgentHandler;
use crate::custom_api::oidc::DefaultIdentityProvider;
use crate::custom_api::oidc::handler::OidcHandler;
use crate::custom_api::oidc::session_store::{RedisSessionStore, SessionStore, SqliteSessionStore};
use crate::custom_api::request_handler::RequestHandler;
use crate::custom_api::route_resolver::RouteResolver;
use crate::custom_api::webhoooks::WebhookCallbackHandler;
use crate::mcp::{McpCapabilityLookup, RegistryServiceMcpCapabilityLookup};
use crate::service::auth::{AuthService, RemoteAuthService};
use crate::service::component::{ComponentService, RemoteComponentService};
use crate::service::limit::{LimitService, RemoteLimitService};
use crate::service::worker::{WorkerClient, WorkerExecutorWorkerClient, WorkerService};
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use golem_common::redis::RedisPool;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::grpc::client::MultiTargetGrpcClient;
use golem_service_base::service::routing_table::{RoutingTableService, RoutingTableServiceDefault};
use std::sync::Arc;
use tonic::codec::CompressionEncoding;

#[derive(Clone)]
pub struct Services {
    pub auth_service: Arc<dyn AuthService>,
    pub limit_service: Arc<dyn LimitService>,
    pub component_service: Arc<dyn ComponentService>,
    pub worker_service: Arc<WorkerService>,
    pub request_handler: Arc<RequestHandler>,
    pub agents_service: Arc<AgentsService>,
    pub mcp_capability_lookup: Arc<dyn McpCapabilityLookup + Sync + Send + 'static>,
}

impl Services {
    pub async fn new(config: &WorkerServiceConfig) -> anyhow::Result<Self> {
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
            config.worker_executor.client.clone(),
        );

        let worker_client: Arc<dyn WorkerClient> = Arc::new(WorkerExecutorWorkerClient::new(
            worker_executor_clients.clone(),
            config.worker_executor.retries.clone(),
            routing_table_service.clone(),
        ));

        let worker_service: Arc<WorkerService> = Arc::new(WorkerService::new(
            registry_service_client.clone(),
            component_service.clone(),
            auth_service.clone(),
            limit_service.clone(),
            worker_client.clone(),
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

        let identity_provider = Arc::new(DefaultIdentityProvider);

        let session_store: Arc<dyn SessionStore> = match &config.gateway_session_storage {
            SessionStoreConfig::Redis(inner) => {
                let redis = RedisPool::configured(&inner.redis_config).await?;

                let session_store = RedisSessionStore::new(
                    redis,
                    fred::types::Expiration::EX(
                        inner.pending_login_expiration.as_secs().try_into()?,
                    ),
                );

                Arc::new(session_store)
            }

            SessionStoreConfig::Sqlite(inner) => {
                let pool = SqlitePool::configured(&inner.sqlite_config).await?;

                let gateway_session_with_sqlite = SqliteSessionStore::new(
                    pool,
                    inner.pending_login_expiration.as_secs().try_into()?,
                    inner.cleanup_interval,
                )
                .await?;

                Arc::new(gateway_session_with_sqlite)
            }
        };

        let oidc_handler = Arc::new(OidcHandler::new(
            session_store.clone(),
            identity_provider.clone(),
        ));

        let webhook_callback_handler = Arc::new(WebhookCallbackHandler::new(
            worker_service.clone(),
            config.webhook_callback_handler.hmac_key.0.clone(),
        ));

        let request_handler = Arc::new(RequestHandler::new(
            route_resolver.clone(),
            call_agent_handler.clone(),
            oidc_handler.clone(),
            webhook_callback_handler.clone(),
        ));

        Ok(Self {
            auth_service,
            limit_service,
            component_service,
            worker_service,
            request_handler,
            mcp_capability_lookup
        })
    }
}
