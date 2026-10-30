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

use super::rebuild::{AbortOnDropIoTask, P3HttpSendRebuild};
use super::rebuild::{RebuildOutcome, ResendOutcome, reissue_recorded_request};
use super::rebuild::{recorded_request_body_replayability, resend_recorded_request};
use super::replay::ReplayedRequestBodyDrainProgress;
use super::send::p3_send_span_finished;
use super::serialization::{deserialize_error_code, serialize_error_code};
use super::serialization::{deserialize_headers, serialize_headers};
use super::*;
use crate::durable_host::concurrent::{
    AccessClaimOptions, CompletionDelivery, DemandDelivery, DemandDeliveryMode, DurableDemandItem,
    DurableDemandStream, InvocationIdSource, ScopeReplayDecision, ScopeReplayRecovery,
    demand_channel, import_invocation_id,
};
use crate::durable_host::durability::CustomInvocationContext;
use crate::durable_host::durability::{
    AsyncRetryDecision, DurabilityHost, DurableCallTrapContext, HostFailureKind,
    InFunctionRetryState, TaskRetryContext, mark_durable_call_trap_context,
    semantic_trap_retry_override_error,
};
use crate::durable_host::http::policy::classify_serializable_http_error_code;
use crate::durable_host::http::policy::{
    ResumeResponseAction, classify_resume_response, has_guest_range_header,
    parse_content_range_start, resume_range_headers,
};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, expect_ctx, observe_function_call,
    observe_function_call_store, wasi_http_view,
};
use crate::durable_host::tail_work::TailActivity;
use crate::workerctx::{P3HttpBodyProducerHook, WorkerCtx};
use bytes::Bytes;
use golem_common::model::RetryContext;
use golem_common::model::oplog::host_functions::{
    P3HttpClientConsumeBody, P3HttpClientConsumeBodyChunk,
};
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult,
};
use golem_common::model::oplog::{
    HostRequestNoInput, HostResponseP3HttpClientConsumeBodyChunk,
    HostResponseP3HttpClientConsumeBodyResult, OplogIndex,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use http::HeaderMap;
use http_body_util::BodyExt as _;
use http_body_util::combinators::UnsyncBoxBody;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::sync::{Notify, mpsc, oneshot};
use tracing::debug;
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureProducer, FutureReader, Resource,
    StreamProducer, StreamReader, StreamResult, TerminalConsumption,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p3::bindings::http::types;
use wasmtime_wasi_http::p3::bindings::http::types::{
    ErrorCode, Headers, Response, StatusCode, Trailers,
};
use wasmtime_wasi_http::p3::{HostBodyStreamProducer, WasiHttp, WasiHttpView};

mod consumer;
mod producer;
mod transport;

pub(super) use consumer::*;
pub(super) use producer::*;
pub(super) use transport::*;

pub(crate) struct OpenP3HttpResponseState {
    /// The `outgoing-http-request` invocation span of the send that produced
    /// this response. Finished when the response body completes (the durable
    /// consume-body terminal) or by a local durable cleanup pair when the
    /// response is dropped unconsumed.
    pub(crate) span: P3HttpSendSpan,
    /// Request method, for retry properties of body-transfer failures.
    pub(crate) method: String,
    /// Request URI, for retry properties of body-transfer failures.
    pub(crate) uri: String,
    /// How to re-send the recorded request without a new guest-visible call.
    /// Used to re-issue the request when a replayed placeholder body's
    /// durable consume-body scope turns out to be incomplete and must
    /// re-execute live, and to resume a failed live body read via a ranged
    /// re-send. Populated on both the live and replay send paths.
    pub(crate) resend: Option<P3HttpSendRebuild>,
    /// True iff the response was replayed from recorded headers and carries
    /// an empty placeholder body: the first live body read must re-issue the
    /// recorded request (via `resend`) to obtain a real body.
    pub(crate) body_is_placeholder: bool,
    /// Custom durable invocation that owns the send and every later body/trailer continuation.
    pub(super) observational_owner: Option<OplogIndex>,
}

pub(super) fn register_open_response<Ctx: WorkerCtx, U: Send>(
    store: &Accessor<U, DurableP3<Ctx>>,
    response: &Resource<Response>,
    state: OpenP3HttpResponseState,
) {
    let rep = response.rep();
    store.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.state.open_p3_http_responses.insert(rep, state);
    });
}

