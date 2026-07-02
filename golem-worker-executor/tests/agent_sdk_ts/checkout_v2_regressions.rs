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

use crate::Tracing;
use axum::Router;
use axum::routing::post;
use golem_common::model::AgentStatus;
use golem_common::{agent_id, data_value};
use golem_test_framework::dsl::TestDsl;
use golem_worker_executor_test_utils::{
    LastUniqueId, PrecompiledComponent, TestContext, TestExecutorOverrides,
    WorkerExecutorTestDependencies, start_with_overrides,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{inherit_test_dep, test, timeout};
use tokio::net::TcpListener;
use tracing::Instrument;

use super::manifest_http_5xx_retry_policy;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_sdk_ts")]
    PrecompiledComponent
);

// ===========================================================================
// CheckoutAgentV2 / chaos-backend reproducers
//
// The four tests below mirror the four shell scripts in the user's
// `~/projects/craft/golem/scripts/` directory:
//
//   s1-payment-failure.sh   → ts_v2_s1_payment_failure_then_reset
//   s2-shipment-hang.sh     → ts_v2_s2_shipment_hangs_then_reset
//   s3-process-crash.sh     → ts_v2_s3_process_crash_mid_workflow
//   s4-high-chaos.sh        → ts_v2_s4_sustained_70_percent_chaos
//
// They share:
//   - a `CheckoutAgentV2` agent that does four sequential POSTs
//     (`/inventory/reserve`, `/payment/charge`, `/shipment/create`,
//     `/email/send`) with 10s `AbortController` deadlines and JSON bodies,
//   - a Rust port of `chaos-backend/server.ts` (the same four routes plus
//     per-endpoint `failureRate`/`latencyMs`/`hang` chaos config that the
//     test harness can mutate at runtime), and
//   - a manifest `http-5xx-retry` policy keyed on `status-code in
//     {500,502,503,504}` matching the user's `golem.yaml`
//     `retryPolicyDefaults`.
// ===========================================================================

#[derive(Clone, Default)]
struct EndpointChaos {
    failure_rate: f64,
    latency_ms: u64,
    hang: bool,
}

#[derive(Clone, Copy)]
enum Endpoint {
    Inventory,
    Payment,
    Shipment,
    Email,
}

/// Per-endpoint *request-arrival* counters. Bumped at the very top of
/// each business handler — BEFORE chaos is applied — so they include
/// hung, failed, and successful requests alike. Used to detect the
/// tight replay/retry loops the user observed (where the chaos log
/// fills with thousands of hits even though the agent never makes
/// durable progress).
#[derive(Default)]
struct AttemptCounts {
    inventory: AtomicUsize,
    payment: AtomicUsize,
    shipment: AtomicUsize,
    email: AtomicUsize,
}

#[derive(Default)]
struct ChaosState {
    inventory: EndpointChaos,
    payment: EndpointChaos,
    shipment: EndpointChaos,
    email: EndpointChaos,
}

/// Per-endpoint count of *successful* (chaos-passed, body-consumed,
/// 200-responded) calls, mirroring the original node.js chaos-backend's
/// `log[name]` length used by the tests' assertions.
#[derive(Default)]
struct LogState {
    inventory: usize,
    payment: usize,
    shipment: usize,
    email: usize,
}

/// Inner shared state of the embedded chaos-backend. Cloned into both
/// the test-facing `ChaosBackend` handle and the axum router so config
/// mutations are visible to in-flight handler invocations.
#[derive(Clone)]
struct ChaosInner {
    attempts: Arc<AttemptCounts>,
    chaos: Arc<Mutex<ChaosState>>,
    log: Arc<Mutex<LogState>>,
}

impl ChaosInner {
    fn read_chaos(&self, ep: Endpoint) -> EndpointChaos {
        let g = self.chaos.lock().unwrap();
        match ep {
            Endpoint::Inventory => g.inventory.clone(),
            Endpoint::Payment => g.payment.clone(),
            Endpoint::Shipment => g.shipment.clone(),
            Endpoint::Email => g.email.clone(),
        }
    }

