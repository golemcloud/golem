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
        assert_eq!(postgres.redis_port, 22000);
        assert_eq!(sqlite.redis_port, 23000);
        let ordinary = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
            (key == "GOLEM_TEST_PORT_OFFSET").then(|| "10000".to_string())
        });
        let all = [ordinary, postgres, sqlite]
            .iter()
            .flat_map(ports)
            .collect::<Vec<_>>();
        assert_eq!(
            all.iter().collect::<std::collections::HashSet<_>>().len(),
            all.len()
        );
    }
}
