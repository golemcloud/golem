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

use super::calls::{BindOutcome, CancelledCall, EntryId, RunningCallGuard, RunningCalls};
use super::event_store::{ReplicaEventStore, parse_event_id, sweep_step};
use super::rules::{
    Admission, PinStep, admit, heartbeat_period, pin_step, pin_written, priming_event_id,
};
use super::store::{McpSessionId, McpSessionRecord, McpSessionStore, McpSessionStoreError};
use super::streams::GetStreams;
use crate::config::McpConfig;
use crate::mcp::McpDeployment;
use crate::mcp::serving::{McpServing, ServeKind, ServeRefusal, Serving};
use async_trait::async_trait;
use futures::{FutureExt, StreamExt, stream};
use golem_common::model::deployment::DeploymentRevision;
use golem_service_base::mcp::CompiledMcp;
use poem::http::HeaderValue;
use poem::web::sse::{Event, SSE};
use poem::{Body, IntoResponse, Response};
use rmcp::model::{
    RequestId, ServerJsonRpcMessage, ServerNotification, ToolListChangedNotification,
};
use rmcp::transport::common::http_header::{EVENT_STREAM_MIME_TYPE, HEADER_SESSION_ID};
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use rmcp::transport::streamable_http_server::session::store::EventStore;
use std::convert::Infallible;
use std::future::ready;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::warn;

/// The reconnect delay that every response stream announces, in milliseconds.
pub(crate) const SSE_RETRY_MILLIS: u64 = 3000;
/// How often one shard of the event store and of the GET-stream registry is swept.
const SWEEP_TICK: Duration = Duration::from_secs(1);

/// The session state that one MCP endpoint keeps in its own memory: response events for resume,
/// the running calls for cancel, and the open GET streams. Session records live in the shared
/// store. Each endpoint has its own replica, as each worker-service process has its own.
pub struct McpReplica {
    store: Arc<dyn McpSessionStore>,
    idle: Duration,
    keep_alive: Duration,
    events: Arc<ReplicaEventStore>,
    streams: Arc<GetStreams>,
    calls: Arc<RunningCalls>,
    /// Stops the sweep task when the replica is dropped.
    _sweep: DropGuard,
    #[cfg(test)]
    hooks: hooks::TestHooks,
}

/// What a streamed response body belongs to, which decides how its stream is bound.
#[derive(Clone, Copy)]
enum ResponseOf {
    /// The `initialize` that opened the session: resumable, with no running call.
    Initialize,
    /// A call of the session: resumable, and kept while the call runs.
    Call(EntryId),
}

/// A call of an admitted session, between the request and its response.
pub(super) struct StartedCall {
    replica: Arc<McpReplica>,
    session: McpSessionId,
    entry: EntryId,
    token: CancellationToken,
}

impl StartedCall {
    /// The call's response. A streamed body binds its stream to the session when its first event
    /// passes, and ends at once when the call is cancelled.
    pub(super) fn respond(self, response: Response) -> Response {
        self.replica.streamed_response(
            response,
            self.session,
            ResponseOf::Call(self.entry),
            Some(self.token),
        )
    }
}

impl McpReplica {
    /// A replica over the shared `store`. A background task sweeps its memory until the replica
    /// is dropped.
    pub fn new(store: Arc<dyn McpSessionStore>, config: &McpConfig) -> Arc<Self> {
        let events = Arc::new(ReplicaEventStore::new());
        let streams = Arc::new(GetStreams::new());
        let sweep = CancellationToken::new();
        tokio::spawn(sweep_until_cancelled(
            events.clone(),
            streams.clone(),
            sweep.clone(),
        ));
        Arc::new(Self {
            store,
            idle: config.session.session_idle_timeout,
            keep_alive: config.sse_keep_alive_interval,
            events,
            streams,
            calls: Arc::new(RunningCalls::new()),
            _sweep: sweep.drop_guard(),
            #[cfg(test)]
            hooks: hooks::TestHooks::default(),
        })
    }

    /// The store of this replica's response events, for rmcp.
    pub fn event_store(&self) -> Arc<dyn EventStore> {
        self.events.clone()
    }

    /// Opens a session for a client whose `initialize` rmcp answered with `response`: creates the
    /// record with the served deployment, names the session in the response, and makes the
    /// response stream resumable. `None` when no session could be created.
    pub(super) async fn open_session(
        self: &Arc<Self>,
        mut response: Response,
        deployment: &McpDeployment,
    ) -> Option<Response> {
        let session = self.create_session(&deployment.compiled).await?;
        let header = HeaderValue::try_from(session.to_string()).ok()?;
        response.headers_mut().insert(HEADER_SESSION_ID, header);
        Some(
            self.clone()
                .streamed_response(response, session, ResponseOf::Initialize, None),
        )
    }

