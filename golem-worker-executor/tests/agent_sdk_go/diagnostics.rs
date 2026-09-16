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

//! Oplog-dumping variants of the Go tests that fail or diverge, following the
//! `understanding-durable-execution` debugging workflow: get the oplog, walk the
//! `Start`/`End`/marker pairs, name the exact host input. All ignored — they are
//! investigation tools, run on demand with `--include-ignored`, and write their
//! dumps under `tmp/oplogs/` for offline diffing.

use crate::Tracing;
use axum::Router;
use axum::extract::Query;
use axum::routing::get;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntry, PublicOplogEntryWithIndex};
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, WorkerExecutorTestDependencies, start,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use test_r::{inherit_test_dep, test, timeout};
use tracing::Instrument;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_go")]
    PrecompiledComponent
);

/// Renders one oplog entry per line: index, kind, and the identity fields that
/// matter for pairing (function name, start index, request presence). Anything
/// else is elided so two dumps diff cleanly.
fn render(entries: &[PublicOplogEntryWithIndex]) -> String {
    let mut out = String::new();
    for e in entries {
        let line = match &e.entry {
            PublicOplogEntry::Start(p) => format!(
                "Start {} req={}",
                p.function_name,
                if p.request.is_some() { "some" } else { "none" }
            ),
            // random responses are the runtime's PRNG seed: show their payload so
            // seed stability across runs can be checked by eye.
            PublicOplogEntry::End(p) if p.response.as_ref().is_some_and(|_| {
                entries.iter().any(|s| {
                    s.oplog_index == p.start_index
                        && matches!(&s.entry, PublicOplogEntry::Start(sp) if sp.function_name.contains("random"))
                })
            }) => format!("End   start={} response={:?}", p.start_index, p.response),
            PublicOplogEntry::End(p) => format!("End   start={}", p.start_index),
            PublicOplogEntry::Cancelled(p) => format!("Cancelled start={}", p.start_index),
            other => {
                let dbg = format!("{other:?}");
                // keep only the variant name for everything else
                dbg.split(['(', ' ', '{']).next().unwrap_or("?").to_string()
            }
        };
        out.push_str(&format!("{:>5} {}\n", e.oplog_index, line));
    }
    out
}

fn dump_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("tmp")
        .join("oplogs");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_dump(name: &str, text: &str) {
    let path = dump_dir().join(name);
    std::fs::write(&path, text).unwrap();
    eprintln!("oplog dump: {}", path.display());
}

/// GOL-486: what is still open when the atomic region tries to close? Dumps the
/// oplog regardless of outcome; the survivor is any `Start` inside the region
/// with no `End`/`Cancelled` below the failure point.
#[test]
#[ignore = "diagnostic: dumps the oplog for GOL-486"]
#[tracing::instrument]
#[timeout("2m")]
async fn diag_atomic_region_with_outgoing_http(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;

    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    #[derive(Deserialize)]
    struct QueryParams {
        payload: String,
    }

    let server = tokio::spawn(
        async move {
            let route = Router::new().route(
                "/callback",
                get(move |query: Query<QueryParams>| async move { query.payload.clone() }),
            );
            axum::serve(listener, route).await.unwrap();
        }
        .in_current_span(),
    );

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("HttpAgent", "go-diag-atomic-1");
    let mut env = HashMap::new();
    env.insert("PORT".to_string(), port.to_string());
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), env, Vec::new())
        .await?;

    // Bound the invocation so a hang still yields a dump of the oplog as it
    // stands while the guest is stuck.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        executor.invoke_and_await_agent(
            &component,
            &agent_id,
            &std::env::var("DIAG_METHOD").unwrap_or_else(|_| "atomic-callback".to_string()),
            data_value!("inside"),
        ),
    )
    .await;

    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let run = std::env::var("DIAG_RUN").unwrap_or_default();
    write_dump(&format!("gol486-atomic-http{run}.txt"), &render(&oplog));

    drop(executor);
    server.abort();
    eprintln!(
        "invocation outcome: {}",
        match &result {
            Ok(Ok(_)) => "ok".to_string(),
            Ok(Err(e)) => format!("err: {e}"),
            Err(_) => "HANG (timed out after 20s)".to_string(),
        }
    );
    Ok(())
}