    fn bump_attempt(&self, ep: Endpoint) {
        let counter = match ep {
            Endpoint::Inventory => &self.attempts.inventory,
            Endpoint::Payment => &self.attempts.payment,
            Endpoint::Shipment => &self.attempts.shipment,
            Endpoint::Email => &self.attempts.email,
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    fn bump_log(&self, ep: Endpoint) {
        let mut g = self.log.lock().unwrap();
        let slot = match ep {
            Endpoint::Inventory => &mut g.inventory,
            Endpoint::Payment => &mut g.payment,
            Endpoint::Shipment => &mut g.shipment,
            Endpoint::Email => &mut g.email,
        };
        *slot += 1;
    }
}

/// Embedded in-process chaos-backend handle returned by
/// `start_chaos_backend`. Holds:
///   - shared chaos config that handlers consult on every request,
///   - per-endpoint attempt counters,
///   - per-endpoint successful-call counters,
///   - an `Arc<ServerShutdown>` that aborts the axum server when the
///     last `ChaosBackend` clone is dropped (matching the kill-on-drop
///     semantics of the previous `tokio::process::Child` handle).
#[derive(Clone)]
struct ChaosBackend {
    inner: ChaosInner,
    _shutdown: Arc<ServerShutdown>,
}

struct ServerShutdown {
    abort: tokio::task::AbortHandle,
}

impl Drop for ServerShutdown {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl ChaosBackend {
    fn attempts_snapshot(&self) -> (usize, usize, usize, usize) {
        let a = &self.inner.attempts;
        (
            a.inventory.load(Ordering::SeqCst),
            a.payment.load(Ordering::SeqCst),
            a.shipment.load(Ordering::SeqCst),
            a.email.load(Ordering::SeqCst),
        )
    }

    fn set(&self, ep: Endpoint, c: EndpointChaos) {
        let mut g = self.inner.chaos.lock().unwrap();
        match ep {
            Endpoint::Inventory => g.inventory = c,
            Endpoint::Payment => g.payment = c,
            Endpoint::Shipment => g.shipment = c,
            Endpoint::Email => g.email = c,
        }
    }

    async fn set_inventory(&self, c: EndpointChaos) {
        self.set(Endpoint::Inventory, c);
    }
    async fn set_payment(&self, c: EndpointChaos) {
        self.set(Endpoint::Payment, c);
    }
    async fn set_shipment(&self, c: EndpointChaos) {
        self.set(Endpoint::Shipment, c);
    }
    async fn set_email(&self, c: EndpointChaos) {
        self.set(Endpoint::Email, c);
    }

    /// Mirrors the node.js `/_chaos/reset` route: clears all chaos
    /// config across every endpoint. Like the original it does NOT clear
    /// the `log` counters — successful-call counts accumulate for the
    /// life of the backend.
    async fn reset(&self) {
        *self.inner.chaos.lock().unwrap() = ChaosState::default();
    }

    /// Returns `(inventory, payment, shipment, email)` *successful* call
    /// counts as observed by the chaos-backend (chaos-passed, body
    /// consumed, 200 responded). For total wire-level attempt counts
    /// (including hung/failed requests) use `attempts_snapshot`.
    async fn snapshot(&self) -> (usize, usize, usize, usize) {
        let g = self.inner.log.lock().unwrap();
        (g.inventory, g.payment, g.shipment, g.email)
    }
}

fn rand_id8() -> String {
    format!("{:08x}", rand::random::<u32>())
}

/// Per-endpoint business handler. Mirrors the node.js chaos-backend's
/// `applyChaos` semantics exactly:
///   - bump the wire-level attempt counter first (matches the previous
///     TCP-relay sniffing that incremented before the request reached
///     the server),
///   - apply latency,
///   - if `hang`, never respond and never consume the request body,
///   - if a synthetic failure fires, return 500 *without* consuming the
///     request body (matches the node.js `send(res, 500, ...); return false`
///     wire shape that the previous TCP relay was preserving),
///   - otherwise read the body, bump the success counter, return a 200
///     response shaped like the original handler.
async fn business_handler(
    state: ChaosInner,
    ep: Endpoint,
    req: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    state.bump_attempt(ep);

    let cfg = state.read_chaos(ep);

    if cfg.latency_ms > 0 {
        tokio::time::sleep(Duration::from_millis(cfg.latency_ms)).await;
    }

    if cfg.hang {
        // Never respond — mirrors node.js handler returning false from
        // `applyChaos` and not calling `res.end()`.
        std::future::pending::<()>().await;
        unreachable!();
    }

    if cfg.failure_rate > 0.0 && rand::random::<f64>() < cfg.failure_rate {
        return chaos_failure_response();
    }

    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(b) => b,
        Err(_) => return chaos_failure_response(),
    };
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes)
        .unwrap_or(serde_json::Value::Object(Default::default()));

    state.bump_log(ep);

    let order_id = body_json
        .get("orderId")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let id = rand_id8();
    let resp_body = match ep {
        Endpoint::Inventory => serde_json::json!({
            "reservationId": format!("res_{id}"),
            "orderId": order_id,
        }),
        Endpoint::Payment => {
            let amount = body_json
                .get("amount")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let currency = body_json
                .get("currency")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::String("USD".to_string()));
            serde_json::json!({
                "paymentId": format!("pay_{id}"),
                "orderId": order_id,
                "amount": amount,
                "currency": currency,
            })
        }
        Endpoint::Shipment => {
            let tracking = format!(
                "1Z{:012X}",
                rand::random::<u64>() & 0x0000_FFFF_FFFF_FFFFu64
            );
            serde_json::json!({
                "shipmentId": format!("shp_{id}"),
                "trackingNumber": tracking,
                "orderId": order_id,
            })
        }
        Endpoint::Email => {
            let to = body_json
                .get("to")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "messageId": format!("msg_{id}"),
                "to": to,
            })
        }
    };

    axum::response::Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(resp_body.to_string()))
        .unwrap()
}