    /// Creates the record of a new session. A taken ID is retried once with a new ID.
    async fn create_session(&self, deployment: &CompiledMcp) -> Option<McpSessionId> {
        let record = McpSessionRecord {
            domain: deployment.domain.clone(),
            environment_id: deployment.environment_id,
            deployment_revision: deployment.deployment_revision,
        };
        let created = match self.try_create(&record).await {
            Ok(None) => self.try_create(&record).await,
            other => other,
        };
        created
            .inspect_err(|error| warn!(error = %error, "Failed to create an MCP session"))
            .ok()
            .flatten()
    }

    async fn try_create(
        &self,
        record: &McpSessionRecord,
    ) -> Result<Option<McpSessionId>, McpSessionStoreError> {
        let id = McpSessionId::generate().map_err(anyhow::Error::from)?;
        let created = self.store.create(id, record, self.idle).await?;
        Ok(created.then_some(id))
    }

    /// Reads the session's record for a request served with `served`, keeping the session live
    /// for the idle timeout.
    pub(super) async fn admit(&self, session: McpSessionId, served: &CompiledMcp) -> Admission {
        let read = self.store.get(session, Some(self.idle)).await;
        if let Err(error) = &read {
            warn!(error = %error, "Failed to read an MCP session");
        }
        admit(&read, served)
    }

    /// Starts a call of an admitted session whose pin is `pin`: the serving to put into the
    /// request, and the call whose response the request gets.
    pub(super) fn start_call(
        self: &Arc<Self>,
        session: McpSessionId,
        request_id: RequestId,
        pin: DeploymentRevision,
        deployment: Arc<McpDeployment>,
    ) -> (Arc<dyn McpServing>, StartedCall) {
        let (token, call) = self.calls.add(&self.events, session, request_id);
        let started = StartedCall {
            replica: self.clone(),
            session,
            entry: call.entry(),
            token: token.clone(),
        };
        let serving = Arc::new(SessionServing {
            replica: self.clone(),
            session,
            pin,
            deployment,
            token,
            _call: call,
        });
        (serving, started)
    }

    /// Moves the session's pin to `pin`, keeping the record's expiry.
    async fn write_pin(
        &self,
        session: McpSessionId,
        served: &CompiledMcp,
        pin: DeploymentRevision,
    ) -> Result<(), ServeRefusal> {
        let record = McpSessionRecord {
            domain: served.domain.clone(),
            environment_id: served.environment_id,
            deployment_revision: pin,
        };
        let replaced = self.store.replace(session, &record).await;
        if let Err(error) = &replaced {
            warn!(error = %error, "Failed to write the pin of an MCP session");
        }
        pin_written(&replaced)
    }