/// GOL-485 residual: dumps the LIVE recording (after the bumps, before the
/// restart) and the oplog after the restart, so passing and failing runs can be
/// diffed — is the recording itself different run to run (guest input not
/// recorded), or identical while replay diverges (executor)?
#[test]
#[ignore = "diagnostic: dumps oplogs around the snapshot recovery divergence"]
#[tracing::instrument]
#[timeout("2m")]
async fn diag_snapshot_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    use golem_worker_executor::services::golem_config::SnapshotPolicy;
    use golem_worker_executor_test_utils::start_with_snapshot_policy;

    let run = std::env::var("DIAG_RUN").unwrap_or_default();
    let context = TestContext::new(last_unique_id);
    let policy = SnapshotPolicy::EveryNInvocation { count: 2 };
    let executor = start_with_snapshot_policy(deps, &context, policy.clone()).await?;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SnapAgent", "go-diag-snapshot-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    for _ in 0..10 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "bump", data_value!())
            .await?;
    }
    let live = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    write_dump(&format!("gol485-snapshot{run}-live.txt"), &render(&live));

    drop(executor);
    let executor = start_with_snapshot_policy(deps, &context, policy).await?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(25),
        executor.invoke_and_await_agent(&component, &agent_id, "value", data_value!()),
    )
    .await;
    let after = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    write_dump(&format!("gol485-snapshot{run}-after.txt"), &render(&after));
    drop(executor);

    eprintln!(
        "invocation outcome: {}",
        match &result {
            Ok(Ok(v)) => format!("ok ({:?})", v.clone().into_typed::<i64>()),
            Ok(Err(e)) => format!("err: {e}"),
            Err(_) => "HANG (timed out after 25s)".to_string(),
        }
    );
    Ok(())
}

/// Scheduling attribution: goroutine spawns, running->X transitions and clock
/// reads during 10 bumps (instrumented fork + temporary `sched-trace` method).
/// Also dumps the oplog so per-invocation clock counts can be aligned with the
/// scheduler sequence.
#[test]
#[ignore = "diagnostic: needs the instrumented go fork"]
#[tracing::instrument]
#[timeout("2m")]
async fn diag_sched_trace(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SnapAgent", "go-diag-sched-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    for _ in 0..10 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "bump", data_value!())
            .await?;
    }
    let report = executor
        .invoke_and_await_agent(&component, &agent_id, "sched-trace", data_value!())
        .await?
        .into_typed::<String>()?;
    let oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let run = std::env::var("DIAG_RUN").unwrap_or_default();
    write_dump(&format!("sched{run}.txt"), &format!("{report}=== oplog ===\n{}", render(&oplog)));
    drop(executor);
    Ok(())
}

/// Replay attribution: run 10 bumps, restart, then read the scheduler trace of
/// the REPLAYED instance (its spawns/transitions/clock reads while replaying
/// the same 10 bumps) and compare with the live trace of the same run. If the
/// recorded seed is replayed, the two traces must be identical.
#[test]
#[ignore = "diagnostic: needs the instrumented go fork"]
#[tracing::instrument]
#[timeout("3m")]
async fn diag_sched_trace_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_go")] agent_sdk_go: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let run = std::env::var("DIAG_RUN").unwrap_or_default();
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_go)
        .store()
        .await?;
    let agent_id = agent_id!("SnapAgent", "go-diag-sched-replay-1");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;
    for _ in 0..10 {
        executor
            .invoke_and_await_agent(&component, &agent_id, "bump", data_value!())
            .await?;
    }
    let live = executor
        .invoke_and_await_agent(&component, &agent_id, "sched-trace", data_value!())
        .await?
        .into_typed::<String>()?;
    let live_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    write_dump(&format!("replay{run}-live.txt"), &format!("{live}=== oplog ===\n{}", render(&live_oplog)));

    drop(executor);
    let executor = start(deps, &context).await?;
    // The replayed instance re-runs the 10 bumps AND the recorded sched-trace
    // invocation (whose recorded result is returned, not recomputed), then this
    // new sched-trace runs live and reports the replay-phase events.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(40),
        executor.invoke_and_await_agent(&component, &agent_id, "sched-trace", data_value!()),
    )
    .await;
    let after_oplog = executor.get_oplog(&worker_id, OplogIndex::INITIAL).await?;
    let replay = match &result {
        Ok(Ok(v)) => v.clone().into_typed::<String>().unwrap_or_default(),
        Ok(Err(e)) => format!("ERR: {e}\n"),
        Err(_) => "HANG\n".to_string(),
    };
    write_dump(&format!("replay{run}-replay.txt"), &format!("{replay}=== oplog ===\n{}", render(&after_oplog)));
    drop(executor);
    Ok(())
}
