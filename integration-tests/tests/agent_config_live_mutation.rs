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

#[path = "agent_config/shared_agent_config_live_mutation.rs"]
mod shared_agent_config_live_mutation;

use convert_case::ccase;
use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
use golem_test_framework::config::{
    EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    WorkerExecutorClusterControl, WorkerExecutorClusterControlDispatch,
    WorkerExecutorClusterControlStub,
};
use std::sync::Arc;
use test_r::{tag_suite, test, test_dep};

test_r::enable!();

tag_suite!(shared_agent_config_live_mutation, group8);

trait TestContext: std::fmt::Debug + Send + Sync {
    fn test_component_file(&self) -> &'static str;
    fn test_component_name(&self) -> &'static str;
    fn agent_method_name(&self) -> &'static str;
    fn create_replay_gate_method_name(&self) -> &'static str;
    fn reveal_secret_then_await_replay_gate_method_name(&self) -> &'static str;
    fn case_config_path_segment(&self, segment: &str) -> String;
}

#[test_dep(scope = PerWorker, tagged_as = "ts")]
fn test_context_ts() -> Arc<dyn TestContext> {
    #[derive(Debug)]
    struct TsTestContext;

    impl TestContext for TsTestContext {
        fn test_component_file(&self) -> &'static str {
            "golem_it_agent_sdk_ts"
        }
        fn test_component_name(&self) -> &'static str {
            "golem-it:agent-sdk-ts"
        }
        fn agent_method_name(&self) -> &'static str {
            "echoLocalConfig"
        }
        fn create_replay_gate_method_name(&self) -> &'static str {
            "createReplayGate"
        }
        fn reveal_secret_then_await_replay_gate_method_name(&self) -> &'static str {
            "revealSecretThenAwaitReplayGate"
        }
        fn case_config_path_segment(&self, segment: &str) -> String {
            ccase!(kebab -> camel, segment)
        }
    }

    Arc::new(TsTestContext)
}

#[test_dep(scope = PerWorker, tagged_as = "rust")]
fn test_context_rust() -> Arc<dyn TestContext> {
    #[derive(Debug)]
    struct RustTestContext;

    impl TestContext for RustTestContext {
        fn test_component_file(&self) -> &'static str {
            "golem_it_agent_sdk_rust_release"
        }
        fn test_component_name(&self) -> &'static str {
            "golem-it:agent-sdk-rust"
        }
        fn agent_method_name(&self) -> &'static str {
            "echo_local_config"
        }
        fn create_replay_gate_method_name(&self) -> &'static str {
            "create_replay_gate"
        }
        fn reveal_secret_then_await_replay_gate_method_name(&self) -> &'static str {
            "reveal_secret_then_await_replay_gate"
        }
        fn case_config_path_segment(&self, segment: &str) -> String {
            ccase!(kebab -> snake, segment)
        }
    }

    Arc::new(RustTestContext)
}

#[derive(Debug)]
pub struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        #[cfg(unix)]
        unsafe {
            backtrace_on_stack_overflow::enable()
        };
        init_tracing_with_default_debug_env_filter(
            &TracingConfig::test_pretty_without_time("agent-config-live-mutation")
                .with_env_overrides(),
        );
        Self
    }
}

