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
mod otlp_plugin;
mod plugins;
mod quota;
mod rich_types;
mod worker;

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

tag_suite!(otlp_plugin, group7);
tag_suite!(plugins, group7);

tag_suite!(custom_api, group10);
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
// Unpaired suites (otlp_plugin, plugins) are NOT matrix-multiplied and run
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
            #[cfg(unix)]
            unsafe {
                backtrace_on_stack_overflow::enable()
            };
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
    // unpaired suites (otlp_plugin, plugins) which take a plain
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