fn chaos_failure_response() -> axum::response::Response {
    axum::response::Response::builder()
        .status(500)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"error":"chaos: synthetic failure"}"#,
        ))
        .unwrap()
}

/// Spawn an embedded in-process axum chaos-backend on an ephemeral port.
///
/// Returns `(port, ChaosBackend)`. The caller hands `port` to the agent
/// under test (so it talks to a real HTTP server at the wire level) and
/// uses the `ChaosBackend` handle to mutate chaos config (`set_*` /
/// `reset`) and read counts (`snapshot` / `attempts_snapshot`). The
/// embedded server is aborted when the last `ChaosBackend` clone is
/// dropped, mirroring the kill-on-drop semantics of the previous
/// `npx tsx server.ts` child process.
async fn start_chaos_backend() -> (u16, ChaosBackend) {
    let inner = ChaosInner {
        attempts: Arc::new(AttemptCounts::default()),
        chaos: Arc::new(Mutex::new(ChaosState::default())),
        log: Arc::new(Mutex::new(LogState::default())),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let make_route = |ep: Endpoint| {
        let s = inner.clone();
        move |req: axum::http::Request<axum::body::Body>| {
            let s = s.clone();
            async move { business_handler(s, ep, req).await }
        }
    };

    let app = Router::new()
        .route("/inventory/reserve", post(make_route(Endpoint::Inventory)))
        .route("/payment/charge", post(make_route(Endpoint::Payment)))
        .route("/shipment/create", post(make_route(Endpoint::Shipment)))
        .route("/email/send", post(make_route(Endpoint::Email)));

    let handle = tokio::spawn(
        async move {
            let _ = axum::serve(listener, app).await;
        }
        .in_current_span(),
    );
    let abort = handle.abort_handle();

    tracing::info!(port, "embedded chaos-backend ready");

    let backend = ChaosBackend {
        inner,
        _shutdown: Arc::new(ServerShutdown { abort }),
    };
    (port, backend)
}

/// Common executor overrides for the four V2 scenarios:
/// `manifest-5xx-retry` policy with a 2s periodic delay (matching the
/// user's `golem.yaml`), `max_retries: 10000` (also matching the user's
/// `countBox` budget). Worker-level trap retry is left at its default
/// because the user did not customize it.
fn v2_overrides() -> TestExecutorOverrides {
    TestExecutorOverrides {
        retry_policies: Some(vec![manifest_http_5xx_retry_policy(
            "manifest-5xx-retry",
            // Matches the `periodic: "2s"` in the user's `golem.yaml`
            // `retryPolicyDefaults.local.http-5xx-retry`.
            Duration::from_secs(2),
        )]),
        ..Default::default()
    }
}

/// Args for `CheckoutAgentV2.checkout(host, port, customerEmail, amount,
/// address, sku, qty)`. Centralized so the four tests stay in lockstep.
fn checkout_args(host: &str, port: u16) -> golem_common::schema::TypedSchemaValue {
    data_value!(
        host,
        port as f64,
        "alice@example.com",
        42.0_f64,
        "1 Main St",
        "sku-1",
        1.0_f64,
    )
}

/// ---------------------------------------------------------------------------
/// Scenario 1 — 100% payment failure (then chaos reset).
///
/// Mirrors `~/projects/craft/golem/scripts/s1-payment-failure.sh`:
///   1. Set `/payment/charge` `failureRate = 1.0`.
///   2. Trigger `CheckoutAgentV2.checkout` (fire-and-forget).
///   3. Hold the failure for 5 seconds, then reset chaos.
///   4. Wait for the agent to reach `Idle`.
///   5. Assert each endpoint succeeded exactly once (final counts =
///      inventory=1, payment=1, shipment=1, email=1).
///
/// What the user observes: V2 fails on the first /payment/charge POST,
/// the host inline status-code retry only fires once, the next 500 escapes
/// to the guest which throws, the default trap-retry exhausts (3 attempts)
/// and the agent goes to `Failed` with payment=0.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s1_payment_failure_then_reset(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s1-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 1. Configure 100% payment failure.
    backend
        .set_payment(EndpointChaos {
            failure_rate: 1.0,
            ..Default::default()
        })
        .await;

    // 2. Spawn the chaos reset on a background task so the invocation runs
    //    concurrently with the chaos manipulation, exactly like the shell
    //    script's fire-and-forget invoke + sleep + reset sequence.
    let backend_for_reset = backend.clone();
    let reset_handle = tokio::spawn(
        async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            tracing::info!("S1: 5s elapsed, resetting chaos so /payment recovers");
            backend_for_reset.reset().await;
        }
        .in_current_span(),
    );

    // 3. Trigger the checkout (we use invoke-and-await rather than
    //    fire-and-forget+poll because both end states are equivalent here:
    //    the user's polling loop just waits for the workflow to finish).
    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let _ = reset_handle.await;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    eprintln!("S1 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}");

    assert!(
        result,
        "S1: agent must complete successfully once /payment recovers"
    );
    assert_eq!(
        inv, 1,
        "S1: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S1: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S1: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S1: email must succeed exactly once, observed {mail}"
    );

    let _ = worker_id;
    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 2 — `/shipment` hangs forever, then chaos reset.
