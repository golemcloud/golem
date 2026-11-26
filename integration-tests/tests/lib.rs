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

mod agent_config;
mod api;
mod capabilities;
mod custom_api;
mod fork;
mod gol40_chunk_g_environment_resolution_acceptance;
mod invocation_context;
mod invocations;
mod metrics_catalog;
mod otlp_plugin;
mod owner_environment_resolution;
mod permissions;
mod plugins;
mod quota;
mod rich_types;
mod worker;

#[cfg(unix)]
#[path = "platform/unix.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "platform/non_unix.rs"]
mod platform;

use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
#[allow(unused_imports)]
use golem_test_framework::config::WorkerExecutorClusterControl;
use golem_test_framework::config::{
    DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    WorkerExecutorClusterControlDispatch, WorkerExecutorClusterControlStub,
};
use std::sync::Once;
use test_r::{define_matrix_dimension, matrix_suite, tag_suite, test_dep};

test_r::enable!();

tag_suite!(worker, group1);
tag_suite!(fork, group1);

tag_suite!(agent_config, group12);
tag_suite!(api, group2);

tag_suite!(invocation_context, group7);
tag_suite!(metrics_catalog, group7);
tag_suite!(otlp_plugin, group7);
tag_suite!(plugins, group7);

tag_suite!(custom_api, group10);
tag_suite!(gol40_chunk_g_environment_resolution_acceptance, group10);
tag_suite!(owner_environment_resolution, group10);
tag_suite!(permissions, group10);
tag_suite!(quota, group10);
tag_suite!(rich_types, group10);
tag_suite!(capabilities, group10);

test_r::sequential_suite!(otlp_plugin);
test_r::sequential_suite!(plugins);

// Matrix dimension for the DB backend. Paired suites (worker, fork, api,
// agent_config, custom_api, quota, rich_types, capabilities) are multiplied
// across this dimension via `matrix_suite!` below, producing one test per case
// named `<test>_postgres` / `<test>_sqlite`, each carrying the `db_postgres` /
// `db_sqlite` auto-tag (selectable with `:tag:db_postgres` / `:tag:db_sqlite`).
// Unpaired suites (invocation_context, otlp_plugin, plugins) are NOT matrix-multiplied and run
// once against the untagged (postgres) `create_deps` below.
define_matrix_dimension!(db: EnvBasedTestDependencies -> "postgres", "sqlite");

// Apply the `db` matrix to each paired suite. The target modules only need to
// `inherit_test_dep!(EnvBasedTestDependencies)` (the untagged getter, which
// they already do); the per-case tagged deps are materialized from the tagged
// constructors below, and the untagged getter is aliased to them at runtime.
matrix_suite!(worker, db, EnvBasedTestDependencies);
matrix_suite!(fork, db, EnvBasedTestDependencies);
matrix_suite!(api, db, EnvBasedTestDependencies);
matrix_suite!(agent_config, db, EnvBasedTestDependencies);
matrix_suite!(custom_api, db, EnvBasedTestDependencies);
matrix_suite!(
    gol40_chunk_g_environment_resolution_acceptance,
    db,
    EnvBasedTestDependencies
);
matrix_suite!(owner_environment_resolution, db, EnvBasedTestDependencies);
matrix_suite!(permissions, db, EnvBasedTestDependencies);
matrix_suite!(quota, db, EnvBasedTestDependencies);
matrix_suite!(rich_types, db, EnvBasedTestDependencies);
matrix_suite!(capabilities, db, EnvBasedTestDependencies);

#[derive(Debug)]
pub struct Tracing;

// `init_tracing_with_default_debug_env_filter` installs a global
// `tracing` subscriber via `Registry::init()`, which panics on a second
// invocation. Guard with a `Once` so it's safe to call from both the
// parent-side `create_deps` body (to capture `ChildProcessLogger` events
// emitted before any worker exists) and the `PerWorker` `tracing()` dep
// (which still has to install a subscriber inside each spawned worker
// subprocess, but in `--nocapture` mode runs in the same process as
// `create_deps`).
static TRACING_INIT: Once = Once::new();

impl Tracing {
    pub fn init() -> Self {
        TRACING_INIT.call_once(|| {
            platform::enable_stack_overflow_backtrace();
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("integration-tests").with_env_overrides(),
            );
        });
        Self
    }
}