// `EnvBasedTestDependencies` is a Hosted dep with an async worker-side
// reconstruction (it attaches to the parent's live Redis, RDB, registry
// service, shard manager, worker service, worker executor cluster, and
// on-disk blob/component caches via `AsyncHostedDep::from_descriptor`).
// Hosted owner constructors cannot depend on other test_deps, so the
// `&Tracing` parameter is dropped here. Tracing remains a separate
// PerWorker dep installed inside each worker subprocess.
//
// Registered as `worker = both(WorkerExecutorClusterControl)`: workers get the
// existing descriptor-reconstructed `EnvBasedTestDependencies`
// (bulk-data gRPC clients keep working unchanged) AND an
// `&WorkerExecutorClusterControlStub` for the small control-plane surface that
// must run against the parent's single owner instance.
#[test_dep(scope = Hosted, worker = both(WorkerExecutorClusterControl))]
pub async fn create_deps() -> EnvBasedTestDependencies {
    let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
        worker_executor_cluster_size: 3,
        environment_state_cache_capacity: Some(0),
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

// Smoke test for the `worker = both(WorkerExecutorClusterControl)` registration: the
// worker subprocess receives both the descriptor-reconstructed
// `EnvBasedTestDependencies` and a `WorkerExecutorClusterControlStub` for the same
// parent-held owner, and the two views agree about the underlying
// parent-owned Redis instance.
//
// Runs in <1s and only exercises the RPC channel, so it's safe to
// keep in this binary alongside the heavier agent-config live-mutation
// suites.
#[test]
async fn redis_control_round_trip(
    deps: &EnvBasedTestDependencies,
    redis_control: &WorkerExecutorClusterControlStub,
) {
    // Parent-owned Redis must be reachable from a worker via the
    // RPC surface (and not just via the descriptor path).
    assert!(
        redis_control.is_redis_healthy().await,
        "WorkerExecutorClusterControl::is_redis_healthy reported the parent-owned Redis as unhealthy"
    );

    // Descriptor view and RPC view must agree on which Redis prefix
    // the parent's single owner is using. This is the cross-view
    // consistency check the `worker = both(T)` shape exists for.
    let prefix_via_descriptor = deps.redis().prefix().to_string();
    let prefix_via_rpc = redis_control.redis_prefix().await;
    assert_eq!(
        prefix_via_descriptor, prefix_via_rpc,
        "descriptor-view Redis prefix ({prefix_via_descriptor:?}) and \
         RPC-view Redis prefix ({prefix_via_rpc:?}) must refer to the \
         same parent-owned Redis instance"
    );

    // Proof that the descriptor view and the RPC view talk to the
    // **same** parent-owned Redis instance (and not just two clients
    // pointed at the same host:port that could have been rotated
    // between snapshots):
    //
    //   1. Use the descriptor view (`deps.redis()` -> parent-side
    //      `Arc<dyn Redis>` reconstructed from the descriptor) to
    //      write a sentinel key into db 15.
    //   2. Use the RPC view (`redis_control.flush_redis_db(15)`)
    //      which runs `FLUSHDB` on the parent's owner against the
    //      same `Arc<dyn Redis>`.
    //   3. Re-open a connection through the descriptor view and
    //      assert the sentinel is gone.
    //
    // If the RPC view ever stops routing back to the parent's owner
    // (e.g. if `worker = both(...)` regressed to building a fresh
    // owner per worker), step 2 would flush a different Redis db
    // and step 3 would still see the sentinel.
    //
    // We pick a high db index (15) that the rest of the suite is
    // unlikely to be using, to avoid interfering with concurrent
    // test traffic.

    let sentinel_key = format!("test-r:hr3.2:sentinel:{}", std::process::id());
    let sentinel_value = "alive";

    {
        let mut conn = deps
            .redis()
            .try_get_connection(15)
            .expect("descriptor-view: opening connection to parent-owned Redis db 15");
        redis::cmd("SET")
            .arg(&sentinel_key)
            .arg(sentinel_value)
            .exec(&mut conn)
            .expect("descriptor-view: SET sentinel must succeed");

        let read_back: String = redis::cmd("GET")
            .arg(&sentinel_key)
            .query(&mut conn)
            .expect("descriptor-view: GET sentinel must succeed");
        assert_eq!(
            read_back, sentinel_value,
            "descriptor-view sanity: the sentinel we just SET must be readable through the same view"
        );
    }

    redis_control.flush_redis_db(15).await.expect(
        "WorkerExecutorClusterControl::flush_redis_db(15) on the parent-owned Redis must succeed",
    );

    {
        let mut conn = deps
            .redis()
            .try_get_connection(15)
            .expect("descriptor-view: opening connection to parent-owned Redis db 15");
        let exists: bool = redis::cmd("EXISTS")
            .arg(&sentinel_key)
            .query(&mut conn)
            .expect("descriptor-view: EXISTS check after flush");
        assert!(
            !exists,
            "RPC-view FLUSHDB on parent-owned Redis db 15 must drop the sentinel \
             `{sentinel_key}` that the descriptor view wrote — if EXISTS is still \
             true, the descriptor and RPC views are not pointing at the same \
             parent-owned Redis instance"
        );
    }
}