impl<Ctx: WorkerCtx> types::HostResponse for DurableP3View<'_, Ctx> {
    fn get_status_code(&mut self, res: Resource<Response>) -> wasmtime::Result<StatusCode> {
        observe_function_call(&*self.0, "http::types::response", "get-status-code");
        types::HostResponse::get_status_code(&mut WasiHttpView::http(self.0), res)
    }

    fn set_status_code(
        &mut self,
        res: Resource<Response>,
        status_code: StatusCode,
    ) -> wasmtime::Result<Result<(), ()>> {
        observe_function_call(&*self.0, "http::types::response", "set-status-code");
        types::HostResponse::set_status_code(&mut WasiHttpView::http(self.0), res, status_code)
    }

    fn get_headers(&mut self, res: Resource<Response>) -> wasmtime::Result<Resource<Headers>> {
        observe_function_call(&*self.0, "http::types::response", "get-headers");
        types::HostResponse::get_headers(&mut WasiHttpView::http(self.0), res)
    }
}

/// Result fed to the guest-facing trailers `FutureReader` once the body closes.
pub(super) type HttpTrailersOutcome = Result<Option<HeaderMap>, ErrorCode>;

#[derive(Default)]
struct HttpBodyDemandSignal {
    observed: AtomicBool,
    notify: Notify,
}