#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl))]
pub async fn create_deps() -> EnvBasedTestDependencies {
    // Initialise tracing on the parent process before spawning any
    // dependency services so that `ChildProcessLogger` events emitted
    // by the parent (e.g. forwarded stdout/stderr of spawned services)
    // are routed through a registered subscriber rather than dropped.
    //
    // Doing this inside the `create_deps` body — instead of via a
    // separate `scope = Hosted` `Tracing` dep — avoids tripping a
    // test-r pruning interaction where a second Hosted dep returning
    // the same type as the `PerWorker` `tracing()` dep causes the
    // latter to silently be skipped in the `--nocapture`/no-spawn-
    // workers code path.
    //
    // This untagged constructor is the postgres backend used by the
    // unpaired suites (invocation_context, otlp_plugin, plugins) which take a plain
    // `&EnvBasedTestDependencies`. It is also the compile-time getter
    // symbol the matrix-multiplied tests resolve against; their
    // dependency is rewritten to the tagged variant at runtime, so this
    // constructor's body is never materialized for them.
    Tracing::init();

    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        db_type: DbType::Postgres,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl), tagged_as = "postgres")]
pub async fn create_deps_postgres() -> EnvBasedTestDependencies {
    Tracing::init();

    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        db_type: DbType::Postgres,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl), tagged_as = "sqlite")]
pub async fn create_deps_sqlite() -> EnvBasedTestDependencies {
    Tracing::init();

    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        db_type: DbType::Sqlite,
        ..EnvBasedTestDependenciesConfig::new()
    })
    .await
    .expect("Failed constructing test dependencies");

    deps.redis_monitor().assert_valid();

    deps
}

#[test_dep(scope = PerWorker)]
pub fn tracing() -> Tracing {
    Tracing::init()
}

#[test_r::tag(group1)]
mod supported_settings {
    use super::Tracing;
    use golem_test_framework::config::{
        DbType, EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestServiceInputs,
    };
    use test_r::{inherit_test_dep, tag, test, test_dep, timeout};

    inherit_test_dep!(Tracing);

    fn config(
        db_type: DbType,
        base: u16,
        env: impl Fn(&str) -> Option<String>,
    ) -> EnvBasedTestDependenciesConfig {
        EnvBasedTestDependenciesConfig {
            service_inputs: TestServiceInputs::SupportedSettings,
            db_type,
            worker_executor_cluster_size: 3,
            redis_port: base,
            registry_http_port: base + 1,
            registry_grpc_port: base + 2,
            compilation_http_port: base + 3,
            compilation_grpc_port: base + 4,
            shard_manager_http_port: base + 5,
            shard_manager_grpc_port: base + 6,
            worker_http_port: base + 7,
            worker_grpc_port: base + 8,
            worker_custom_request_port: base + 9,
            worker_mcp_port: base + 10,
            executor_base_http_port: base + 20,
            executor_base_grpc_port: base + 30,
            ..EnvBasedTestDependenciesConfig::default()
        }
        .with_env_overrides_from(|key| match key {
            "GOLEM_TEST_PORT_OFFSET" | "QUIET" | "GOLEM_REPO_ROOT" | "GOLEM_TEST_BIN_PROFILE" => {
                env(key)
            }
            _ => None,
        })
    }

    #[test_dep(scope = Hosted, tagged_as = "supported_postgres")]
    async fn postgres() -> EnvBasedTestDependencies {
        Tracing::init();
        EnvBasedTestDependencies::new(config(DbType::Postgres, 12000, |key| {
            std::env::var(key).ok()
        }))
        .await
        .expect("construct supported PostgreSQL dependencies")
    }

    #[test_dep(scope = Hosted, tagged_as = "supported_sqlite")]
    async fn sqlite() -> EnvBasedTestDependencies {
        Tracing::init();
        EnvBasedTestDependencies::new(config(DbType::Sqlite, 13000, |key| std::env::var(key).ok()))
            .await
            .expect("construct supported SQLite dependencies")
    }

    #[test]
    #[tag(db_postgres)]
    #[timeout("4m")]
    async fn counter_resource_test_1_postgres(
        #[tagged_as("supported_postgres")] deps: &EnvBasedTestDependencies,
        tracing: &Tracing,
    ) -> anyhow::Result<()> {
        super::worker::counter_resource_test_1(deps, tracing).await
    }