///
/// Mirrors `~/projects/craft/golem/scripts/s2-shipment-hang.sh`:
///   1. Set `/shipment/create` `hang = true`.
///   2. Trigger `CheckoutAgentV2.checkout`.
///   3. Wait 15 seconds — long enough for the agent's 10s
///      `AbortController` deadline to fire at least once.
///   4. Reset chaos so subsequent `/shipment` calls succeed.
///   5. Wait for the agent to reach `Idle`.
///   6. Assert each endpoint succeeded exactly once and that the live
///      `/shipment` request count is bounded — the user observed
///      thousands of in-flight shipment calls in a tight replay loop.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s2_shipment_hangs_then_reset(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s2-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    backend
        .set_shipment(EndpointChaos {
            hang: true,
            ..Default::default()
        })
        .await;

    let backend_for_reset = backend.clone();
    let reset_handle = tokio::spawn(
        async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            tracing::info!("S2: 15s elapsed, resetting chaos so /shipment recovers");
            backend_for_reset.reset().await;
        }
        .in_current_span(),
    );

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let _ = reset_handle.await;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    let (inv_a, pay_a, ship_a, mail_a) = backend.attempts_snapshot();
    eprintln!(
        "S2 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}; \
         attempts: inventory={inv_a} payment={pay_a} shipment={ship_a} email={mail_a}"
    );

    assert!(
        result,
        "S2: agent must complete successfully once /shipment recovers"
    );
    assert_eq!(
        inv, 1,
        "S2: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S2: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S2: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S2: email must succeed exactly once, observed {mail}"
    );

    // Hard upper bound on the *number of /shipment attempts*: the user
    // observed hundreds-to-thousands of in-flight POSTs because every
    // trap-replay re-issues the hung request. With the bug fixed, each
    // failed attempt should be charged against a small bounded retry
    // budget (default trap retry: ~5 attempts) plus the one successful
    // attempt after chaos resets — so 32 is a generous ceiling that any
    // working executor will stay well under and any tight-loop
    // regression will easily exceed.
    assert!(
        ship_a <= 32,
        "S2: /shipment attempt count must be bounded (tight retry loop bug), observed {ship_a}"
    );

    // Regression: the /shipment retries happen inside `atomically(...)`, so
    // every persisted `OplogEntry::Error.retry_from` must point at the
    // active `BeginAtomicRegion` index (BAR) — never at a per-attempt
    // `BeginRemoteWrite` index. If `retry_from` were keyed off the unstable
    // BeginRemoteWrite index, the retry budget would silently reset on
    // every replay and the named retry policy could never exhaust, which is
    // exactly the unbounded-loop bug that the now-removed
    // `lookup_retry_state_with_replay_aggregation` fallback used to mask.
    let oplog = executor
        .get_oplog(&worker_id, golem_common::model::oplog::OplogIndex::INITIAL)
        .await?;
    let begin_atomic_region_indices: std::collections::HashSet<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            golem_common::model::oplog::PublicOplogEntry::BeginAtomicRegion(_) => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    // Scope-`Start` entries (e.g. the batched-write scope opened around
    // `atomically(...)`) carry no `request` payload and a synthetic
    // `<scope:batched-write>` function name.
    let begin_remote_write_indices: std::collections::HashSet<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            golem_common::model::oplog::PublicOplogEntry::Start(params)
                if params.request.is_none() && params.function_name == "<scope:batched-write>" =>
            {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    let mut error_entries_checked = 0usize;
    for e in &oplog {
        if let golem_common::model::oplog::PublicOplogEntry::Error(params) = &e.entry {
            assert!(
                !begin_remote_write_indices.contains(&params.retry_from),
                "S2: Error.retry_from must NOT point to a BeginRemoteWrite index \
                 (would mean unstable per-attempt retry identity); \
                 retry_from={:?}, error={:?}",
                params.retry_from,
                params.error,
            );
            assert!(
                begin_atomic_region_indices.contains(&params.retry_from),
                "S2: Error.retry_from must point to the active atomic region's \
                 BeginAtomicRegion index; retry_from={:?}, error={:?}, BAR indices={:?}",
                params.retry_from,
                params.error,
                begin_atomic_region_indices,
            );
            error_entries_checked += 1;
        }
    }
    assert!(
        error_entries_checked > 0,
        "S2: expected at least one persisted Error entry from the shipment retries, found none"
    );

    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 3 — process crash mid-workflow.
///
/// Mirrors `~/projects/craft/golem/scripts/s3-process-crash.sh`:
///   1. Set `/email/send` `latencyMs = 15000` so the agent is still in the
///      middle of the email POST when we crash it.
///   2. Trigger `CheckoutAgentV2.checkout`.
///   3. Wait 5 seconds (so inventory/payment/shipment have completed).
///   4. `simulated_crash` to abruptly kill the worker.
///   5. Reset chaos so the email step does not re-wait 15 seconds on replay.
///   6. Wait for the agent to reach `Idle`.
///   7. Assert each endpoint was called live exactly once — Golem replays
///      the oplog, so /inventory, /payment, /shipment must NOT be re-issued,
///      and the user's observed "thousands of /email calls" replay loop
///      must not occur.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s3_process_crash_mid_workflow(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s3-v2");
    let worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 15s artificial latency on /email/send so the agent is still in the
    // middle of the email POST when we crash.
    backend
        .set_email(EndpointChaos {
            latency_ms: 15_000,
            ..Default::default()
        })
        .await;

    // Fire-and-forget: invoke the checkout, then crash the worker mid-flight.
    executor
        .invoke_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?;

    tokio::time::sleep(Duration::from_secs(5)).await;

    let (inv_pre, pay_pre, ship_pre, mail_pre) = backend.snapshot().await;
    eprintln!(
        "S3 pre-crash counts: inventory={inv_pre} payment={pay_pre} shipment={ship_pre} email={mail_pre}"
    );

    tracing::info!("S3: simulating crash mid-workflow");
    executor.simulated_crash(&worker_id).await?;

    // Reset chaos so the email step does not re-wait 15 seconds on replay.
    backend.reset().await;

    // Wait for the agent to settle.
    let metadata = executor
        .wait_for_statuses(
            &worker_id,
            &[AgentStatus::Idle, AgentStatus::Failed],
            Duration::from_secs(60),
        )
        .await?;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    let (inv_a, pay_a, ship_a, mail_a) = backend.attempts_snapshot();
    eprintln!(
        "S3 final counts: inventory={inv} payment={pay} shipment={ship} email={mail} \
         status={:?}; attempts: inventory={inv_a} payment={pay_a} shipment={ship_a} email={mail_a}",
        metadata.status
    );

    assert_eq!(
        metadata.status,
        AgentStatus::Idle,
        "S3: agent must reach Idle (recovered via oplog replay), got {:?}",
        metadata.status
    );
    assert_eq!(
        inv, 1,
        "S3: /inventory must NOT be re-executed after crash, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S3: /payment must NOT be re-executed after crash, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S3: /shipment must NOT be re-executed after crash, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S3: /email must succeed exactly once after replay (no tight retry loop), observed {mail}"
    );

    // Hard upper bound on /email attempts: the user observed thousands
    // of in-flight POSTs to /email after a mid-call crash because every
    // replay re-issued the in-flight request. With the bug fixed, the
    // total /email attempt count must stay bounded.
    assert!(
        mail_a <= 32,
        "S3: /email attempt count must be bounded (tight retry loop bug), observed {mail_a}"
    );

    Ok(())
}

