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

use super::*;
use test_r::{test, timeout};

#[test]
fn body_terminal_maps_to_semantic_span_outcome() {
    assert_eq!(
        body_span_outcome(false, &Ok(None)),
        golem_common::model::oplog::SpanOutcome::Completed
    );
    assert_eq!(
        body_span_outcome(false, &Err(ErrorCode::HttpProtocolError)),
        golem_common::model::oplog::SpanOutcome::Failed
    );
    assert_eq!(
        body_span_outcome(true, &Ok(None)),
        golem_common::model::oplog::SpanOutcome::Cancelled
    );
}

#[test]
#[timeout("10s")]
async fn missing_scope_readiness_refuses_pending_proof_after_response_demand() {
    let demand_signal = Arc::new(HttpBodyDemandSignal::default());
    demand_signal.observe();
    let request_body_progress = Arc::new(ReplayedRequestBodyDrainProgress::default());
    request_body_progress.set_waiting_for_guest_body(true);

    let error = require_recording_before_response_demand(
        std::future::pending::<Result<(), WorkerExecutorError>>(),
        demand_signal,
        Some(request_body_progress),
    )
    .await
    .expect_err("response demand must bound a pending recording proof");

    assert!(error.to_string().contains("response-body demand"));
}

#[test]
async fn missing_scope_readiness_prefers_an_already_completed_proof() {
    let demand_signal = Arc::new(HttpBodyDemandSignal::default());
    demand_signal.observe();
    let request_body_progress = Arc::new(ReplayedRequestBodyDrainProgress::default());
    request_body_progress.set_waiting_for_guest_body(true);

    require_recording_before_response_demand(
        async { Ok(()) },
        demand_signal,
        Some(request_body_progress),
    )
    .await
    .expect("a completed recording proof must win over concurrent response demand");
}

#[test]
async fn missing_scope_readiness_ignores_demand_for_storage_only_validation() {
    let demand_signal = Arc::new(HttpBodyDemandSignal::default());
    demand_signal.observe();
    let request_body_progress = Arc::new(ReplayedRequestBodyDrainProgress::default());
    let (ready_tx, ready_rx) = oneshot::channel();
    let mut check = Box::pin(require_recording_before_response_demand(
        async move {
            ready_rx.await.expect("readiness sender dropped");
            Ok(())
        },
        demand_signal,
        Some(request_body_progress),
    ));

    assert!(
        futures::poll!(check.as_mut()).is_pending(),
        "response demand alone must not refuse bounded storage-only validation"
    );
    ready_tx.send(()).expect("readiness receiver dropped");
    check
        .await
        .expect("storage-only validation must complete after becoming ready");
}
