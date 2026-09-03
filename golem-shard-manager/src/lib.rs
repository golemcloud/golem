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
mod metrics;
mod quota;
mod registry_event_subscriber;
pub(crate) mod sharding;

use self::grpc::ShardManagerServiceImpl;
#[cfg(feature = "kubernetes")]
use crate::config::HealthCheckK8sConfig;
use crate::config::{EtcdConfig, HealthCheckMode, PersistenceConfig};
use crate::quota::{
    DbQuotaRepo, GrpcResourceDefinitionFetcher, QuotaService, UnavailableQuotaRepo,
};
use crate::registry_event_subscriber::ShardManagerRegistryInvalidationHandler;
use crate::sharding::etcd_connection::connect_for_requests;
use crate::sharding::etcd_retry::{is_retriable_read, retry_retriable};
use crate::sharding::healthcheck::GrpcHealthCheck;
use crate::sharding::worker_executor::WorkerExecutorServiceDefault;
#[cfg(feature = "kubernetes")]
use anyhow::Context;
use config::ShardManagerConfig;
use etcd_client::Client;
use futures::TryFutureExt;
use golem_api_grpc::proto;
use golem_api_grpc::proto::golem::shardmanager::v1::shard_manager_service_server::ShardManagerServiceServer;
use golem_service_base::clients::registry::GrpcRegistryService;
use golem_service_base::grpc::server::GrpcServerTlsConfig;
use include_dir::include_dir;
use prometheus::Registry;
pub use sharding::error::{HealthCheckError, ShardManagerError};
pub use sharding::healthcheck::HealthCheck;
pub use sharding::leader_election::{
    Elected, LEADER_ELECTION_NAME, LeaderElection, LeaderFence, LeadershipHandle, LeaseKeepAlive,
    LeaseLossReason, LeaseLost,
};
pub use sharding::persistence::{
    DbRoutingTablePersistence, EtcdRoutingTablePersistence, ExternalRevision, NO_REVISION,
    RoutingTablePersistence, STATE_KEY,
};
pub use sharding::shard_management::ShardManagement;
pub use sharding::worker_executor::WorkerExecutorService;
pub use sharding::{
    ExecutorAddr, ExecutorAddrs, ExecutorId, ExecutorLease, ExecutorShards, ShardAssignmentEntry,
    ShardEpoch, ShardLeaseRevision, ShardLeaseState,
};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic_tracing_opentelemetry::middleware;
use tonic_tracing_opentelemetry::middleware::filters;
use tracing::Instrument;
use tracing::{debug, error, info, warn};

#[cfg(test)]
test_r::enable!();

pub static DB_MIGRATIONS: include_dir::Dir = include_dir!("$CARGO_MANIFEST_DIR/db/migration");

pub struct RunDetails {
    pub http_port: u16,
    pub grpc_port: u16,
    pub leadership: Option<LeadershipHandle>,
}

/// Startup's persistence wiring; the leadership handle is present only in distributed mode.
type Persistence = (
    Arc<dyn RoutingTablePersistence>,
    Arc<dyn crate::quota::QuotaRepo>,
    Option<LeadershipHandle>,
);

/// Whether this process is a dedicated shard manager or is embedded in the single `golem` binary.
#[derive(Clone, Debug)]
pub enum Deployment {
    /// The `golem-shard-manager` binary. May block until elected.
    Standalone { shutdown: CancellationToken },
    /// The `golem` single binary. `run()` must return promptly.
    Embedded,
}

