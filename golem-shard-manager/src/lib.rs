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
use golem_common::base_model::shard_lease;
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
    ExecutorAddr, ExecutorAddrs, ExecutorId, ExecutorLease, ExecutorShards, RegisterAck,
    ShardAssignmentEntry, ShardAssignmentPush, ShardEpoch, ShardLeaseGrant, ShardLeaseRevision,
    ShardLeaseState,
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

#[cfg(test)]
mod lease_timing_tests {
    use super::validate_timing_config;
    use crate::config::{EtcdConfig, PersistenceConfig, ShardManagerConfig};
    use golem_common::base_model::shard_lease;
    use std::time::Duration;
    use test_r::test;

    fn config_with(shard_lease_duration: Duration) -> ShardManagerConfig {
        ShardManagerConfig {
            shard_lease_duration,
            ..ShardManagerConfig::default()
        }
    }

    /// The shipped default is the shortest lease that delivers the full renewal budget, so it must
    /// start without so much as a warning - if it did not, every deployment would be told to tune
    /// a value it never set.
    #[test]
    fn the_default_lease_starts_silently() {
        let warnings = validate_timing_config(&ShardManagerConfig::default())
            .expect("the shipped default must be accepted");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    /// Between the minimum and the recommended minimum the protocol still works, it just survives
    /// fewer consecutive shard-manager hiccups. That is availability, not correctness, so it warns
    /// rather than refusing - and the warning names the key, or an operator is left guessing which
    /// of several durations to move.
    #[test]
    fn a_short_but_workable_lease_warns_and_names_the_key() {
        let warnings =
            validate_timing_config(&config_with(shard_lease::min_shard_lease_duration()))
                .expect("a lease at the minimum must still start");
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("shard_lease_duration"),
            "the warning must name the key it is about: {}",
            warnings[0]
        );
    }

    /// Below the minimum an executor cannot finish one renewal attempt inside the gap between two
    /// renewals, so every unanswered call costs the lease outright. Refused rather than degraded:
    /// every replica shares the setting, so a cluster would fail the same way at the same time.
    #[test]
    fn a_lease_too_short_to_renew_is_refused() {
        let error = validate_timing_config(&config_with(
            shard_lease::min_shard_lease_duration() - Duration::from_millis(1),
        ))
        .expect_err("a lease below the minimum must be refused");
        let message = format!("{error:#}");
        assert!(
            message.contains("shard_lease_duration"),
            "the refusal must name the key: {message}"
        );
    }

    #[test]
    fn a_zero_lease_is_still_refused() {
        assert!(validate_timing_config(&config_with(Duration::ZERO)).is_err());
    }

    #[test]
    fn the_maximum_state_write_timeout_is_accepted() {
        let config = ShardManagerConfig {
            state_write_timeout: shard_lease::max_state_write_timeout(),
            ..ShardManagerConfig::default()
        };
        validate_timing_config(&config).expect("the supported maximum must be accepted");
    }

