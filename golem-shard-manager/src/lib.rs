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

pub mod config;
pub mod error;
mod grpc;
mod quota;
mod registry_event_subscriber;
pub(crate) mod sharding;

use self::grpc::ShardManagerServiceImpl;
use self::sharding::worker_executor::WorkerExecutorService;
use crate::config::{HealthCheckK8sConfig, HealthCheckMode};
use crate::quota::{DbQuotaRepo, GrpcResourceDefinitionFetcher, QuotaService};
use crate::registry_event_subscriber::ShardManagerRegistryInvalidationHandler;
use crate::sharding::healthcheck::{GrpcHealthCheck, HealthCheck};
use crate::sharding::shard_management::ShardManagement;
use crate::sharding::worker_executor::WorkerExecutorServiceDefault;
use config::ShardManagerConfig;
use futures::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerServiceServer;
use golem_service_base::clients::registry::GrpcRegistryService;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use include_dir::include_dir;
use prometheus::Registry;
pub use sharding::persistence::{DbRoutingTablePersistence, RoutingTablePersistence};
pub use sharding::{PodState, RoutingTable, RoutingTableEntry};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::Instrument;
use tracing::{debug, info};

#[cfg(test)]
test_r::enable!();

pub static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
}

pub async fn run(
    shard_manager_config: &ShardManagerConfig,
    registry: Registry,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing shard manager");

    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<ShardManagerServiceServer<ShardManagerServiceImpl>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build_v1()?;

    let http_port = golem_service_base::observability::start_health_and_metrics_server(
        SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), shard_manager_config.http_port),
        registry,
        "shard manager is running",
        join_set,
    )
    .await?;

    let shard_manager_config = Arc::new(shard_manager_config.clone());

    let (persistence_service, quota_repo): (
        Arc<dyn RoutingTablePersistence>,
        Arc<dyn crate::quota::QuotaRepo>,
    ) = {
        use golem_common::config::DbConfig;
        use golem_service_base::db;
        use golem_service_base::migration::{IncludedMigrationsDir, Migrations};
        use include_dir::include_dir;

        static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");
        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

        match &shard_manager_config.db {
            DbConfig::Postgres(postgres) => {
                db::postgres::migrate(postgres, migrations.postgres_migrations()).await?;
                let pool =
                    golem_service_base::db::postgres::PostgresPool::configured(postgres).await?;

                let persistence = Arc::new(
                    crate::sharding::persistence::DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    ),
                );
                let quota_repo = Arc::new(DbQuotaRepo::logged(pool));
                (persistence, quota_repo)
            }
            DbConfig::Sqlite(sqlite) => {
                db::sqlite::migrate(sqlite, migrations.sqlite_migrations()).await?;
                let pool = golem_service_base::db::sqlite::SqlitePool::configured(sqlite).await?;

                let persistence = Arc::new(
                    crate::sharding::persistence::DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    ),
                );
                let quota_repo = Arc::new(DbQuotaRepo::logged(pool));
                (persistence, quota_repo)
            }
        }
    };
    let worker_executors = Arc::new(WorkerExecutorServiceDefault::new(
        shard_manager_config.worker_executors.clone(),
    ));

    let health_check: Arc<dyn HealthCheck> = match &shard_manager_config.health_check.mode {
        HealthCheckMode::Grpc(_) => Arc::new(GrpcHealthCheck::new(
            worker_executors.clone(),
            shard_manager_config.worker_executors.retries.clone(),
            shard_manager_config.health_check.silent,
        )),
        #[cfg(feature = "kubernetes")]
        HealthCheckMode::K8s(HealthCheckK8sConfig { namespace }) => Arc::new(
            crate::sharding::healthcheck::kubernetes::KubernetesHealthCheck::new(
                namespace.clone(),
                shard_manager_config.worker_executors.retries.clone(),
                shard_manager_config.health_check.silent,
            )
            .await
            .expect("Failed to initialize K8s health checker"),
        ),
    };

    let registry_service = Arc::new(GrpcRegistryService::new(
        &shard_manager_config.registry_service,
    ));

    let fetcher: Arc<dyn crate::quota::ResourceDefinitionFetcher> =
        Arc::new(GrpcResourceDefinitionFetcher::new(
            registry_service.clone(),
            &shard_manager_config.resource_definition_fetcher,
        ));

    let quota_service = QuotaService::new(
        shard_manager_config.quota.clone(),
        fetcher.clone(),
        quota_repo,
    );
    quota_service.restore_state().await?;

    join_set.spawn({
        let quota_service = quota_service.clone();
        async move {
            ShardManagerRegistryInvalidationHandler::run(registry_service, fetcher, quota_service)
                .await;
            Ok(())
        }
    });

    let shard_management = Arc::new(
        ShardManagement::new(
            persistence_service.clone(),
            worker_executors.clone(),
            health_check.clone(),
            shard_manager_config.rebalance_threshold,
            join_set,
        )
        .await?,
    );

    self::sharding::healthcheck_loop::start_health_check_loop(
        shard_management.clone(),
        health_check.clone(),
        &shard_manager_config.health_check,
        join_set,
    );

    let shard_manager = ShardManagerServiceImpl::new(shard_management, quota_service);

    let service = ShardManagerServiceServer::new(shard_manager);

    let listener = TcpListener::bind(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        shard_manager_config.grpc.port,
    ))
    .await?;

    let grpc_port = listener.local_addr()?.port();

    join_set.spawn({
        let mut server = Server::builder();

        if let GrpcServerTlsConfig::Enabled(tls) = &shard_manager_config.grpc.tls {
            server = server.tls_config(tls.to_tonic())?;
        }

        server
            .layer(middleware::server::OtelGrpcLayer::default().filter(filters::reject_healthcheck))
            .add_service(reflection_service)
            .add_service(
                service
                    .accept_compressed(CompressionEncoding::Gzip)
                    .send_compressed(CompressionEncoding::Gzip),
            )
            .add_service(health_service)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .map_err(anyhow::Error::from)
            .in_current_span()
    });

    info!("Started shard manager on ports: grpc: {grpc_port}");

    Ok(RunDetails {
        http_port,
        grpc_port,
    })
}