    /// Keeps the session live while one of its calls runs. The returned future never completes;
    /// it stops when it is dropped with the call.
    fn heartbeat(&self, session: McpSessionId) -> impl Future<Output = ()> + Send + 'static {
        heartbeat(self.store.clone(), session, self.idle)
    }

    /// Tells this replica's GET streams of the session that its tool list changed.
    fn notify_tool_list_changed(&self, session: McpSessionId) {
        self.streams.notify(
            session,
            ServerJsonRpcMessage::notification(ServerNotification::ToolListChangedNotification(
                ToolListChangedNotification {
                    method: Default::default(),
                    extensions: Default::default(),
                },
            )),
        );
    }

    /// Cancels the session's running calls with this request ID.
    pub(super) async fn cancel_request(&self, session: McpSessionId, request_id: &RequestId) {
        self.cancel(self.calls.cancel(session, request_id)).await;
    }

    /// Ends the session: deletes its record, cancels its running calls on this replica and
    /// closes its GET streams here.
    pub(super) async fn close_session(
        &self,
        session: McpSessionId,
    ) -> Result<(), McpSessionStoreError> {
        self.store.delete(session).await?;
        self.cancel(self.calls.cancel_session(session)).await;
        self.streams.close_session(session);
        Ok(())
    }

    /// Marks the response stream of every cancelled call first, then cancels every call's token:
    /// the event store then already refuses any event that a call produces because of its
    /// cancel.
    async fn cancel(&self, cancelled: Vec<CancelledCall>) {
        cancelled
            .iter()
            .filter_map(|call| call.stream.as_deref())
            .for_each(|stream| self.events.mark_cancelled(stream));
        #[cfg(test)]
        self.hooks.reached(HookPoint::BetweenMarkAndCancel).await;
        cancelled.iter().for_each(|call| call.token.cancel());
    }

    /// Notes the call's response stream. The event store is bound first, so the stream counts as
    /// running exactly while the call's entry exists, whenever the call ends around these steps.
    pub(super) async fn bind_call(&self, session: McpSessionId, entry: EntryId, stream: &str) {
        self.events.bind_call(stream, session);
        #[cfg(test)]
        self.hooks.reached(HookPoint::BetweenBindSteps).await;
        match self.calls.bind(session, entry, stream) {
            BindOutcome::Bound => {}
            BindOutcome::Gone => self.events.call_ended(stream),
            BindOutcome::CancelledBefore => self.events.mark_cancelled(stream),
        }
    }

    async fn bind_stream(&self, session: McpSessionId, of: ResponseOf, stream: &str) {
        match of {
            ResponseOf::Initialize => self.events.bind_response(stream, session),
            ResponseOf::Call(entry) => self.bind_call(session, entry, stream).await,
        }
    }

    /// Binds a streamed response body when its first event passes. With a token, the body ends
    /// as soon as the token is cancelled.
    fn streamed_response(
        self: Arc<Self>,
        mut response: Response,
        session: McpSessionId,
        of: ResponseOf,
        token: Option<CancellationToken>,
    ) -> Response {
        let streamed = response
            .headers()
            .get(poem::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with(EVENT_STREAM_MIME_TYPE));
        if !streamed {
            return response;
        }
        let inner = response.take_body().into_bytes_stream();
        let body = stream::unfold(
            (inner, token, Some(self)),
            move |(mut inner, token, binder)| async move {
                let chunk = match &token {
                    Some(token) => tokio::select! {
                        biased;
                        _ = token.cancelled() => None,
                        chunk = inner.next() => chunk,
                    },
                    None => inner.next().await,
                }?;
                if let (Some(replica), Ok(frame)) = (&binder, &chunk)
                    && let Some((stream, _)) = priming_event_id(frame).and_then(parse_event_id)
                {
                    replica.bind_stream(session, of, stream).await;
                }
                Some((chunk, (inner, token, None)))
            },
        );
        response.set_body(Body::from_bytes_stream(body));
        response
    }

    /// A GET stream of the session: it announces the reconnect delay, then carries the session's
    /// notifications and keep-alive comments until it is closed or the session's record is gone.
    pub(super) fn notification_stream(&self, session: McpSessionId) -> Response {
        let receiver = self.streams.admit(session);
        let gone = record_gone(self.store.clone(), session, self.idle);
        let events = stream::once(ready(Event::retry(SSE_RETRY_MILLIS)))
            .chain(ReceiverStream::new(receiver).filter_map(|message| {
                ready(serde_json::to_string(&message).ok().map(Event::message))
            }))
            .take_until(gone);
        SSE::new(events).keep_alive(self.keep_alive).into_response()
    }

    /// The rest of the response stream that `last_event_id` belongs to, when that stream is bound
    /// to `session`; otherwise an empty stream.
    pub(super) fn resume(&self, last_event_id: &str, session: McpSessionId) -> Response {
        match self.events.replay(last_event_id, session) {
            Some(events) => SSE::new(events.map(|(id, event)| sse_event(id, event)))
                .keep_alive(self.keep_alive)
                .into_response(),
            None => SSE::new(stream::empty()).into_response(),
        }
    }
}

/// Sweeps one shard of the event store and of the GET-stream registry per tick, until `stop` is
/// cancelled.
async fn sweep_until_cancelled(
    events: Arc<ReplicaEventStore>,
    streams: Arc<GetStreams>,
    stop: CancellationToken,
) {
    IntervalStream::new(tokio::time::interval(SWEEP_TICK))
        .take_until(stop.cancelled_owned())
        .enumerate()
        .for_each(|(tick, _)| {
            let shard = sweep_step(tick);
            events.sweep(shard, Instant::now());
            streams.sweep(shard);
            ready(())
        })
        .await
}

fn sse_event(id: String, event: ServerSseMessage) -> Event {
    let data = event
        .message
        .as_deref()
        .and_then(|message| serde_json::to_string(message).ok())
        .unwrap_or_default();
    Event::message(data).id(id)
}

/// Completes when a read of the session's record, once per idle timeout and without moving its
/// expiry, finds no record. A failed read does not end the stream.
async fn record_gone(store: Arc<dyn McpSessionStore>, session: McpSessionId, idle: Duration) {
    let reads = IntervalStream::new(tokio::time::interval_at(Instant::now() + idle, idle));
    reads
        .then(move |_| {
            let store = store.clone();
            async move { store.get(session, None).await }
        })
        .any(|read| ready(matches!(read, Ok(None))))
        .await;
}

