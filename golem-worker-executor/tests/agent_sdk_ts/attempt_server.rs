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

use axum::Router;
use axum::extract::State;
use axum::routing::{get, post};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tracing::Instrument;

#[derive(Clone)]
struct AttemptCounterState {
    counter: Arc<AtomicUsize>,
    fail_count: usize,
}

async fn attempt_handler(State(state): State<AttemptCounterState>) -> axum::response::Response {
    let attempt = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    if attempt <= state.fail_count {
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        axum::response::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}

pub(crate) async fn start_attempt_counter_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let state = AttemptCounterState {
        counter: counter.clone(),
        fail_count,
    };
    let app = Router::new()
        .route("/attempt", get(attempt_handler))
        .route("/attempt-post", post(attempt_post_handler))
        .with_state(state);
    let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(
        async move {
            axum::serve(listener, app).await.unwrap();
        }
        .in_current_span(),
    );
    (port, counter)
}

/// POST counterpart of `attempt_handler` that mirrors the user's
/// chaos-backend exactly: on the failure path it returns 500
/// **immediately, without reading the request body**, then closes the
/// response. The success path consumes the body and returns 200.
///
/// This shape matches `applyChaos` in `chaos-backend/server.ts`:
///
///   if (cfg.failureRate && Math.random() < cfg.failureRate) {
///     send(res, 500, { error: "chaos: synthetic failure" });
///     return false;       // body never consumed
///   }
///
/// Returning 500 mid-request without consuming the body is what makes the
/// host inline retry resend path go wrong — on the resent request the
/// receiving HTTP server replies with HTTP 400 (no Content-Type, empty
/// body), the guest sees a non-5xx response, throws, traps, and the
/// default trap policy gives up.
async fn attempt_post_handler(
    State(state): State<AttemptCounterState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let attempt = state.counter.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(attempt, "attempt-post handler received request");
    if attempt <= state.fail_count {
        // Failure path: respond 500 *without* consuming the request body.
        // Drop the request (and therefore its body) immediately.
        drop(request);
        axum::response::Response::builder()
            .status(500)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        // Success path: drain the body, then respond 200.
        let _ = axum::body::to_bytes(request.into_body(), 64 * 1024).await;
        axum::response::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap()
    }
}