impl HttpBodyDemandSignal {
    fn observe(&self) {
        self.observed.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    async fn observed(&self) {
        while !self.observed.load(Ordering::Acquire) {
            self.notify.notified().await;
        }
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostResponseWithStore<U> for DurableP3<Ctx> {
    fn new(
        mut store: Access<U, Self>,
        headers: Resource<Headers>,
        contents: Option<StreamReader<u8>>,
        trailers: FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    ) -> wasmtime::Result<(Resource<Response>, FutureReader<Result<(), ErrorCode>>)> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::response",
            "new",
        );
        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore<U>>::new(store, headers, contents, trailers)
    }

    fn consume_body(
        mut store: Access<U, Self>,
        res: Resource<Response>,
        fut: FutureReader<Result<(), ErrorCode>>,
    ) -> wasmtime::Result<(
        StreamReader<u8>,
        FutureReader<Result<Option<Resource<Trailers>>, ErrorCode>>,
    )> {
        // Take ownership of the response's open-response state (if this
        // response came from `client::send`): the durable consume-body task
        // finishes its span when the body reaches its terminal and uses its
        // rebuild info when an incomplete scope must re-execute live. Removing
        // the mapping here also keeps the later `drop` of the response
        // resource from finishing the span a second time.
        let response_state = {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            ctx.state.open_p3_http_responses.remove(&res.rep())
        };

        // Delegate to the built-in implementation to wire `fut` into the body's
        // transmission-result channel and to build the host body stream.
        let (upstream_stream, mut upstream_trailers) = {
            let http_store =
                Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
            <WasiHttp as types::HostResponseWithStore<U>>::consume_body(http_store, res, fut)?
        };

        // Recover the host body producer so we can drive and record the body
        // transfer ourselves. Responses obtained from `client.send` (live or
        // replayed) always carry a host-constructed body, so this succeeds.
        let body =
            match upstream_stream.try_into::<HostBodyStreamProducer<U>>(store.as_context_mut()) {
                Ok(mut producer) => {
                    let body = producer.take_body();
                    // Dropping the now-empty producer resolves the upstream
                    // trailers future (`Ok(None)`), which we discard below.
                    drop(producer);
                    body
                }
                Err(stream) => {
                    // Guest-constructed response body (not from `send`): fall back
                    // to the non-durable passthrough. No state was registered for
                    // such responses (`response_state` is `None` here).
                    debug_assert!(response_state.is_none());
                    return Ok((stream, upstream_trailers));
                }
            };
        // A consume-body inside a custom durable invocation is not claimed on replay, whichever
        // invocation observed the send, so it takes no ordinal from the root counter.
        let observed_by_custom_invocation = store
            .as_context_mut()
            .guest_task_context::<CustomInvocationContext>()?
            .as_deref()
            .and_then(CustomInvocationContext::current)
            .is_some();
        let invocation_id = if observed_by_custom_invocation {
            InvocationIdSource::Reserved(None)
        } else {
            let mut store_ctx = store.as_context_mut();
            import_invocation_id(
                durable_worker_ctx::<Ctx, U>(store_ctx.data_mut()),
                response_state
                    .as_ref()
                    .and_then(|state| state.observational_owner),
            )?
        };

        // We surface trailers through our own future, so discard the built-in
        // trailers future.
        upstream_trailers.close(store.as_context_mut())?;

        // Capacity 1 suffices (and bounds memory as defense in depth): the
        // producer keeps at most one demand in flight at a time.
        let (demand_tx, demand_rx) = demand_channel();
        let demand_signal = Arc::new(HttpBodyDemandSignal::default());
        let (trailers_tx, trailers_rx) = oneshot::channel();
        let trailers_delivery = HttpTrailersDelivery::new();
        let producer_hook = {
            let mut store_ctx = store.as_context_mut();
            expect_ctx::<Ctx, U>(store_ctx.data_mut()).p3_http_body_producer_hook()
        };

        // Build both guest-facing handles before spawning the durable task. The
        // task appends the `consume-body` `Start`; the guest cannot poll either
        // handle until this host call returns, so spawning first would risk
        // committing a `Start` with no terminal (orphaned `Start`) if a later
        // handle construction fails.
        let mut stream = StreamReader::new(
            &mut store,
            DurableHttpBodyProducer::new(demand_tx, demand_signal.clone(), producer_hook),
        )?;
        let mut trailers = match FutureReader::new(
            &mut store,
            HttpTrailersFutureProducer::<Ctx, U>::new(trailers_rx),
        ) {
            Ok(trailers) => trailers,
            Err(err) => {
                let _ = stream.close(store.as_context_mut());
                return Err(err);
            }
        };
        let delivery_guard = HttpTrailersDeliveryGuard::new(trailers_delivery.clone());
        if let Err(err) = trailers.register_terminal_observer(&mut store, move |consumption| {
            delivery_guard.observe(consumption);
        }) {
            let _ = trailers.close(store.as_context_mut());
            let _ = stream.close(store.as_context_mut());
            return Err(err);
        }

        let activity = {
            let mut store_ctx = store.as_context_mut();
            durable_worker_ctx::<Ctx, U>(store_ctx.data_mut())
                .tail_work_tracker()
                .activity()
        };
        store.spawn(HttpConsumeBodyTask::<Ctx>::new(
            body,
            demand_rx,
            demand_signal,
            trailers_tx,
            trailers_delivery,
            response_state,
            invocation_id,
            activity,
        ));
        Ok((stream, trailers))
    }

    fn drop(mut store: Access<U, Self>, res: Resource<Response>) -> wasmtime::Result<()> {
        observe_function_call_store::<Ctx, U>(
            store.as_context_mut().data_mut(),
            "http::types::response",
            "drop",
        );

        // Sample liveness at drop, not at send: replay-created responses can survive into live
        // invocations. Snapshot drops and replay drops only release resident state.
        {
            let mut store_ctx = store.as_context_mut();
            let ctx = durable_worker_ctx::<Ctx, U>(store_ctx.data_mut());
            if let Some(state) = ctx.state.open_p3_http_responses.remove(&res.rep())
                && state.span.claim_closure()
            {
                crate::durable_host::concurrent::HttpSpanCleanupRecorder::new(
                    ctx,
                    state.span.send_start_index,
                    state.span.persisted,
                )
                .record(p3_send_span_finished(
                    &state.span,
                    golem_common::model::oplog::SpanOutcome::Abandoned,
                ));
            }
        }

        let store = Access::<U, WasiHttp>::new(store.as_context_mut(), wasi_http_view::<Ctx, U>);
        <WasiHttp as types::HostResponseWithStore<U>>::drop(store, res)
    }
}