/// ---------------------------------------------------------------------------
/// Scenario 4 — sustained 70% failure rate on all four endpoints.
///
/// Mirrors `~/projects/craft/golem/scripts/s4-high-chaos.sh`:
///   1. Set `failureRate = 0.7` on every endpoint.
///   2. Trigger `CheckoutAgentV2.checkout` (chaos stays on the whole time).
///   3. Wait for the agent to reach `Idle`.
///   4. Assert each endpoint succeeded exactly once.
///
/// What the user observes: V2 typically fails on the very first /inventory
/// POST. The host status-code retry fires once, the next 500 reaches user
/// code which throws, the default trap-retry policy gives up after 3
/// attempts and the agent ends in `Failed` with all four counts at 0.
/// ---------------------------------------------------------------------------
#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn ts_v2_s4_sustained_70_percent_chaos(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    _tracing: &Tracing,
    #[tagged_as("agent_sdk_ts")] agent_sdk_ts: &PrecompiledComponent,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start_with_overrides(deps, &context, v2_overrides()).await?;

    let (port, backend) = start_chaos_backend().await;

    let component = executor
        .component_dep(&context.default_environment_id, agent_sdk_ts)
        .store()
        .await?;

    let agent_id = agent_id!("CheckoutAgentV2", "ord-s4-v2");
    let _worker_id = executor
        .start_agent_with(&component.id, agent_id.clone(), HashMap::new(), Vec::new())
        .await?;

    // 70% failure rate on all four endpoints — sustained for the full
    // duration of the workflow.
    let chaos = EndpointChaos {
        failure_rate: 0.7,
        ..Default::default()
    };
    backend.set_inventory(chaos.clone()).await;
    backend.set_payment(chaos.clone()).await;
    backend.set_shipment(chaos.clone()).await;
    backend.set_email(chaos).await;

    let result = executor
        .invoke_and_await_agent(
            &component,
            &agent_id,
            "checkout",
            checkout_args("127.0.0.1", port),
        )
        .await?
        .into_typed::<bool>()?;

    let (inv, pay, ship, mail) = backend.snapshot().await;
    eprintln!("S4 final counts: inventory={inv} payment={pay} shipment={ship} email={mail}");

    assert!(
        result,
        "S4: agent must complete successfully despite 70% failure rate"
    );
    assert_eq!(
        inv, 1,
        "S4: inventory must succeed exactly once, observed {inv}"
    );
    assert_eq!(
        pay, 1,
        "S4: payment must succeed exactly once, observed {pay}"
    );
    assert_eq!(
        ship, 1,
        "S4: shipment must succeed exactly once, observed {ship}"
    );
    assert_eq!(
        mail, 1,
        "S4: email must succeed exactly once, observed {mail}"
    );

    Ok(())
}