/// Campaigns for leadership, failing if an already-running task dies instead of waiting forever.
async fn campaign_watching_startup(
    election: &LeaderElection,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<Elected> {
    let mut campaign = std::pin::pin!(election.campaign_until_elected());

    loop {
        tokio::select! {
            elected = &mut campaign => return Ok(elected?),
            joined = join_set.join_next(), if !join_set.is_empty() => match joined {
                Some(Err(err)) => {
                    return Err(anyhow::Error::new(err)
                        .context("a shard manager task panicked while campaigning for leadership"));
                }
                Some(Ok(Err(err))) => {
                    return Err(err
                        .context("a shard manager task failed while campaigning for leadership"));
                }
                Some(Ok(Ok(()))) => {
                    warn!("A shard manager task finished while campaigning for leadership")
                }
                None => {}
            }
        }
    }
}

fn ensure_shard_count_matches(stored: usize, configured: usize) -> anyhow::Result<()> {
    anyhow::ensure!(
        stored == configured,
        "the persisted shard lease state was written with {stored} shards, but this shard manager \
         is configured for {configured}. The stored value governs routing, so starting would \
         silently ignore the configuration; changing the shard count is not supported."
    );
    Ok(())
}

/// Blocks until elected, returning the etcd client and the fence every state write is guarded on.
async fn start_distributed_mode(
    etcd: &EtcdConfig,
    deployment: &Deployment,
    number_of_shards: usize,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<(Client, LeaderFence, LeadershipHandle)> {
    let shutdown = match deployment {
        Deployment::Standalone { shutdown } => shutdown.clone(),
        Deployment::Embedded => anyhow::bail!(
            "etcd persistence selects distributed mode, which campaigns for \
             leadership and blocks until elected. The single-binary server starts the \
             shard manager inline and would never finish starting. Use Sqlite or \
             Postgres persistence for the embedded server."
        ),
    };

    anyhow::ensure!(
        etcd.leader_lease_ttl.subsec_nanos() == 0,
        "persistence.config.leader_lease_ttl must be a whole number of seconds, but \
         is {:?}; etcd would silently truncate it.",
        etcd.leader_lease_ttl
    );
    anyhow::ensure!(
        etcd.leader_lease_ttl >= Duration::from_secs(2),
        "persistence.config.leader_lease_ttl must be at least 2s (etcd's MinLeaseTTL), \
         but is {:?}; etcd would silently clamp it up.",
        etcd.leader_lease_ttl
    );
    anyhow::ensure!(
        etcd.request_timeout <= etcd.leader_lease_ttl / 2,
        "persistence.config.request_timeout must be at most half of \
         persistence.config.leader_lease_ttl, but is {:?} against a {:?} lease TTL; a lease could \
         then expire before its first renewal.",
        etcd.request_timeout,
        etcd.leader_lease_ttl
    );

    let kv = connect_for_requests(etcd).await?;
    info!(
        endpoints = etcd.endpoints.join(", "),
        state_key = STATE_KEY,
        "Configured the etcd client for shard lease state persistence"
    );

    metrics::record_standing_by();

    let stored_count = retry_retriable(
        "reading the stored shard count",
        || async {
            EtcdRoutingTablePersistence::stored_number_of_shards(&kv)
                .await
                .inspect_err(|err| {
                    if is_retriable_read(err) {
                        metrics::record_campaign_attempt_failure();
                    }
                })
        },
        &shutdown,
    )
    .await?;
    if let Some(stored) = stored_count {
        ensure_shard_count_matches(stored, number_of_shards)?;
    }

    let election = LeaderElection::connect(etcd, LEADER_ELECTION_NAME)
        .await?
        .with_shutdown(shutdown);
    let elected = campaign_watching_startup(&election, join_set).await?;

    metrics::record_elected(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default(),
    );

    // The keepalive can only end in an error, and that error ends the process: a lost lease must
    // stop this replica rather than leave it serving a table it can no longer write.
    let keepalive = elected.keepalive;
    let leadership = elected.leadership;
    let stepping_down = leadership.clone();
    join_set.spawn(
        async move {
            let lost = keepalive.run().await;
            metrics::record_standing_by();
            if stepping_down.has_stepped_down() {
                info!(error = %lost, "The etcd leadership lease ended after stepping down");
            } else {
                error!(error = %lost, "Lost etcd leadership");
            }
            Err(anyhow::Error::new(lost).context(
                "the shard manager lost its etcd leader lease; exiting so a standby \
                 can take over",
            ))
        }
        .in_current_span(),
    );

    Ok((kv, elected.fence, leadership))
}

pub async fn run(
    shard_manager_config: &ShardManagerConfig,
    deployment: Deployment,
    registry: Registry,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing shard manager");

    anyhow::ensure!(
        !shard_manager_config.shard_lease_duration.is_zero(),
        "shard_lease_duration must be greater than zero"
    );
    anyhow::ensure!(
        chrono::Duration::from_std(shard_manager_config.shard_lease_duration).is_ok(),
        "shard_lease_duration {:?} is out of range",
        shard_manager_config.shard_lease_duration
    );

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
        shard_manager_config.runtime_metrics_sampling_interval,
        "shard manager is running",
        join_set,
    )
    .await?;

    let shard_manager_config = Arc::new(shard_manager_config.clone());

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
            .context("failed to build the Kubernetes API client for the health checker")?,
        ),
    };

    let (persistence_service, quota_repo, leadership): Persistence = {
        use golem_service_base::db;
        use golem_service_base::migration::{IncludedMigrationsDir, Migrations};

        let migrations = IncludedMigrationsDir::new(&DB_MIGRATIONS);

        match &shard_manager_config.persistence {
            PersistenceConfig::Postgres(postgres) => {
                db::postgres::migrate(postgres, migrations.postgres_migrations()).await?;
                let pool = db::postgres::PostgresPool::configured(postgres).await?;

                let pool_for_metrics = pool.clone();
                join_set
                    .spawn(async move { pool_for_metrics.run_metrics_loop("shard_manager").await });

                (
                    Arc::new(DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    )),
                    Arc::new(DbQuotaRepo::logged(pool)),
                    None,
                )
            }
            PersistenceConfig::Sqlite(sqlite) => {
                db::sqlite::migrate(sqlite, migrations.sqlite_migrations()).await?;
                let pool = db::sqlite::SqlitePool::configured(sqlite).await?;

                (
                    Arc::new(DbRoutingTablePersistence::new(
                        pool.clone(),
                        shard_manager_config.number_of_shards,
                    )),
                    Arc::new(DbQuotaRepo::logged(pool)),
                    None,
                )
            }
            PersistenceConfig::Etcd(etcd) => {
                let (kv, fence, leadership) = start_distributed_mode(
                    etcd,
                    &deployment,
                    shard_manager_config.number_of_shards,
                    join_set,
                )
                .await?;

                // Distributed mode. The shard lease state is durable in etcd, but the quota
                // tables have not moved there and there is no SQL pool here to hold them, so
                // quota operations fail rather than silently succeeding against nothing.
                (
                    Arc::new(EtcdRoutingTablePersistence::with_client(
                        kv,
                        shard_manager_config.number_of_shards,
                        fence,
                    )),
                    Arc::new(UnavailableQuotaRepo),
                    Some(leadership),
                )
            }
        }
    };

    let startup = async {
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
                ShardManagerRegistryInvalidationHandler::run(
                    registry_service,
                    fetcher,
                    quota_service,
                )
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
                shard_manager_config.shard_lease_duration,
                join_set,
            )
            .await?,
        );

        ensure_shard_count_matches(
            shard_management.current_snapshot().await.number_of_shards,
            shard_manager_config.number_of_shards,
        )?;

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
                .layer(
                    middleware::server::OtelGrpcLayer::default()
                        .filter(filters::reject_healthcheck),
                )
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

        anyhow::Ok(grpc_port)
    };

    let started = match &deployment {
        Deployment::Standalone { shutdown } => tokio::select! {
            biased;
            started = startup => started,
            _ = shutdown.cancelled() => Err(anyhow::Error::new(ShardManagerError::ShutdownRequested)),
        },
        Deployment::Embedded => startup.await,
    };

    let grpc_port = match started {
        Ok(grpc_port) => grpc_port,
        Err(err) => {
            // Stop the tasks before releasing the leadership, so nothing writes state after a
            // standby can take over.
            join_set.abort_all();
            release_leadership(leadership.as_ref()).await;
            return Err(err);
        }
    };

    info!("Started shard manager on ports: grpc: {grpc_port}");

    Ok(RunDetails {
        http_port,
        grpc_port,
        leadership,
    })
}

