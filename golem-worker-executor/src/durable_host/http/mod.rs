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

use crate::durable_host::concurrent::finish_span_in_memory;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::{DurableFunctionType, SpanFinished};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::warn;

pub mod inline_retry;
mod mcp;
pub mod outgoing_http;
pub(crate) mod policy;
pub mod types;

pub(crate) async fn end_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
) -> Result<(), WorkerExecutorError> {
    if let Some(state) = ctx.state.open_http_requests.remove(&current_handle) {
        let span_finished = SpanFinished {
            span_id: state.session.span_id().clone(),
            finished_at: golem_common::model::Timestamp::now_utc(),
            outcome: state.session.outcome(),
        };
        if state.session.persisted() {
            ctx.end_durable_function_with_span(
                &DurableFunctionType::WriteRemoteBatched(None),
                state.begin_index(),
                false,
                span_finished,
            )
            .await?;
        }

        state.session.mark_scope_closed();
        finish_span_in_memory(ctx, state.session.span_id())?;
        state.session.mark_closed();
    } else {
        warn!(
            "No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}",
            current_handle, ctx.state.open_http_requests
        );
    }

    Ok(())
}

pub(crate) fn continue_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
    new_handle: u32,
) {
    if let Some(state) = ctx.state.open_http_requests.remove(&current_handle) {
        ctx.state.open_http_requests.insert(new_handle, state);
    } else {
        warn!(
            "No matching HTTP request is associated with resource handle. Handle: {}, open requests: {:?}",
            current_handle, ctx.state.open_http_requests
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::durable_host::HttpRequestSession;
    use crate::durable_host::concurrent::DropEvent;
    use golem_common::model::invocation_context::SpanId;
    use golem_common::model::oplog::{DurableFunctionType, OplogIndex, SpanOutcome};
    use test_r::test;

    #[test]
    fn cloned_http_session_closes_only_after_its_final_owner_drops() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session =
            HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), true, Some(tx));
        let response_owner = session.clone();
        let body_owner = response_owner.clone();

        drop(session);
        drop(response_owner);
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));

        drop(body_owner);
        match rx.try_recv().expect("the final owner must defer closure") {
            DropEvent::CloseDurableScope {
                function_type,
                begin_index,
                span_finished: Some(span_finished),
            } => {
                assert_eq!(function_type, DurableFunctionType::WriteRemoteBatched(None));
                assert_eq!(begin_index, OplogIndex::INITIAL);
                assert_eq!(
                    span_finished.outcome,
                    golem_common::model::oplog::SpanOutcome::Cancelled
                );
            }
            other => panic!("expected a durable-scope close event, got {other:?}"),
        }
    }

    #[test]
    fn synchronous_http_drop_enqueues_scope_close_exactly_once() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session =
            HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), true, Some(tx));

        session.defer_close();
        drop(session);

        assert!(matches!(
            rx.try_recv(),
            Ok(DropEvent::CloseDurableScope { .. })
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failed_in_memory_span_finish_defers_only_the_remaining_span_work() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session =
            HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), true, Some(tx));

        session.mark_scope_closed();
        drop(session);

        assert!(matches!(rx.try_recv(), Ok(DropEvent::FinishSpan { .. })));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn http_outcome_survives_owner_transfer_and_later_eof() {
        for (observations, expected) in [
            (vec![SpanOutcome::Completed], SpanOutcome::Completed),
            (
                vec![SpanOutcome::Failed, SpanOutcome::Completed],
                SpanOutcome::Failed,
            ),
            (
                vec![SpanOutcome::Completed, SpanOutcome::Failed],
                SpanOutcome::Failed,
            ),
        ] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let session =
                HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), true, Some(tx));
            let owner = session.clone();
            for outcome in observations {
                session.record_outcome(outcome);
            }
            assert_eq!(owner.outcome(), expected);
            drop(session);
            drop(owner);
            match rx.try_recv().unwrap() {
                DropEvent::CloseDurableScope {
                    span_finished: Some(span),
                    ..
                } => assert_eq!(span.outcome, expected),
                other => panic!("expected scope close, got {other:?}"),
            }
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn snapshot_http_session_never_enqueues_a_durable_close() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session =
            HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), false, Some(tx));
        session.defer_close();
        drop(session);
        assert!(matches!(rx.try_recv(), Ok(DropEvent::FinishSpan { .. })));
        assert!(rx.try_recv().is_err());
    }
}
