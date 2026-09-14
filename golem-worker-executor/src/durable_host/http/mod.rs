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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::{InvocationContextManagement, WorkerCtx};
use golem_common::model::oplog::DurableFunctionType;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use tracing::warn;

pub mod inline_retry;
pub mod outgoing_http;
pub(crate) mod policy;
pub mod types;

pub(crate) async fn end_http_request<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    current_handle: u32,
) -> Result<(), WorkerExecutorError> {
    if let Some(state) = ctx.state.open_http_requests.remove(&current_handle) {
        ctx.end_durable_function(
            &DurableFunctionType::WriteRemoteBatched(None),
            state.begin_index(),
            false,
        )
        .await?;

        state.session.mark_scope_closed();
        ctx.finish_span(state.session.span_id()).await?;
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
    use golem_common::model::oplog::{DurableFunctionType, OplogIndex};
    use test_r::test;

    #[test]
    fn cloned_http_session_closes_only_after_its_final_owner_drops() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), Some(tx));
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
                span_id: Some(_),
            } => {
                assert_eq!(function_type, DurableFunctionType::WriteRemoteBatched(None));
                assert_eq!(begin_index, OplogIndex::INITIAL);
            }
            other => panic!("expected a durable-scope close event, got {other:?}"),
        }
    }

    #[test]
    fn synchronous_http_drop_enqueues_scope_close_exactly_once() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), Some(tx));

        session.defer_close();
        drop(session);

        assert!(matches!(
            rx.try_recv(),
            Ok(DropEvent::CloseDurableScope { .. })
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn failed_span_finish_defers_only_the_remaining_span_work() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = HttpRequestSession::new(OplogIndex::INITIAL, SpanId::generate(), Some(tx));

        session.mark_scope_closed();
        drop(session);

        assert!(matches!(
            rx.try_recv(),
            Ok(DropEvent::FinishSpan { durable: true, .. })
        ));
        assert!(rx.try_recv().is_err());
    }
}