/// A failed revoke is never propagated: the lease expires on its own, so failing the shutdown
/// would report a problem that resolves itself.
async fn release_leadership(leadership: Option<&LeadershipHandle>) {
    if let Some(leadership) = leadership
        && let Err(err) = leadership.step_down().await
    {
        warn!(
            error = %err,
            "Cannot release the shard manager leadership; the lease will expire on its own"
        );
    }
}

/// Runs the shard manager until a task fails, until every task has finished, or until `shutdown`
/// fires, releasing the leadership on the way out of all three.
pub async fn serve_until_stopped(
    details: RunDetails,
    mut join_set: JoinSet<anyhow::Result<()>>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            joined = join_set.join_next() => match joined {
                Some(Ok(Ok(()))) => warn!("A shard manager task finished"),
                Some(Ok(Err(err))) => {
                    join_set.abort_all();
                    release_leadership(details.leadership.as_ref()).await;
                    return Err(err.context("a shard manager task failed"));
                }
                Some(Err(panicked)) => {
                    join_set.abort_all();
                    release_leadership(details.leadership.as_ref()).await;
                    return Err(anyhow::Error::new(panicked)
                        .context("a shard manager task panicked"));
                }
                None => {
                    release_leadership(details.leadership.as_ref()).await;
                    return Ok(());
                }
            },
            _ = shutdown.cancelled() => {
                join_set.abort_all();
                release_leadership(details.leadership.as_ref()).await;
                return Ok(());
            }
        }
    }
}