/// Keeps the session live while a call runs: moves its expiry forward at once, then once per
/// heartbeat period. It never completes; it stops when it is dropped with the call.
async fn heartbeat(store: Arc<dyn McpSessionStore>, session: McpSessionId, idle: Duration) {
    let mut ticks = tokio::time::interval(heartbeat_period(idle));
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    IntervalStream::new(ticks)
        .for_each(|_| {
            let store = store.clone();
            async move {
                if let Err(error) = store.get(session, Some(idle)).await {
                    warn!(error = %error, "Failed to extend an MCP session with a running call");
                }
            }
        })
        .await
}

/// Cancels `ct` when the call is cancelled through the session.
async fn cancel_with(token: CancellationToken, ct: CancellationToken) {
    token.cancelled().await;
    ct.cancel();
}

/// Serves the requests of an admitted session with the request's deployment, after the pin
/// rule: lists move the pin to the served revision, and a call is served only with the
/// revision its client listed. When the call is cancelled through the session, the handler's
/// cancellation token is cancelled too.
struct SessionServing {
    replica: Arc<McpReplica>,
    session: McpSessionId,
    /// The revision the session's client last listed, as admitted.
    pin: DeploymentRevision,
    deployment: Arc<McpDeployment>,
    token: CancellationToken,
    /// Kept for its `Drop`: the call leaves the running-call table when the request ends.
    _call: RunningCallGuard,
}

#[async_trait]
impl McpServing for SessionServing {
    async fn serve(
        &self,
        kind: ServeKind,
        ct: &CancellationToken,
    ) -> Result<Serving, ServeRefusal> {
        let served = &self.deployment.compiled;
        match pin_step(kind, self.pin, served.deployment_revision) {
            PinStep::Serve => {}
            PinStep::Write(pin) => self.replica.write_pin(self.session, served, pin).await?,
            PinStep::Changed => {
                self.replica.notify_tool_list_changed(self.session);
                return Err(ServeRefusal::ToolSetChanged);
            }
        }
        let cancel = cancel_with(self.token.clone(), ct.clone());
        let while_running = match kind {
            ServeKind::Invoke => {
                let heartbeat = self.replica.heartbeat(self.session);
                async move {
                    futures::future::join(heartbeat, cancel).await;
                    std::future::pending::<Infallible>().await
                }
                .boxed()
            }
            ServeKind::Initialize | ServeKind::List => async move {
                cancel.await;
                std::future::pending::<Infallible>().await
            }
            .boxed(),
        };
        Ok(Serving {
            deployment: self.deployment.clone(),
            while_running,
        })
    }
}

/// Places in the replica's work where a test can hold it.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(super) enum HookPoint {
    BetweenBindSteps,
    BetweenMarkAndCancel,
}

#[cfg(test)]
impl McpReplica {
    pub(super) fn hooks(&self) -> &hooks::TestHooks {
        &self.hooks
    }

    pub(super) fn calls(&self) -> &Arc<RunningCalls> {
        &self.calls
    }

    pub(super) fn events(&self) -> &Arc<ReplicaEventStore> {
        &self.events
    }

    pub(super) fn streams(&self) -> &GetStreams {
        &self.streams
    }
}

#[cfg(test)]
pub(super) mod hooks {
    use super::HookPoint;
    use futures::future::BoxFuture;
    use std::sync::{Arc, OnceLock};

    pub(in crate::mcp::session) type Hook = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

    /// Work a test runs when the replica reaches a [`HookPoint`], for example waiting on a gate.
    #[derive(Default)]
    pub(in crate::mcp::session) struct TestHooks {
        between_bind_steps: OnceLock<Hook>,
        between_mark_and_cancel: OnceLock<Hook>,
    }

    impl TestHooks {
        fn slot(&self, point: HookPoint) -> &OnceLock<Hook> {
            match point {
                HookPoint::BetweenBindSteps => &self.between_bind_steps,
                HookPoint::BetweenMarkAndCancel => &self.between_mark_and_cancel,
            }
        }

        pub(in crate::mcp::session) async fn reached(&self, point: HookPoint) {
            if let Some(hook) = self.slot(point).get() {
                hook().await;
            }
        }

        pub(in crate::mcp::session) fn set(&self, point: HookPoint, hook: Hook) {
            assert!(self.slot(point).set(hook).is_ok(), "hook already set");
        }
    }
}