    #[test]
    fn state_write_timeouts_outside_the_protocol_range_are_refused() {
        for state_write_timeout in [
            Duration::ZERO,
            shard_lease::max_state_write_timeout() + Duration::from_millis(1),
        ] {
            let config = ShardManagerConfig {
                state_write_timeout,
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an unsupported state write timeout must be refused");
            assert!(
                error.to_string().contains("state_write_timeout"),
                "the refusal must name the invalid key: {error:#}"
            );
        }
    }

    #[test]
    fn invalid_startup_timeouts_are_refused() {
        let invalid = [
            ("state_read_timeout", Duration::ZERO, None),
            (
                "state_read_timeout",
                shard_lease::default_state_write_timeout(),
                None,
            ),
            (
                "initial_health_check_timeout",
                ShardManagerConfig::default().state_read_timeout,
                Some(Duration::ZERO),
            ),
        ];

        for (key, state_read_timeout, initial_health_check_timeout) in invalid {
            let config = ShardManagerConfig {
                state_read_timeout,
                initial_health_check_timeout: initial_health_check_timeout
                    .unwrap_or_else(|| ShardManagerConfig::default().initial_health_check_timeout),
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an invalid startup timeout must be refused");
            assert!(
                error.to_string().contains(key),
                "the refusal must name {key}: {error:#}"
            );
        }
    }

    #[test]
    fn invalid_etcd_retry_settings_are_refused() {
        let invalid = [
            (
                "read_retry_timeout",
                EtcdConfig {
                    read_retry_timeout: Duration::ZERO,
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_min_delay",
                EtcdConfig {
                    retry_min_delay: Duration::ZERO,
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_max_delay",
                EtcdConfig {
                    retry_min_delay: Duration::from_secs(2),
                    retry_max_delay: Duration::from_secs(1),
                    ..EtcdConfig::default()
                },
            ),
            (
                "retry_max_delay",
                EtcdConfig {
                    retry_max_delay: Duration::MAX,
                    ..EtcdConfig::default()
                },
            ),
            (
                "read_retry_timeout",
                EtcdConfig {
                    read_retry_timeout: Duration::from_secs(26),
                    ..EtcdConfig::default()
                },
            ),
        ];

        for (key, etcd) in invalid {
            let config = ShardManagerConfig {
                persistence: PersistenceConfig::Etcd(etcd),
                ..ShardManagerConfig::default()
            };
            let error = validate_timing_config(&config)
                .expect_err("an invalid etcd retry setting must be refused");
            assert!(
                error.to_string().contains(key),
                "the refusal must name {key}: {error:#}"
            );
        }
    }

    #[test]
    fn etcd_retry_timeout_that_cannot_form_an_instant_is_refused() {
        let request_timeout = Duration::from_secs(1);
        let etcd = EtcdConfig {
            request_timeout,
            read_retry_timeout: Duration::MAX - request_timeout,
            ..EtcdConfig::default()
        };

        let config = ShardManagerConfig {
            state_read_timeout: Duration::MAX,
            persistence: PersistenceConfig::Etcd(etcd),
            ..ShardManagerConfig::default()
        };

        assert!(
            std::time::Instant::now()
                .checked_add(Duration::MAX - request_timeout)
                .is_none(),
            "the test value must exceed this platform's Instant range"
        );
        validate_timing_config(&config)
            .expect_err("startup must reject a retry timeout that panics when added to Instant");
    }
}

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
    // Scoped so the campaign future is dropped before the cleanup below: while it is alive it
    // borrows `election`, and it is precisely a dropped campaign that leaves a lease behind.
    let outcome = {
        let mut campaign = std::pin::pin!(election.campaign_until_elected());

        loop {
            tokio::select! {
                elected = &mut campaign => break elected.map_err(anyhow::Error::from),
                joined = join_set.join_next(), if !join_set.is_empty() => match joined {
                    Some(Err(err)) => {
                        break Err(anyhow::Error::new(err).context(
                            "a shard manager task panicked while campaigning for leadership",
                        ));
                    }
                    Some(Ok(Err(err))) => {
                        break Err(err
                            .context("a shard manager task failed while campaigning for leadership"));
                    }
                    Some(Ok(Ok(()))) => {
                        warn!("A shard manager task finished while campaigning for leadership")
                    }
                    None => {}
                }
            }
        }
    };

    if outcome.is_err() {
        election.revoke_pending_lease().await;
    }
    outcome
}

pub(crate) fn ensure_shard_count_matches(
    stored: usize,
    configured: usize,
) -> Result<(), ShardManagerError> {
    if stored != configured {
        return Err(ShardManagerError::Internal(format!(
            "the persisted shard lease state was written with {stored} shards, but this shard \
             manager is configured for {configured}. The stored value governs routing, so \
             starting would silently ignore the configuration; changing the shard count is not \
             supported."
        )));
    }
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
        etcd.retry_min_delay,
        etcd.retry_max_delay,
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

/// Checks the configured startup, persistence, retry, and lease timings, returning warnings worth
/// logging and refusing outright combinations that cannot work.
///
/// The executor renews at a third of the lease and gives up on one attempt at half of that, never
/// below a floor, and the shard manager needs its own write budget to fit inside that attempt.
/// Those relations live in [`golem_common::base_model::shard_lease`]; this is where the configured
/// duration meets them, because the lease duration is the shard manager's config and the executor
/// only ever learns it from the wire.
fn validate_timing_config(config: &ShardManagerConfig) -> anyhow::Result<Vec<String>> {
    anyhow::ensure!(
        !config.state_read_timeout.is_zero(),
        "state_read_timeout must be greater than zero"
    );
    anyhow::ensure!(
        !config.state_write_timeout.is_zero()
            && config.state_write_timeout <= shard_lease::max_state_write_timeout(),
        "state_write_timeout must be greater than zero and at most {:?}, but is {:?}",
        shard_lease::max_state_write_timeout(),
        config.state_write_timeout
    );
    anyhow::ensure!(
        config.state_read_timeout > config.state_write_timeout,
        "state_read_timeout must be greater than state_write_timeout, but is {:?} against {:?}",
        config.state_read_timeout,
        config.state_write_timeout
    );
    anyhow::ensure!(
        !config.initial_health_check_timeout.is_zero(),
        "initial_health_check_timeout must be greater than zero"
    );
    if let PersistenceConfig::Etcd(etcd) = &config.persistence {
        anyhow::ensure!(
            !etcd.read_retry_timeout.is_zero(),
            "persistence.config.read_retry_timeout must be greater than zero"
        );
        anyhow::ensure!(
            std::time::Instant::now()
                .checked_add(etcd.read_retry_timeout)
                .is_some(),
            "persistence.config.read_retry_timeout is too large for a retry deadline"
        );
        anyhow::ensure!(
            !etcd.retry_min_delay.is_zero(),
            "persistence.config.retry_min_delay must be greater than zero"
        );
        anyhow::ensure!(
            etcd.retry_max_delay >= etcd.retry_min_delay,
            "persistence.config.retry_max_delay must be at least retry_min_delay, but is {:?} \
             against {:?}",
            etcd.retry_max_delay,
            etcd.retry_min_delay
        );
        anyhow::ensure!(
            tokio::time::Instant::now()
                .checked_add(etcd.retry_max_delay)
                .is_some(),
            "persistence.config.retry_max_delay is too large for a retry backoff"
        );
        anyhow::ensure!(
            etcd.read_retry_timeout
                .checked_add(etcd.request_timeout)
                .is_some_and(|read_with_final_attempt| {
                    read_with_final_attempt <= config.state_read_timeout
                }),
            "persistence.config.read_retry_timeout plus request_timeout must fit within \
             state_read_timeout, but {:?} plus {:?} exceeds {:?}",
            etcd.read_retry_timeout,
            etcd.request_timeout,
            config.state_read_timeout
        );
    }
    anyhow::ensure!(
        !config.shard_lease_duration.is_zero(),
        "shard_lease_duration must be greater than zero"
    );
    anyhow::ensure!(
        chrono::Duration::from_std(config.shard_lease_duration).is_ok(),
        "shard_lease_duration {:?} is out of range",
        config.shard_lease_duration
    );
    anyhow::ensure!(
        config.shard_lease_duration >= shard_lease::min_shard_lease_duration(),
        "shard_lease_duration must be at least {:?}, but is {:?}. An executor gives up on one \
         renewal after {:?} at the shortest, which is the whole gap between two renewals at this \
         lease: a single unanswered call would cost the lease with no attempt left to save it.",
        shard_lease::min_shard_lease_duration(),
        config.shard_lease_duration,
        shard_lease::rpc_deadline_floor(),
    );

    let mut warnings = Vec::new();
    if config.shard_lease_duration < shard_lease::recommended_min_shard_lease_duration() {
        warnings.push(format!(
            "shard_lease_duration is {:?}; an executor gets fewer than the {} renewal attempts a \
             lease is meant to survive, because the per-attempt deadline is held up by its {:?} \
             floor. {:?} is the shortest lease that delivers all of them.",
            config.shard_lease_duration,
            shard_lease::SHARD_LEASE_RENEWAL_ATTEMPTS,
            shard_lease::rpc_deadline_floor(),
            shard_lease::recommended_min_shard_lease_duration(),
        ));
    }
    Ok(warnings)
}

pub async fn run(
    shard_manager_config: &ShardManagerConfig,
    deployment: Deployment,
    registry: Registry,
    join_set: &mut JoinSet<anyhow::Result<()>>,
) -> anyhow::Result<RunDetails> {
    debug!("Initializing shard manager");

    for warning in validate_timing_config(shard_manager_config)? {
        warn!("{warning}");
    }

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
                        etcd.compaction_retention_revisions,
                        etcd.read_retry_timeout,
                        etcd.retry_min_delay,
                        etcd.retry_max_delay,
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
            ShardManagement::new_with_timeouts(
                persistence_service.clone(),
                worker_executors.clone(),
                health_check.clone(),
                shard_manager_config.rebalance_threshold,
                shard_manager_config.shard_lease_duration,
                shard_manager_config.state_read_timeout,
                shard_manager_config.state_write_timeout,
                shard_manager_config.initial_health_check_timeout,
                shard_manager_config.number_of_shards,
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
            // Awaited, not just requested: a task still running could reach an executor after a
            // standby has taken over.
            join_set.shutdown().await;
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
///
/// Every path stops the tasks and *waits* for them before the lease is released, so no executor
/// command can be in flight once another replica may be leading.
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
                    join_set.shutdown().await;
                    release_leadership(details.leadership.as_ref()).await;
                    return Err(err.context("a shard manager task failed"));
                }
                Some(Err(panicked)) => {
                    join_set.shutdown().await;
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
                join_set.shutdown().await;
                release_leadership(details.leadership.as_ref()).await;
                return Ok(());
            }
        }
    }
}