    #[test]
    #[tag(db_sqlite)]
    #[timeout("4m")]
    async fn counter_resource_test_1_sqlite(
        #[tagged_as("supported_sqlite")] deps: &EnvBasedTestDependencies,
        tracing: &Tracing,
    ) -> anyhow::Result<()> {
        super::worker::counter_resource_test_1(deps, tracing).await
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[tag(db_sqlite)]
    #[timeout("8m")]
    async fn supported_owner_restart_preserves_launch(tracing: &Tracing) -> anyhow::Result<()> {
        use anyhow::Context;
        use futures::{FutureExt, StreamExt, TryStreamExt};
        use golem_test_framework::components::{is_serving_grpc, worker_executor::WorkerExecutor};
        use golem_test_framework::config::TestDependencies;
        use std::panic::AssertUnwindSafe;
        use std::time::Duration;
        use tokio::time::{Instant, timeout};

        async fn stopped(executor: &dyn WorkerExecutor, http: u16, grpc: u16, generation: u64) {
            assert_eq!(executor.pid(), None);
            assert!(executor.is_reaped());
            assert!(!executor.is_running().await);
            assert_eq!(executor.grpc_host(), "localhost");
            assert_eq!(executor.grpc_port(), grpc);
            assert_eq!(
                executor.metrics_endpoint(),
                Some((format!("http://localhost:{http}/metrics"), generation))
            );
        }

        async fn live(
            executor: &dyn WorkerExecutor,
            http: u16,
            grpc: u16,
            generation: u64,
            witness: &str,
        ) -> anyhow::Result<()> {
            let pid = executor
                .pid()
                .context("supported executor has no owned PID")?;
            assert!(!executor.is_reaped());
            assert!(executor.is_running().await);
            assert_eq!(executor.grpc_host(), "localhost");
            assert_eq!(executor.grpc_port(), grpc);
            let url = format!("http://localhost:{http}/metrics");
            assert_eq!(executor.metrics_endpoint(), Some((url.clone(), generation)));
            assert!(is_serving_grpc("localhost", grpc, Duration::from_secs(1)).await);
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()?;
            let scrapes = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
                Duration::from_millis(50),
            ))
            .then(|_| client.get(&url).send())
            .filter_map(|response| {
                std::future::ready(
                    response
                        .ok()
                        .filter(|response| response.status().is_success()),
                )
            });
            futures::pin_mut!(scrapes);
            timeout(Duration::from_secs(10), scrapes.next())
                .await
                .context("replacement metrics did not serve")?
                .context("metrics polling ended")?;

            let environment = std::fs::read(format!("/proc/{pid}/environ"))
                .context("read owned executor environment")?;
            let present = [
                witness,
                "GOLEM_ADVANCED__WORKER_EXECUTOR__BIND_ADDRESS",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__COMPILED_COMPONENT_SERVICE__TYPE",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__ENGINE__ENABLE_FS_CACHE",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__LIMITS__FUEL_TO_BORROW",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__COMPUTE",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__QUOTA_SERVICE__INLINE_WAIT_THRESHOLD",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__QUOTA_SERVICE__RENEWAL_INTERVAL",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__RETRIES__MAX_ATTEMPTS",
                "GOLEM_ADVANCED__WORKER_EXECUTOR__AGENT_WEBHOOKS_SERVICE__USE_HTTPS_FOR_WEBHOOK_URL",
            ].map(|key| (key, environment.split(|byte| *byte == 0).any(|record| record.split(|byte| *byte == b'=').next() == Some(key.as_bytes()))));
            drop(environment);
            present
                .into_iter()
                .for_each(|(key, present)| assert!(!present, "unsupported child input {key}"));
            assert!(executor.is_running().await);
            Ok(())
        }

        let witness = if std::env::var_os("GOLEM_TEST_SUPPORTED_PARENT_SENTINEL").is_some() {
            "GOLEM_TEST_SUPPORTED_PARENT_SENTINEL"
        } else {
            "PATH"
        };
        anyhow::ensure!(
            std::env::var_os(witness).is_some(),
            "parent inheritance witness {witness} is absent"
        );
        let config = EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 1,
            ..config(DbType::Sqlite, 14000, |key| std::env::var(key).ok())
        };
        let http = config.executor_base_http_port;
        let grpc = config.executor_base_grpc_port;
        let owner = EnvBasedTestDependencies::new(config).await?;
        let registry = owner.registry_service();
        assert_eq!(
            registry.admin_account_id().0,
            uuid::uuid!("e71a6160-4144-4720-9e34-e5943458d129")
        );
        assert_eq!(registry.admin_account_email().to_string(), "initial@user");
        assert_eq!(
            registry.builtin_plugin_owner_account_id().0,
            uuid::uuid!("adb2694f-cd9f-425d-905d-ca2888c9c5de")
        );
        assert_eq!(
            registry.default_plan().0,
            uuid::uuid!("157dc684-00eb-496d-941c-da8fd1d15c63")
        );
        assert_eq!(
            [
                registry.low_fuel_plan(),
                registry.low_disk_space_plan(),
                registry.low_http_calls_plan(),
                registry.low_rpc_calls_plan()
            ],
            [None; 4]
        );
        let cluster = owner.worker_executor_cluster();
        assert_eq!(cluster.size(), 1);
        let executor = cluster
            .to_vec()
            .into_iter()
            .next()
            .context("missing owned executor")?;
        let generation = executor
            .metrics_endpoint()
            .context("missing owner metrics")?
            .1;
        live(executor.as_ref(), http, grpc, generation, witness).await?;
        executor
            .kill_and_wait(Instant::now() + Duration::from_secs(60))
            .await?;
        stopped(executor.as_ref(), http, grpc, generation).await;

        futures::stream::iter([("HTTP_PORT", http + 1), ("GRPC__PORT", grpc + 1)])
            .then(|(leaf, port)| {
                let executor = &executor;
                async move {
                    let rejected = timeout(
                        Duration::from_secs(5),
                        AssertUnwindSafe(executor.restart_with_extra_env_vars(vec![(
                            format!("GOLEM_ADVANCED__WORKER_EXECUTOR__{leaf}"),
                            port.to_string(),
                        )]))
                        .catch_unwind(),
                    )
                    .await?;
                    anyhow::ensure!(
                        rejected.is_err(),
                        "supported executor accepted listener relocation"
                    );
                    stopped(executor.as_ref(), http, grpc, generation).await;
                    Ok::<(), anyhow::Error>(())
                }
            })
            .try_collect::<Vec<_>>()
            .await?;

        timeout(
            Duration::from_secs(120),
            executor.restart_with_extra_env_vars(vec![
                (
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT".into(),
                    http.to_string(),
                ),
                (
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT".into(),
                    grpc.to_string(),
                ),
            ]),
        )
        .await?;
        live(executor.as_ref(), http, grpc, generation + 1, witness).await?;
        executor
            .kill_and_wait(Instant::now() + Duration::from_secs(60))
            .await?;
        stopped(executor.as_ref(), http, grpc, generation + 1).await;
        timeout(Duration::from_secs(120), executor.restart()).await?;
        live(executor.as_ref(), http, grpc, generation + 2, witness).await?;

        let shard = owner.shard_manager();
        let shard_host = shard.grpc_host();
        let shard_port = shard.grpc_port();
        timeout(Duration::from_secs(30), shard.kill()).await?;
        assert!(!is_serving_grpc(&shard_host, shard_port, Duration::from_secs(1)).await);
        let refused = timeout(
            Duration::from_secs(5),
            AssertUnwindSafe(shard.restart(Some(17))).catch_unwind(),
        )
        .await?;
        anyhow::ensure!(
            refused.is_err(),
            "supported shard manager accepted policy override"
        );
        assert!(!is_serving_grpc(&shard_host, shard_port, Duration::from_secs(1)).await);
        timeout(Duration::from_secs(120), shard.restart(None)).await?;
        assert!(is_serving_grpc(&shard_host, shard_port, Duration::from_secs(1)).await);
        let registered = tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(
            Duration::from_secs(2),
        ))
        .then(|_| shard.get_routing_table())
        .try_filter(|table| std::future::ready(table.all().len() == 1));
        futures::pin_mut!(registered);
        timeout(Duration::from_secs(60), registered.next())
            .await
            .context("executor did not register after shard restart")?
            .context("registration polling ended")??;
        super::worker::counter_resource_test_1(&owner, tracing).await
    }

    #[test]
    fn tracer_resources_are_disjoint_and_offset_once() {
        let ports = |config: &EnvBasedTestDependenciesConfig| {
            [
                config.redis_port,
                config.registry_http_port,
                config.registry_grpc_port,
                config.compilation_http_port,
                config.compilation_grpc_port,
                config.shard_manager_http_port,
                config.shard_manager_grpc_port,
                config.worker_http_port,
                config.worker_grpc_port,
                config.worker_custom_request_port,
                config.worker_mcp_port,
            ]
            .into_iter()
            .chain(
                (0..config.worker_executor_cluster_size as u16).flat_map(|index| {
                    [
                        config.executor_base_http_port + index,
                        config.executor_base_grpc_port + index,
                    ]
                }),
            )
            .collect::<Vec<_>>()
        };
        let env = |key: &str| match key {
            "GOLEM_TEST_PORT_OFFSET" => Some("10000".to_string()),
            "REDIS_PORT" | "REGISTRY_HTTP_PORT" => Some("1".to_string()),
            _ => None,
        };
        let postgres = config(DbType::Postgres, 12000, env);
        let sqlite = config(DbType::Sqlite, 13000, env);
        let owner = EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 1,
            ..config(DbType::Sqlite, 14000, env)
        };
        assert_eq!(postgres.redis_port, 22000);
        assert_eq!(sqlite.redis_port, 23000);
        assert_eq!(owner.redis_port, 24000);
        let ordinary = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
            (key == "GOLEM_TEST_PORT_OFFSET").then(|| "10000".to_string())
        });
        let all = [ordinary, postgres, sqlite, owner]
            .iter()
            .flat_map(ports)
            .collect::<Vec<_>>();
        assert_eq!(
            all.iter().collect::<std::collections::HashSet<_>>().len(),
            all.len()
        );
    }
}
