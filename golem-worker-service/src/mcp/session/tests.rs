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

//! The session layer in front of rmcp, over a SQLite session store and a stub MCP handler whose
//! calls wait on gates. Two endpoints with their own replicas over one store stand in for two
//! worker-service replicas.

use super::McpReplica;
use super::event_store::ReplicaEventStore;
use super::replica::HookPoint;
use super::sharded::lock_check;
use super::store::{
    McpSessionId, McpSessionRecord, McpSessionStore, McpSessionStoreError, SqliteMcpSessionStore,
};
use crate::config::McpConfig;
use crate::custom_api::oidc::session_store::migrated_sqlite_pool;
use crate::mcp::InvocationHarness;
use crate::mcp::serving::{McpServing, ServeKind, ServeRefusal, StatelessServing};
use crate::mcp::{
    GolemAgentMcpServer, McpCapabilitiesLookupError, McpCapabilityLookup, McpDeployment,
    mcp_transport,
};
use crate::service::worker::WorkerService;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt, stream};
use golem_common::config::DbSqliteConfig;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::application::ApplicationName;
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_service_base::mcp::CompiledMcp;
use poem::endpoint::{BoxEndpoint, TowerCompatExt};
use poem::http::{Method, StatusCode};
use poem::{Endpoint, EndpointExt, Request, Response};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::ServerSseMessage;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::session::store::{
    EventId, EventStore, EventStoreError, EventStream,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::NamedTempFile;
use test_r::test;
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout};

const WAIT: Duration = Duration::from_secs(5);
use super::test_ids::SESSION as ANY_SESSION;

// ---------------------------------------------------------------------------------------------
// The stub handler

/// A named gate of the stub: the stub's call waits until the test opens it, and tells the test
/// when it started and how it ended.
struct Gate {
    open: Semaphore,
    started: Semaphore,
    returned: Semaphore,
    outcome: Mutex<Option<&'static str>>,
}

impl Gate {
    fn new() -> Self {
        Self {
            open: Semaphore::new(0),
            started: Semaphore::new(0),
            returned: Semaphore::new(0),
            outcome: Mutex::new(None),
        }
    }

    fn open(&self) {
        self.open.add_permits(1);
    }

    async fn started(&self) {
        timeout(WAIT, self.started.acquire())
            .await
            .expect("the call starts")
            .unwrap()
            .forget();
    }

    async fn returned(&self) -> &'static str {
        timeout(WAIT, self.returned.acquire())
            .await
            .expect("the handler returns")
            .unwrap()
            .forget();
        self.outcome.lock().unwrap().unwrap()
    }

    fn finish(&self, outcome: &'static str) {
        *self.outcome.lock().unwrap() = Some(outcome);
        self.returned.add_permits(1);
    }
}

struct Probe {
    /// The serving that the test puts into every request, as the authentication layer does.
    stateless: Arc<dyn McpServing>,
    /// For each handled request: its method, and whether it saw `stateless`.
    seen: Mutex<Vec<(&'static str, bool)>>,
    gates: Mutex<HashMap<Box<str>, Arc<Gate>>>,
    list_gated: AtomicBool,
    /// For each `tools/call`: whether its serving served it, or why not.
    invoked: Mutex<Vec<Result<(), ServeRefusal>>>,
}

impl Probe {
    fn gate(&self, name: &str) -> Arc<Gate> {
        self.gates
            .lock()
            .unwrap()
            .entry(Box::from(name))
            .or_insert_with(|| Arc::new(Gate::new()))
            .clone()
    }

    fn saw(
        &self,
        method: &'static str,
        context: &RequestContext<RoleServer>,
    ) -> Arc<dyn McpServing> {
        let serving = serving_of(context);
        self.seen
            .lock()
            .unwrap()
            .push((method, Arc::ptr_eq(&serving, &self.stateless)));
        serving
    }
}

fn serving_of(context: &RequestContext<RoleServer>) -> Arc<dyn McpServing> {
    context
        .extensions
        .get::<poem::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Arc<dyn McpServing>>())
        .cloned()
        .expect("the request has a serving")
}

#[derive(Clone)]
struct Stub(Arc<Probe>);

fn text(text: &str) -> CallToolResponse {
    CallToolResult::success(vec![ContentBlock::text(text)]).into()
}

impl ServerHandler for Stub {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_03_26)
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.0.saw("initialize", &context);
        Ok(self.get_info())
    }

    async fn discover(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<DiscoverResult, ErrorData> {
        self.0.saw("server/discover", &context);
        Ok(DiscoverResult::from_server_info(
            self.supported_protocol_versions().into_owned(),
            self.get_info(),
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.0.saw("tools/list", &context);
        if self.0.list_gated.load(Ordering::SeqCst) {
            let gate = self.0.gate("list");
            gate.started.add_permits(1);
            gate.open.acquire().await.unwrap().forget();
            gate.finish("opened");
        }
        Ok(ListToolsResult::with_all_items(vec![]))
    }

    /// `echo` answers at once. `gated` waits for its gate, as an agent method does, whatever
    /// happens to the request's token; `panic` panics when its gate opens. `native` waits for its gate or the request's token, as a
    /// native tool does, and answers "tool invocation cancelled" on the token. A gated call
    /// releases its request before it tells the test that it returned, so its running-call entry
    /// is gone by then.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let request_serving = self.0.saw("tools/call", &context);
        let served = request_serving.serve(ServeKind::Invoke, &context.ct).await;
        self.0
            .invoked
            .lock()
            .unwrap()
            .push(served.as_ref().map(|_| ()).map_err(|refusal| *refusal));
        let serving = served
            .map_err(|_| ErrorData::invalid_params("tool set changed; list tools again", None))?;
        let gate_name = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("gate"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let gate = self.0.gate(&gate_name);
        let ct = context.ct.clone();
        let work = async {
            match request.name.as_ref() {
                "echo" => (Ok(text("echo")), None),
                "gated" => {
                    gate.started.add_permits(1);
                    gate.open.acquire().await.unwrap().forget();
                    (Ok(text("opened")), Some("opened"))
                }
                "panic" => {
                    gate.started.add_permits(1);
                    gate.open.acquire().await.unwrap().forget();
                    panic!("the stub call panics")
                }
                "native" => {
                    gate.started.add_permits(1);
                    tokio::select! {
                        _ = gate.open.acquire() => (Ok(text("opened")), Some("opened")),
                        _ = ct.cancelled() => (
                            Err(ErrorData::internal_error("tool invocation cancelled", None)),
                            Some("cancelled"),
                        ),
                    }
                }
                other => (
                    Err(ErrorData::invalid_params(format!("no tool {other}"), None)),
                    None,
                ),
            }
        };
        let (result, outcome) = tokio::select! {
            done = work => done,
            never = serving.while_running => match never {},
        };
        drop(request_serving);
        drop(context);
        if let Some(outcome) = outcome {
            gate.finish(outcome);
        }
        result
    }
}

// ---------------------------------------------------------------------------------------------
// The recording store

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Create,
    Get { session: McpSessionId, extend: bool },
    Replace,
    Delete(McpSessionId),
}

/// The store under test, recording each operation. It can fail every operation, fail only
/// deletes, or answer `create` as if the ID were taken.
struct RecordingStore {
    inner: Arc<dyn McpSessionStore>,
    ops: Mutex<Vec<Op>>,
    failing: AtomicBool,
    failing_deletes: AtomicBool,
    taken_ids: AtomicUsize,
    /// When set, `replace` waits for this gate.
    replace_gate: Mutex<Option<Arc<Gate>>>,
}

impl RecordingStore {
    fn record(&self, op: Op) -> Result<(), McpSessionStoreError> {
        self.ops.lock().unwrap().push(op);
        if self.failing.load(Ordering::SeqCst) {
            Err(anyhow::anyhow!("store unreachable").into())
        } else {
            Ok(())
        }
    }

    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
}

#[async_trait]
impl McpSessionStore for RecordingStore {
    async fn create(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
        ttl: Duration,
    ) -> Result<bool, McpSessionStoreError> {
        self.record(Op::Create)?;
        if self
            .taken_ids
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |taken| {
                taken.checked_sub(1)
            })
            .is_ok()
        {
            return Ok(false);
        }
        self.inner.create(id, record, ttl).await
    }

    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, McpSessionStoreError> {
        self.record(Op::Get {
            session: id,
            extend: extend.is_some(),
        })?;
        self.inner.get(id, extend).await
    }

    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, McpSessionStoreError> {
        self.record(Op::Replace)?;
        let gate = self.replace_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.started.add_permits(1);
            gate.open.acquire().await.unwrap().forget();
            gate.finish("opened");
        }
        self.inner.replace(id, record).await
    }

    async fn delete(&self, id: McpSessionId) -> Result<(), McpSessionStoreError> {
        self.record(Op::Delete(id))?;
        if self.failing_deletes.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("delete failed").into());
        }
        self.inner.delete(id).await
    }
}

/// Session records in memory, expiring by tokio's clock, so that expiry tests run on paused
/// time. A read that extends keeps the record live for at least the idle timeout from now, as
/// every store does.
#[derive(Default)]
struct ClockStore {
    records: Mutex<HashMap<McpSessionId, (McpSessionRecord, Instant)>>,
}

impl ClockStore {
    fn live(&self, id: McpSessionId, now: Instant) -> Option<McpSessionRecord> {
        self.records
            .lock()
            .unwrap()
            .get(&id)
            .filter(|(_, expires_at)| *expires_at > now)
            .map(|(record, _)| record.clone())
    }
}

#[async_trait]
impl McpSessionStore for ClockStore {
    async fn create(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
        ttl: Duration,
    ) -> Result<bool, McpSessionStoreError> {
        let now = Instant::now();
        let taken = self.live(id, now).is_some();
        if !taken {
            self.records
                .lock()
                .unwrap()
                .insert(id, (record.clone(), now + ttl));
        }
        Ok(!taken)
    }

    async fn get(
        &self,
        id: McpSessionId,
        extend: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, McpSessionStoreError> {
        let now = Instant::now();
        let live = self.live(id, now);
        if let (Some(_), Some(idle)) = (&live, extend)
            && let Some((_, expires_at)) = self.records.lock().unwrap().get_mut(&id)
        {
            *expires_at = (*expires_at).max(now + idle);
        }
        Ok(live)
    }

    async fn replace(
        &self,
        id: McpSessionId,
        record: &McpSessionRecord,
    ) -> Result<bool, McpSessionStoreError> {
        let live = self.live(id, Instant::now()).is_some();
        if live && let Some((stored, _)) = self.records.lock().unwrap().get_mut(&id) {
            *stored = record.clone();
        }
        Ok(live)
    }

    async fn delete(&self, id: McpSessionId) -> Result<(), McpSessionStoreError> {
        self.records.lock().unwrap().remove(&id);
        Ok(())
    }
}

/// Runs `test` on a runtime whose clock is paused; it moves only by `tokio::time::advance`
/// and when every task waits for a timer. The runtime runs on its own thread, and the test
/// fails when it has not finished within 30 seconds of real time.
fn on_paused_time(test: impl Future<Output = ()> + Send + 'static) {
    let (finished, done) = std::sync::mpsc::channel();
    let runner = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(test);
        let _ = finished.send(());
    });
    match done.recv_timeout(Duration::from_secs(30)) {
        Ok(()) => runner.join().unwrap(),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(runner.join().unwrap_err())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("the paused-time test did not finish within 30 seconds")
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The fixture

fn compiled(domain: &str) -> CompiledMcp {
    CompiledMcp {
        account_id: AccountId::new(),
        account_email: AccountEmail::new("owner@example.com"),
        application_name: ApplicationName("app".into()),
        environment_id: EnvironmentId::new(),
        environment_name: EnvironmentName("env".into()),
        deployment_revision: DeploymentRevision::INITIAL,
        domain: Domain(domain.into()),
        security_scheme_name: None,
        security_scheme: None,
        registered_agent_types: vec![],
        tools: vec![],
    }
}

fn deployment(domain: &str) -> Arc<McpDeployment> {
    Arc::new(McpDeployment::new(compiled(domain)))
}

/// The same deployment at another revision, as a deploy or a rollback makes it.
fn at_revision(deployment: &McpDeployment, revision: DeploymentRevision) -> Arc<McpDeployment> {
    Arc::new(McpDeployment::new(CompiledMcp {
        deployment_revision: revision,
        ..deployment.compiled.clone()
    }))
}

fn config(idle: Duration, keep_alive: Duration) -> McpConfig {
    let mut config = McpConfig {
        sse_keep_alive_interval: keep_alive,
        ..McpConfig::default()
    };
    config.session.session_idle_timeout = idle;
    config
}

/// The MCP handler behind the session layer: the stub, or Golem's handler.
#[derive(Clone)]
enum Handler {
    Stub(Arc<Probe>),
    Golem {
        lookup: Arc<dyn McpCapabilityLookup>,
        worker_service: Arc<WorkerService>,
    },
}

/// The replica's event store as rmcp sees it, counting each attempt to store a response or
/// error. rmcp stores the response of a call after the handler returns, so a test that checks
/// what a resume replays waits for the attempt first.
struct ObservedEvents {
    inner: Arc<dyn EventStore>,
    terminals: tokio::sync::watch::Sender<usize>,
}

impl ObservedEvents {
    async fn terminals_attempted(&self, count: usize) {
        let mut attempted = self.terminals.subscribe();
        timeout(WAIT, attempted.wait_for(|attempted| *attempted >= count))
            .await
            .expect("the terminals are stored or refused")
            .unwrap();
    }
}

#[async_trait]
impl EventStore for ObservedEvents {
    async fn store_event(
        &self,
        stream_id: &str,
        event: &ServerSseMessage,
    ) -> Result<EventId, EventStoreError> {
        let stored = self.inner.store_event(stream_id, event).await;
        if matches!(
            event.message.as_deref(),
            Some(ServerJsonRpcMessage::Response(_) | ServerJsonRpcMessage::Error(_))
        ) {
            self.terminals.send_modify(|attempted| *attempted += 1);
        }
        stored
    }

    async fn replay_events_after(
        &self,
        last_event_id: &str,
    ) -> Result<EventStream, EventStoreError> {
        self.inner.replay_events_after(last_event_id).await
    }
}

/// The endpoint of `replica` over `handler`, as `mcp_transport` builds it. With `observe`, rmcp
/// stores events through an [`ObservedEvents`].
fn transport<S, H>(
    handler: H,
    replica: &Arc<McpReplica>,
    config: &McpConfig,
    observe: bool,
) -> (BoxEndpoint<'static>, Option<Arc<ObservedEvents>>)
where
    S: ServerHandler + Send + 'static,
    H: Fn() -> S + Send + Sync + 'static,
{
    let (events, observed): (Arc<dyn EventStore>, _) = if observe {
        let observed = Arc::new(ObservedEvents {
            inner: replica.event_store(),
            terminals: tokio::sync::watch::Sender::new(0),
        });
        (observed.clone(), Some(observed))
    } else {
        (replica.event_store(), None)
    };
    let endpoint = mcp_transport(handler, replica.clone(), events, config)
        .map_to_response()
        .boxed();
    (endpoint, observed)
}

struct Fixture {
    endpoint: BoxEndpoint<'static>,
    replica: Arc<McpReplica>,
    store: Arc<RecordingStore>,
    probe: Arc<Probe>,
    handler: Handler,
    deployment: Arc<McpDeployment>,
    config: McpConfig,
    observed: Option<Arc<ObservedEvents>>,
    _db: Option<NamedTempFile>,
}

async fn sqlite_store() -> (Arc<dyn McpSessionStore>, NamedTempFile) {
    let db = NamedTempFile::new().unwrap();
    let pool = migrated_sqlite_pool(&DbSqliteConfig {
        database: db.path().to_string_lossy().into_owned(),
        max_connections: 2,
        foreign_keys: false,
    })
    .await
    .unwrap();
    (
        Arc::new(SqliteMcpSessionStore::new(pool, Duration::from_secs(3600))),
        db,
    )
}

fn default_config() -> McpConfig {
    config(Duration::from_secs(300), Duration::from_secs(15))
}

impl Fixture {
    async fn new() -> Self {
        Self::with_config(default_config()).await
    }

    async fn with_config(config: McpConfig) -> Self {
        let (store, db) = sqlite_store().await;
        Self::with_store(config, store, Some(db))
    }

    /// A fixture over `inner`. `db` is the file behind `inner`, kept for the fixture's life.
    fn with_store(
        config: McpConfig,
        inner: Arc<dyn McpSessionStore>,
        db: Option<NamedTempFile>,
    ) -> Self {
        Self::build(
            config,
            inner,
            db,
            None,
            deployment("tools.example.com"),
            false,
        )
    }

    /// The stub behind a layer whose event store is observed.
    async fn observed() -> Self {
        let (store, db) = sqlite_store().await;
        Self::build(
            default_config(),
            store,
            Some(db),
            None,
            deployment("tools.example.com"),
            true,
        )
    }

    /// Golem's handler over the harness's worker service, serving `deployment` by default.
    async fn golem(
        harness: &InvocationHarness,
        lookup: Arc<dyn McpCapabilityLookup>,
        deployment: Arc<McpDeployment>,
        observe: bool,
    ) -> Self {
        let (store, db) = sqlite_store().await;
        let handler = Handler::Golem {
            lookup,
            worker_service: harness.worker_service.clone(),
        };
        Self::build(
            default_config(),
            store,
            Some(db),
            Some(handler),
            deployment,
            observe,
        )
    }

    fn build(
        config: McpConfig,
        inner: Arc<dyn McpSessionStore>,
        db: Option<NamedTempFile>,
        handler: Option<Handler>,
        deployment: Arc<McpDeployment>,
        observe: bool,
    ) -> Self {
        let store = Arc::new(RecordingStore {
            inner,
            ops: Mutex::new(Vec::new()),
            failing: AtomicBool::new(false),
            failing_deletes: AtomicBool::new(false),
            taken_ids: AtomicUsize::new(0),
            replace_gate: Mutex::new(None),
        });
        let probe = Arc::new(Probe {
            stateless: Arc::new(StatelessServing(deployment.clone())),
            seen: Mutex::new(Vec::new()),
            gates: Mutex::new(HashMap::new()),
            list_gated: AtomicBool::new(false),
            invoked: Mutex::new(Vec::new()),
        });
        let handler = handler.unwrap_or_else(|| Handler::Stub(probe.clone()));
        let (endpoint, replica, observed) = Self::endpoint(&store, &handler, &config, observe);
        Self {
            endpoint,
            replica,
            store,
            probe,
            handler,
            deployment,
            config,
            observed,
            _db: db,
        }
    }

    fn endpoint(
        store: &Arc<RecordingStore>,
        handler: &Handler,
        config: &McpConfig,
        observe: bool,
    ) -> (
        BoxEndpoint<'static>,
        Arc<McpReplica>,
        Option<Arc<ObservedEvents>>,
    ) {
        let replica = McpReplica::new(store.clone(), config);
        let (endpoint, observed) = match handler.clone() {
            Handler::Stub(probe) => {
                transport(move || Stub(probe.clone()), &replica, config, observe)
            }
            Handler::Golem {
                lookup,
                worker_service,
            } => transport(
                move || GolemAgentMcpServer::new(lookup.clone(), worker_service.clone()),
                &replica,
                config,
                observe,
            ),
        };
        (endpoint, replica, observed)
    }

    /// Another replica over the same store and handler.
    fn second_replica(&self) -> (BoxEndpoint<'static>, Arc<McpReplica>) {
        let (endpoint, replica, _) =
            Self::endpoint(&self.store, &self.handler, &self.config, false);
        (endpoint, replica)
    }

    fn observed_events(&self) -> &ObservedEvents {
        self.observed.as_deref().expect("an observed fixture")
    }

    /// Adds what the authentication layer adds: the request's deployment and its serving.
    fn authenticated(&self, mut request: Request, deployment: &Arc<McpDeployment>) -> Request {
        request
            .headers_mut()
            .insert("host", deployment.compiled.domain.0.parse().unwrap());
        let serving: Arc<dyn McpServing> = if Arc::ptr_eq(deployment, &self.deployment) {
            self.probe.stateless.clone()
        } else {
            Arc::new(StatelessServing(deployment.clone()))
        };
        request.extensions_mut().insert(deployment.clone());
        request.extensions_mut().insert(serving);
        request
    }

    fn post(&self, session: Option<&str>, body: Value) -> Request {
        self.post_bytes(session, serde_json::to_vec(&body).unwrap())
    }

    /// A POST served with `deployment` instead of the fixture's.
    fn post_with(&self, session: &str, body: Value, deployment: &Arc<McpDeployment>) -> Request {
        self.authenticated(self.post(Some(session), body), deployment)
    }

    fn post_bytes(&self, session: Option<&str>, body: Vec<u8>) -> Request {
        let builder = Request::builder()
            .method(Method::POST)
            .uri_str("/mcp")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        let builder = match session {
            Some(session) => builder.header("mcp-session-id", session),
            None => builder,
        };
        self.authenticated(builder.body(body), &self.deployment)
    }

    fn get(&self, session: Option<&str>, last_event_id: Option<&str>) -> Request {
        let builder = Request::builder()
            .method(Method::GET)
            .uri_str("/mcp")
            .header("accept", "text/event-stream");
        let builder = match session {
            Some(session) => builder.header("mcp-session-id", session),
            None => builder,
        };
        let builder = match last_event_id {
            Some(id) => builder.header("last-event-id", id),
            None => builder,
        };
        self.authenticated(builder.finish(), &self.deployment)
    }

    fn delete(&self, session: &str) -> Request {
        self.authenticated(
            Request::builder()
                .method(Method::DELETE)
                .uri_str("/mcp")
                .header("mcp-session-id", session)
                .finish(),
            &self.deployment,
        )
    }

    async fn send(&self, request: Request) -> Response {
        self.endpoint.call(request).await.unwrap()
    }

    async fn initialize(&self) -> String {
        initialize_on(&self.endpoint, self.post(None, initialize())).await
    }

    async fn record(&self, session: &str) -> Option<McpSessionRecord> {
        self.store
            .inner
            .get(session.parse().unwrap(), None)
            .await
            .unwrap()
    }

    /// Ends the test: no shard lock was ever taken while another one was held.
    fn finish(self) {
        lock_check::assert_no_violation();
    }
}

/// Sends an `initialize` to `endpoint` and returns the new session's ID.
async fn initialize_on(endpoint: &BoxEndpoint<'static>, request: Request) -> String {
    let response = endpoint.call(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    read_to_end(response).await;
    session
}

/// A `server/discover` as rmcp's per-request negotiation expects it.
fn discover(fx: &Fixture) -> Request {
    let mut request = fx.post(
        None,
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2025-11-25",
            "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}}}}),
    );
    request
        .headers_mut()
        .insert("mcp-protocol-version", "2025-11-25".parse().unwrap());
    request
        .headers_mut()
        .insert("mcp-method", "server/discover".parse().unwrap());
    request
}

fn initialize() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}})
}

fn call(id: i64, tool: &str, gate: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": tool, "arguments": {"gate": gate}}})
}

fn cancel(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id}})
}

fn tools_list(id: i64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"})
}

// ---------------------------------------------------------------------------------------------
// Reading responses

#[derive(Debug, Default, Clone, PartialEq)]
struct Frame {
    id: Option<String>,
    retry: Option<String>,
    data: Option<String>,
}

impl Frame {
    /// The JSON-RPC message of the frame, when it carries one.
    fn message(&self) -> Option<Value> {
        self.data
            .as_deref()
            .filter(|data| !data.is_empty())
            .map(|data| serde_json::from_str(data).unwrap())
    }
}

fn frames(text: &str) -> Vec<Frame> {
    text.split("\n\n")
        .map(|block| {
            block.lines().fold(Frame::default(), |mut frame, line| {
                let value = |prefix: &str| {
                    line.strip_prefix(prefix)
                        .map(|value| value.strip_prefix(' ').unwrap_or(value).to_string())
                };
                frame.id = frame.id.or_else(|| value("id:"));
                frame.retry = frame.retry.or_else(|| value("retry:"));
                frame.data = frame.data.or_else(|| value("data:"));
                frame
            })
        })
        .filter(|frame| *frame != Frame::default())
        .collect()
}

/// The JSON-RPC messages among `frames`.
fn messages(frames: &[Frame]) -> Vec<Value> {
    frames.iter().filter_map(Frame::message).collect()
}

type BodyStream = BoxStream<'static, Result<Bytes, std::io::Error>>;

fn body_of(response: Response) -> BodyStream {
    response.into_body().into_bytes_stream().boxed()
}

async fn next_frames(body: &mut BodyStream) -> Vec<Frame> {
    let chunk = timeout(WAIT, body.next())
        .await
        .expect("a frame arrives")
        .expect("the body has a frame")
        .unwrap();
    frames(std::str::from_utf8(&chunk).unwrap())
}

/// The rest of the body; `None` when it does not end in time.
async fn rest(body: BodyStream) -> Option<Vec<Frame>> {
    let chunks: Vec<Result<Bytes, std::io::Error>> = timeout(WAIT, body.collect()).await.ok()?;
    let text: String = chunks
        .into_iter()
        .map(|chunk| String::from_utf8(chunk.unwrap().to_vec()).unwrap())
        .collect();
    Some(frames(&text))
}

async fn read_to_end(response: Response) -> Vec<Frame> {
    rest(body_of(response)).await.expect("the body ends")
}

// ---------------------------------------------------------------------------------------------
// Sessions

#[test]
async fn the_initialize_stream_resumes_for_its_session_only() {
    let fx = Fixture::new().await;
    let response = fx.send(fx.post(None, initialize())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let frames = read_to_end(response).await;
    let priming = frames[0].id.clone().expect("the priming event has an ID");

    let resumed =
        messages(&read_to_end(fx.send(fx.get(Some(&session), Some(&priming))).await).await);
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["id"], 1);
    assert_eq!(resumed[0]["result"]["protocolVersion"], "2025-03-26");

    let stream = super::event_store::parse_event_id(&priming)
        .unwrap()
        .0
        .to_string();
    assert_eq!(
        fx.replica.events().binding(&stream),
        Some((Some(session.parse().unwrap()), false)),
        "bound to the new session, with no running call"
    );
    assert_eq!(fx.replica.calls().total(), 0, "no running-call entry");
    let other = fx.initialize().await;
    let elsewhere = read_to_end(fx.send(fx.get(Some(&other), Some(&priming))).await).await;
    assert!(elsewhere.is_empty());
    fx.finish();
}

#[test]
async fn initialize_issues_a_session_with_the_served_deployment() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    assert_eq!(
        fx.record(&session).await,
        Some(McpSessionRecord {
            domain: fx.deployment.compiled.domain.clone(),
            environment_id: fx.deployment.compiled.environment_id,
            deployment_revision: fx.deployment.compiled.deployment_revision,
        })
    );
    let response = fx.send(fx.post(Some(&session), tools_list(2))).await;
    assert_eq!(response.status(), StatusCode::OK);
    fx.finish();
}

#[test]
async fn a_taken_session_id_is_retried_once() {
    let fx = Fixture::new().await;
    fx.store.taken_ids.store(1, Ordering::SeqCst);
    let session = fx.initialize().await;
    assert!(fx.record(&session).await.is_some());
    assert_eq!(fx.store.ops(), [Op::Create, Op::Create]);

    fx.store.taken_ids.store(2, Ordering::SeqCst);
    let response = fx.send(fx.post(None, initialize())).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("mcp-session-id"));
    fx.finish();
}

#[test]
async fn requests_without_a_session_get_400_except_initialize_and_discover() {
    let fx = Fixture::new().await;
    let refused = [
        fx.post(None, tools_list(1)),
        fx.post(None, call(1, "echo", "")),
        fx.post(
            None,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        ),
        fx.get(None, None),
        fx.authenticated(
            Request::builder()
                .method(Method::DELETE)
                .uri_str("/mcp")
                .finish(),
            &fx.deployment,
        ),
    ];
    let statuses: Vec<StatusCode> = stream::iter(refused)
        .then(|request| fx.send(request))
        .map(|response| response.status())
        .collect()
        .await;
    assert!(
        statuses
            .iter()
            .all(|status| *status == StatusCode::BAD_REQUEST),
        "{statuses:?}"
    );
    assert_eq!(fx.send(discover(&fx)).await.status(), StatusCode::OK);
    fx.initialize().await;
    fx.finish();
}

#[test]
async fn unknown_malformed_and_deleted_sessions_get_404() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    assert_eq!(
        fx.send(fx.delete(&session)).await.status(),
        StatusCode::ACCEPTED
    );
    let ops_before = fx.store.ops().len();
    assert_eq!(
        fx.send(fx.post(Some("not-a-session"), tools_list(2)))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fx.store.ops().len(),
        ops_before,
        "a malformed ID makes no store call"
    );
    let statuses: Vec<StatusCode> = stream::iter([ANY_SESSION, session.as_str()])
        .then(|id| fx.send(fx.post(Some(id), tools_list(2))))
        .map(|response| response.status())
        .collect()
        .await;
    assert_eq!(statuses, [StatusCode::NOT_FOUND, StatusCode::NOT_FOUND]);
    fx.initialize().await;
    fx.finish();
}

#[test]
async fn a_session_of_another_domain_or_environment_gets_404() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let other_domain = deployment("other.example.com");
    let other_environment = Arc::new(McpDeployment::new(CompiledMcp {
        environment_id: EnvironmentId::new(),
        ..fx.deployment.compiled.clone()
    }));
    let statuses: Vec<StatusCode> = stream::iter([other_domain, other_environment])
        .then(|served| {
            let request = fx.post(Some(&session), tools_list(2));
            fx.send(fx.authenticated(request, &served))
        })
        .map(|response| response.status())
        .collect()
        .await;
    assert_eq!(statuses, [StatusCode::NOT_FOUND, StatusCode::NOT_FOUND]);
    fx.finish();
}

#[test]
async fn an_unreachable_store_gives_503_and_no_session() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    fx.store.failing.store(true, Ordering::SeqCst);
    let response = fx.send(fx.post(None, initialize())).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(!response.headers().contains_key("mcp-session-id"));
    assert_eq!(
        fx.send(fx.post(Some(&session), tools_list(2)))
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    fx.finish();
}

#[test]
async fn initialize_ignores_any_session_header() {
    let fx = Fixture::new().await;
    let other = fx.initialize().await;
    let record_before = fx.record(&other).await;
    let ops_before = fx.store.ops().len();
    let sessions: Vec<String> = stream::iter(["not-a-session", ANY_SESSION, other.as_str()])
        .then(|header| fx.send(fx.post(Some(header), initialize())))
        .map(|response| {
            assert_eq!(response.status(), StatusCode::OK);
            response.headers()["mcp-session-id"]
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect()
        .await;
    assert!(sessions.iter().all(|session| *session != other));
    assert_eq!(
        fx.store.ops()[ops_before..],
        [Op::Create, Op::Create, Op::Create],
        "no record is read for an initialize"
    );
    assert_eq!(fx.record(&other).await, record_before);
    fx.finish();
}

#[test]
async fn an_unparsable_body_with_an_unknown_session_gets_404() {
    let fx = Fixture::new().await;
    let unknown = fx
        .send(fx.post_bytes(Some(ANY_SESSION), b"{not json".to_vec()))
        .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    let session = fx.initialize().await;
    let live = fx
        .send(fx.post_bytes(Some(&session), b"{not json".to_vec()))
        .await;
    assert!(live.status().is_client_error());
    assert_ne!(live.status(), StatusCode::NOT_FOUND);
    fx.finish();
}

#[test]
async fn a_json_rpc_response_with_a_session_gets_202() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let response = fx
        .send(fx.post(
            Some(&session),
            json!({"jsonrpc": "2.0", "id": 3, "result": {}}),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    fx.finish();
}

#[test]
async fn header_checks_come_before_the_session_read() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let ops_before = fx.store.ops().len();
    let wrong_accept = {
        let mut request = fx.get(Some(&session), None);
        request
            .headers_mut()
            .insert("accept", "application/json".parse().unwrap());
        request
    };
    let unknown_version = |mut request: Request| {
        request
            .headers_mut()
            .insert("mcp-protocol-version", "1999-01-01".parse().unwrap());
        request
    };
    let post_wrong_accept = {
        let mut request = fx.post(Some(&session), tools_list(2));
        request
            .headers_mut()
            .insert("accept", "application/json".parse().unwrap());
        request
    };
    let post_wrong_type = {
        let mut request = fx.post(Some(&session), tools_list(2));
        request
            .headers_mut()
            .insert("content-type", "text/plain".parse().unwrap());
        request
    };
    let statuses: Vec<StatusCode> = stream::iter([
        wrong_accept,
        unknown_version(fx.get(Some(&session), None)),
        unknown_version(fx.delete(&session)),
        post_wrong_accept,
        post_wrong_type,
    ])
    .then(|request| fx.send(request))
    .map(|response| response.status())
    .collect()
    .await;
    assert_eq!(
        statuses,
        [
            StatusCode::NOT_ACCEPTABLE,
            StatusCode::BAD_REQUEST,
            StatusCode::BAD_REQUEST,
            StatusCode::NOT_ACCEPTABLE,
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ]
    );
    assert_eq!(fx.store.ops().len(), ops_before);
    assert!(fx.record(&session).await.is_some());
    fx.finish();
}

#[test]
async fn other_methods_get_405_with_allow() {
    let fx = Fixture::new().await;
    let response = fx
        .send(
            fx.authenticated(
                Request::builder()
                    .method(Method::PUT)
                    .uri_str("/mcp")
                    .finish(),
                &fx.deployment,
            ),
        )
        .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["allow"], "GET, POST, DELETE");
    fx.finish();
}

// ---------------------------------------------------------------------------------------------
// Expiry

#[test]
fn an_idle_session_expires_and_a_used_one_stays() {
    on_paused_time(async {
        let fx = Fixture::with_store(
            config(Duration::from_secs(2), Duration::from_secs(15)),
            Arc::new(ClockStore::default()),
            None,
        );
        let idle = fx.initialize().await;
        let used = fx.initialize().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            fx.send(fx.post(Some(&used), tools_list(2))).await.status(),
            StatusCode::OK
        );
        // Just past the idle timeout of the untouched session.
        tokio::time::advance(Duration::from_millis(1001)).await;
        assert!(fx.record(&idle).await.is_none(), "the idle session ended");
        assert!(fx.record(&used).await.is_some(), "the used session is live");
        fx.finish();
    })
}

#[test]
fn a_request_refused_by_a_header_check_does_not_extend_the_session() {
    on_paused_time(async {
        let fx = Fixture::with_store(
            config(Duration::from_secs(2), Duration::from_secs(15)),
            Arc::new(ClockStore::default()),
            None,
        );
        let session = fx.initialize().await;
        tokio::time::advance(Duration::from_millis(1500)).await;
        let mut refused = fx.post(Some(&session), tools_list(2));
        refused
            .headers_mut()
            .insert("accept", "application/json".parse().unwrap());
        assert_eq!(fx.send(refused).await.status(), StatusCode::NOT_ACCEPTABLE);
        // Just past the idle timeout counted from the session's creation.
        tokio::time::advance(Duration::from_millis(501)).await;
        assert!(fx.record(&session).await.is_none());
        fx.finish();
    })
}

// ---------------------------------------------------------------------------------------------
// Pass-through and serving

#[test]
async fn discover_without_a_session_is_answered_as_without_the_layer() {
    let fx = Fixture::new().await;
    let plain = StreamableHttpService::new(
        {
            let probe = fx.probe.clone();
            move || Ok(Stub(probe.clone()))
        },
        Arc::new(
            NeverSessionManager::default().with_event_store(Arc::new(ReplicaEventStore::new())),
        ),
        crate::mcp::mcp_http_transport_config(&fx.config),
    )
    .compat()
    .map_to_response()
    .boxed();
    let through_layer = fx.send(discover(&fx)).await;
    let without_layer = plain.call(discover(&fx)).await.unwrap();
    assert_eq!(through_layer.status(), StatusCode::OK);
    assert_eq!(through_layer.status(), without_layer.status());
    assert_eq!(
        headers_without_framing(&through_layer),
        headers_without_framing(&without_layer)
    );
    let layered = read_to_end(through_layer).await;
    let plain = read_to_end(without_layer).await;
    assert_eq!(messages(&layered), messages(&plain));
    assert_eq!(messages(&layered).len(), 1);
    assert!(fx.store.ops().is_empty(), "no store call");
    assert_eq!(fx.replica.calls().total(), 0, "no running-call entry");
    let bound = layered
        .iter()
        .filter_map(|frame| frame.id.as_deref())
        .filter_map(super::event_store::parse_event_id)
        .filter_map(|(stream, _)| fx.replica.events().binding(stream))
        .any(|(session, _)| session.is_some());
    assert!(!bound, "the discover stream is bound to no session");
    fx.finish();
}

/// The response headers, without the ones that frame the body.
fn headers_without_framing(response: &Response) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .filter(|(name, _)| {
            ![
                poem::http::header::CONTENT_LENGTH,
                poem::http::header::TRANSFER_ENCODING,
            ]
            .contains(name)
        })
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap().to_string()))
        .collect();
    headers.sort();
    headers
}

#[test]
async fn session_requests_are_served_with_the_session() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    read_to_end(fx.send(discover(&fx)).await).await;
    read_to_end(fx.send(fx.post(Some(&session), tools_list(2))).await).await;
    assert_eq!(
        *fx.probe.seen.lock().unwrap(),
        [
            ("initialize", true),
            ("server/discover", true),
            ("tools/list", false)
        ]
    );

    let mut body = body_of(
        fx.send(fx.post(Some(&session), call(3, "native", "n")))
            .await,
    );
    next_frames(&mut body).await;
    fx.probe.gate("n").started().await;
    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(3))).await.status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(fx.probe.gate("n").returned().await, "cancelled");

    // A call served with another revision than the session's pin is refused by the session's
    // serving; the stateless serving would serve it.
    let next = at_revision(&fx.deployment, DeploymentRevision::INITIAL.next().unwrap());
    read_to_end(
        fx.send(fx.post_with(&session, call(4, "echo", ""), &next))
            .await,
    )
    .await;
    assert_eq!(
        *fx.probe.invoked.lock().unwrap(),
        [Ok(()), Err(ServeRefusal::ToolSetChanged)]
    );
    fx.finish();
}

// ---------------------------------------------------------------------------------------------
// GET streams

#[test]
async fn a_get_stream_starts_with_the_retry_and_no_event_id() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let response = fx.send(fx.get(Some(&session), None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = body_of(response);
    assert_eq!(
        next_frames(&mut body).await,
        [Frame {
            retry: Some("3000".into()),
            ..Frame::default()
        }]
    );
    fx.finish();
}

#[test]
#[test_r::timeout("30s")]
async fn a_get_stream_ends_when_the_record_is_gone() {
    let fx = Fixture::with_config(config(Duration::from_secs(1), Duration::from_secs(15))).await;
    let session = fx.initialize().await;
    let mut body = body_of(fx.send(fx.get(Some(&session), None)).await);
    next_frames(&mut body).await;
    fx.store
        .inner
        .delete(session.parse().unwrap())
        .await
        .unwrap();
    assert!(rest(body).await.is_some(), "the stream ends");
    fx.finish();
}

#[test]
async fn delete_ends_the_session_its_calls_and_its_streams() {
    let fixture = Fixture::new().await;
    let fx = &fixture;
    let session = fx.initialize().await;
    let other = fx.initialize().await;
    let calls: Vec<BodyStream> = stream::iter([(5, "a"), (5, "b"), (6, "c")])
        .then(|(id, gate)| fx.send(fx.post(Some(&session), call(id, "native", gate))))
        .map(body_of)
        .collect()
        .await;
    let calls: Vec<BodyStream> = stream::iter(calls)
        .then(|mut body| async move {
            next_frames(&mut body).await;
            body
        })
        .collect()
        .await;
    stream::iter(["a", "b", "c"])
        .for_each(|gate| async move { fx.probe.gate(gate).started().await })
        .await;
    let mut other_call = body_of(
        fx.send(fx.post(Some(&other), call(5, "native", "other")))
            .await,
    );
    next_frames(&mut other_call).await;
    let mut get_stream = body_of(fx.send(fx.get(Some(&session), None)).await);
    next_frames(&mut get_stream).await;

    assert_eq!(
        fx.send(fx.delete(&session)).await.status(),
        StatusCode::ACCEPTED
    );

    assert!(fx.record(&session).await.is_none());
    let outcomes: Vec<&str> = stream::iter(["a", "b", "c"])
        .then(|gate| async move { fx.probe.gate(gate).returned().await })
        .collect()
        .await;
    assert_eq!(outcomes, ["cancelled", "cancelled", "cancelled"]);
    let ends: Vec<Option<Vec<Frame>>> = stream::iter(calls).then(rest).collect().await;
    assert!(
        ends.iter().all(|end| end
            .as_ref()
            .is_some_and(|frames| messages(frames).is_empty())),
        "every call body ends without a result: {ends:?}"
    );
    assert!(rest(get_stream).await.is_some(), "the GET stream ends");

    fx.probe.gate("other").open();
    assert_eq!(fx.probe.gate("other").returned().await, "opened");
    assert_eq!(messages(&rest(other_call).await.unwrap()).len(), 1);
    fixture.finish();
}

// ---------------------------------------------------------------------------------------------
// Resume

/// Starts a gated call, reads its priming event and cuts the body. Returns the priming ID.
async fn cut_call(fx: &Fixture, session: &str, id: i64, gate: &str) -> String {
    let mut body = body_of(
        fx.send(fx.post(Some(session), call(id, "gated", gate)))
            .await,
    );
    let priming = next_frames(&mut body).await;
    fx.probe.gate(gate).started().await;
    drop(body);
    priming[0].id.clone().expect("the priming event has an ID")
}

#[test]
async fn a_cut_call_stream_resumes_on_its_replica_only() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let priming = cut_call(&fx, &session, 4, "g").await;
    fx.probe.gate("g").open();
    fx.probe.gate("g").returned().await;
    fx.replica.calls().wait_until_empty().await;

    let resumed =
        messages(&read_to_end(fx.send(fx.get(Some(&session), Some(&priming))).await).await);
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["id"], 4);
    assert_eq!(resumed[0]["result"]["content"][0]["text"], "opened");

    let (second, _replica) = fx.second_replica();
    let elsewhere = second
        .call(fx.get(Some(&session), Some(&priming)))
        .await
        .unwrap();
    assert_eq!(elsewhere.status(), StatusCode::OK);
    assert!(messages(&read_to_end(elsewhere).await).is_empty());
    fx.finish();
}

#[test]
async fn a_cut_list_stream_resumes() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    fx.probe.list_gated.store(true, Ordering::SeqCst);
    let mut body = body_of(fx.send(fx.post(Some(&session), tools_list(8))).await);
    let priming = next_frames(&mut body).await[0].id.clone().unwrap();
    fx.probe.gate("list").started().await;
    drop(body);
    fx.probe.gate("list").open();
    fx.probe.gate("list").returned().await;
    fx.replica.calls().wait_until_empty().await;
    let resumed =
        messages(&read_to_end(fx.send(fx.get(Some(&session), Some(&priming))).await).await);
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["id"], 8);
    fx.finish();
}

#[test]
async fn a_stream_of_another_session_is_not_resumed() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let other = fx.initialize().await;
    let priming = cut_call(&fx, &session, 4, "g").await;
    fx.probe.gate("g").open();
    fx.probe.gate("g").returned().await;
    fx.replica.calls().wait_until_empty().await;
    let response = fx.send(fx.get(Some(&other), Some(&priming))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(read_to_end(response).await.is_empty());
    fx.finish();
}

/// Resumes the stream of `priming` `count` times and returns the response bodies, oldest first.
async fn resume_times(fx: &Fixture, session: &str, priming: &str, count: usize) -> Vec<BodyStream> {
    stream::iter(0..count)
        .then(|_| fx.send(fx.get(Some(session), Some(priming))))
        .map(body_of)
        .collect()
        .await
}

/// Opens a GET stream of the session and reads its first frame.
async fn open_get_stream(fx: &Fixture, session: &str) -> BodyStream {
    let mut body = body_of(fx.send(fx.get(Some(session), None)).await);
    next_frames(&mut body).await;
    body
}

#[test]
async fn the_33rd_resume_closes_the_oldest_resumed_stream() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let session_id: McpSessionId = session.parse().unwrap();
    let priming = cut_call(&fx, &session, 4, "g").await;

    let mut resumed = resume_times(&fx, &session, &priming, 33).await;
    assert_eq!(fx.replica.streams().count(session_id), 32);
    let oldest = resumed.remove(0);
    assert!(
        rest(oldest)
            .await
            .is_some_and(|frames| messages(&frames).is_empty()),
        "the oldest resumed stream ends without the result"
    );

    fx.probe.gate("g").open();
    let results: Vec<Vec<Value>> = stream::iter(resumed)
        .then(|body| async move { messages(&rest(body).await.expect("the stream ends")) })
        .collect()
        .await;
    assert!(
        results
            .iter()
            .all(|messages| messages.len() == 1 && messages[0]["id"] == 4),
        "every other resumed stream carries the result: {results:?}"
    );
    fx.finish();
}

#[test]
#[test_r::timeout("30s")]
async fn a_resumed_stream_ends_when_the_record_is_gone() {
    let fx = Fixture::with_config(config(Duration::from_secs(1), Duration::from_secs(15))).await;
    let session = fx.initialize().await;
    let priming = cut_call(&fx, &session, 4, "g").await;
    let resumed = resume_times(&fx, &session, &priming, 1).await;
    fx.store
        .inner
        .delete(session.parse().unwrap())
        .await
        .unwrap();
    assert!(
        rest(resumed.into_iter().next().unwrap()).await.is_some(),
        "the resumed stream ends while the call runs"
    );
    fx.probe.gate("g").open();
    fx.finish();
}

#[test]
async fn resumed_and_get_streams_share_the_limit() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let session_id: McpSessionId = session.parse().unwrap();
    let priming = cut_call(&fx, &session, 4, "g").await;

    let get_stream = open_get_stream(&fx, &session).await;
    let mut resumed = resume_times(&fx, &session, &priming, 31).await;
    assert_eq!(fx.replica.streams().count(session_id), 32);
    resumed.extend(resume_times(&fx, &session, &priming, 1).await);
    assert!(
        rest(get_stream).await.is_some(),
        "a resume closes the oldest stream, a GET stream"
    );

    let _newest_get = open_get_stream(&fx, &session).await;
    assert_eq!(fx.replica.streams().count(session_id), 32);
    assert!(
        rest(resumed.remove(0)).await.is_some(),
        "a GET stream closes the oldest stream, a resumed stream"
    );
    fx.probe.gate("g").open();
    fx.finish();
}

#[test]
async fn a_notification_does_not_reach_a_resumed_stream() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let session = fx.initialize().await;
    // A response stream of the session that can still get events.
    let stream = "running-call";
    fx.replica
        .events()
        .bind_call(stream, session.parse().unwrap());
    let events = fx.replica.event_store();
    let priming = events
        .store_event(stream, &ServerSseMessage::retry(Duration::from_secs(3)))
        .await
        .unwrap();
    let resumed = resume_times(&fx, &session, &priming, 1).await;
    let mut get_stream = open_get_stream(&fx, &session).await;

    let refused = answer(
        fx.send(fx.post_with(&session, call_tool(2, AGENT_TOOL), &next))
            .await,
    )
    .await;
    assert!(is_tool_set_changed(&refused), "{refused:?}");
    let notified = messages(&next_frames(&mut get_stream).await);
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0]["method"], "notifications/tools/list_changed");

    events
        .store_event(
            stream,
            &ServerSseMessage::from_message(ServerJsonRpcMessage::Error(JsonRpcError::new(
                Some(RequestId::Number(4)),
                ErrorData::internal_error("done", None),
            ))),
        )
        .await
        .unwrap();
    let replayed = messages(
        &rest(resumed.into_iter().next().unwrap())
            .await
            .expect("the resumed stream ends after the response"),
    );
    assert_eq!(replayed.len(), 1, "{replayed:?}");
    assert_eq!(replayed[0]["id"], 4);
    fx.finish();
}

// ---------------------------------------------------------------------------------------------
// Cancel

#[test]
async fn cancelled_native_calls_store_no_terminal() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let session_id: McpSessionId = session.parse().unwrap();
    // Two running calls with the same request ID, each with a known response stream.
    let calls: Vec<(BodyStream, Arc<str>)> = stream::iter(["n1", "n2"])
        .then(|gate| {
            let fx = &fx;
            let session = &session;
            async move {
                let mut body = body_of(
                    fx.send(fx.post(Some(session), call(9, "native", gate)))
                        .await,
                );
                let priming: Arc<str> = next_frames(&mut body).await[0]
                    .id
                    .as_deref()
                    .map(Arc::from)
                    .unwrap();
                fx.probe.gate(gate).started().await;
                (body, priming)
            }
        })
        .collect()
        .await;
    let primings: Arc<[Arc<str>]> = calls.iter().map(|(_, priming)| priming.clone()).collect();
    let events = fx.replica.events().clone();
    let replayed = primings.clone();
    // Whether the hook ran, and whether at every run both replays had already ended: a replay
    // of a stream ends at once only when the stream was marked cancelled.
    let hook_runs = Arc::new(AtomicUsize::new(0));
    let all_marked = Arc::new(AtomicBool::new(true));
    fx.replica.hooks().set(HookPoint::BetweenMarkAndCancel, {
        let (hook_runs, all_marked) = (hook_runs.clone(), all_marked.clone());
        Arc::new(move || {
            let replays: Vec<_> = replayed
                .iter()
                .filter_map(|priming| events.replay(priming, session_id))
                .map(|replay| replay.collect::<Vec<_>>())
                .collect();
            let (hook_runs, all_marked) = (hook_runs.clone(), all_marked.clone());
            async move {
                hook_runs.fetch_add(1, Ordering::SeqCst);
                let mut replays = Box::pin(futures::future::join_all(replays));
                if futures::poll!(replays.as_mut()).is_pending() {
                    all_marked.store(false, Ordering::SeqCst);
                    // Let the responses of calls cancelled too early be stored, at most a
                    // second, so that the resumes below show them.
                    let _ = timeout(Duration::from_secs(1), replays).await;
                }
            }
            .boxed()
        })
    });
    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(9))).await.status(),
        StatusCode::ACCEPTED
    );
    assert!(
        hook_runs.load(Ordering::SeqCst) > 0,
        "the cancel reached the hook"
    );
    assert!(
        all_marked.load(Ordering::SeqCst),
        "every stream was marked before any token was cancelled"
    );
    let outcomes: Vec<&str> = stream::iter(["n1", "n2"])
        .then(|gate| {
            let fx = &fx;
            async move { fx.probe.gate(gate).returned().await }
        })
        .collect()
        .await;
    assert_eq!(outcomes, ["cancelled", "cancelled"]);
    let ends: Vec<Option<Vec<Frame>>> = stream::iter(calls)
        .then(|(body, _)| rest(body))
        .collect()
        .await;
    assert!(
        ends.iter().all(|end| end
            .as_ref()
            .is_some_and(|frames| messages(frames).is_empty())),
        "both bodies end with no result: {ends:?}"
    );
    let resumed: Vec<Vec<Frame>> = stream::iter(primings.iter())
        .then(|priming| {
            let fx = &fx;
            let session = &session;
            async move { read_to_end(fx.send(fx.get(Some(session), Some(&**priming))).await).await }
        })
        .collect()
        .await;
    assert!(
        resumed.iter().all(|frames| messages(frames).is_empty()),
        "no terminal is replayed: {resumed:?}"
    );
    fx.finish();
}

#[test]
async fn a_failed_delete_ends_nothing() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let session_id: McpSessionId = session.parse().unwrap();
    let mut call_body = body_of(
        fx.send(fx.post(Some(&session), call(4, "native", "d")))
            .await,
    );
    next_frames(&mut call_body).await;
    fx.probe.gate("d").started().await;
    let mut get_stream = body_of(fx.send(fx.get(Some(&session), None)).await);
    next_frames(&mut get_stream).await;

    fx.store.failing_deletes.store(true, Ordering::SeqCst);
    assert_eq!(
        fx.send(fx.delete(&session)).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    assert!(fx.record(&session).await.is_some());
    assert_eq!(
        fx.replica.calls().count(session_id),
        1,
        "the call still runs"
    );
    assert_eq!(
        fx.replica.streams().count(session_id),
        1,
        "the GET stream stays open"
    );
    fx.probe.gate("d").open();
    assert_eq!(
        fx.probe.gate("d").returned().await,
        "opened",
        "the call was not cancelled"
    );
    assert_eq!(messages(&rest(call_body).await.unwrap()).len(), 1);
    drop(get_stream);
    fx.finish();
}

#[test]
async fn a_cancel_through_another_domain_gets_404_and_cancels_nothing() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let mut body = body_of(
        fx.send(fx.post(Some(&session), call(9, "native", "n")))
            .await,
    );
    next_frames(&mut body).await;
    fx.probe.gate("n").started().await;
    let other_domain = deployment("other.example.com");
    let cancel = fx.authenticated(fx.post(Some(&session), cancel(9)), &other_domain);
    assert_eq!(fx.send(cancel).await.status(), StatusCode::NOT_FOUND);
    fx.probe.gate("n").open();
    assert_eq!(fx.probe.gate("n").returned().await, "opened");
    assert_eq!(messages(&rest(body).await.unwrap()).len(), 1);
    fx.finish();
}

#[test]
async fn a_cancel_reaches_the_running_call_with_a_reused_request_id() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let mut first = body_of(
        fx.send(fx.post(Some(&session), call(5, "gated", "a")))
            .await,
    );
    next_frames(&mut first).await;
    fx.probe.gate("a").started().await;
    let mut second = body_of(
        fx.send(fx.post(Some(&session), call(5, "gated", "b")))
            .await,
    );
    next_frames(&mut second).await;
    fx.probe.gate("b").started().await;

    fx.probe.gate("a").open();
    assert_eq!(
        messages(&rest(first).await.unwrap()).len(),
        1,
        "the first call answers"
    );
    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(5))).await.status(),
        StatusCode::ACCEPTED
    );
    let second_end = rest(second)
        .await
        .expect("the second body ends while its gate is closed");
    assert!(messages(&second_end).is_empty());

    fx.probe.gate("b").open();
    fx.replica.calls().wait_until_empty().await;
    assert_eq!(fx.replica.calls().count(session.parse().unwrap()), 0);
    fx.finish();
}

/// Starts a gated call, cancels it, and checks that its body ends with no result and that the
/// call leaves the table.
async fn add_and_cancel_a_call(fx: &Fixture, session: &str, id: i64, gate_name: &str) {
    let mut body = body_of(
        fx.send(fx.post(Some(session), call(id, "gated", gate_name)))
            .await,
    );
    next_frames(&mut body).await;
    let gate = fx.probe.gate(gate_name);
    gate.started().await;
    assert_eq!(
        fx.send(fx.post(Some(session), cancel(id))).await.status(),
        StatusCode::ACCEPTED
    );
    let end = rest(body).await.expect("the cancelled call's body ends");
    assert!(messages(&end).is_empty(), "call {id} sends no result");
    gate.open();
    gate.returned().await;
    assert_eq!(
        fx.replica.calls().count(session.parse().unwrap()),
        0,
        "call {id} left the table"
    );
}

#[test]
#[test_r::timeout("30s")]
async fn cancel_and_completion_race_without_a_stuck_call() {
    let fx = Arc::new(Fixture::new().await);
    let session: Arc<str> = fx.initialize().await.into();
    stream::iter(0..200i64)
        .for_each(|id| {
            let fx = fx.clone();
            let session = session.clone();
            async move {
                let gate_name = format!("race-{id}");
                let mut body = body_of(
                    fx.send(fx.post(Some(&session), call(id, "gated", &gate_name)))
                        .await,
                );
                next_frames(&mut body).await;
                let gate = fx.probe.gate(&gate_name);
                gate.started().await;
                let barrier = Arc::new(tokio::sync::Barrier::new(2));
                let canceller = tokio::spawn({
                    let (fx, session, barrier) = (fx.clone(), session.clone(), barrier.clone());
                    async move {
                        barrier.wait().await;
                        fx.send(fx.post(Some(&session), cancel(id))).await.status()
                    }
                });
                let opener = tokio::spawn({
                    let (gate, barrier) = (gate.clone(), barrier.clone());
                    async move {
                        barrier.wait().await;
                        gate.open();
                    }
                });
                assert_eq!(canceller.await.unwrap(), StatusCode::ACCEPTED);
                opener.await.unwrap();
                assert!(rest(body).await.is_some(), "call {id} ends");
                gate.returned().await;
                assert_eq!(
                    fx.replica.calls().count(session.parse().unwrap()),
                    0,
                    "call {id} left the table"
                );
                add_and_cancel_a_call(&fx, &session, 10_000 + id, &format!("after-{id}")).await;
            }
        })
        .await;
    Arc::into_inner(fx).unwrap().finish();
}

// ---------------------------------------------------------------------------------------------
// Bind order

/// When the request of a call ends, relative to the binding of its response stream.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CallEnds {
    BeforeBind,
    BetweenBindSteps,
    AfterBind,
    Never,
}

fn end_call(call: &Mutex<Option<super::calls::RunningCallGuard>>) {
    let taken = call.lock().unwrap().take();
    drop(taken);
}

/// Binds the response stream of a call whose request ends at `ends`, and returns whether the
/// stream is running and whether the call is in the table.
async fn bind_when_the_call_ends(ends: CallEnds) -> (bool, bool) {
    let replica = McpReplica::new(Arc::new(NoStore), &McpConfig::default());
    let session = super::test_ids::session();
    let (_token, call) = replica
        .calls()
        .add(replica.events(), session, RequestId::Number(1));
    let entry = call.entry();
    let call = Arc::new(Mutex::new(Some(call)));
    if ends == CallEnds::BeforeBind {
        end_call(&call);
    }
    if ends == CallEnds::BetweenBindSteps {
        let call = call.clone();
        replica.hooks().set(
            HookPoint::BetweenBindSteps,
            Arc::new(move || {
                end_call(&call);
                std::future::ready(()).boxed()
            }),
        );
    }
    replica.bind_call(session, entry, "s").await;
    if ends == CallEnds::AfterBind {
        end_call(&call);
    }
    let (_, running) = replica.events().binding("s").unwrap();
    let call_exists = call.lock().unwrap().is_some();
    (running, call_exists)
}

struct NoStore;

#[async_trait]
impl McpSessionStore for NoStore {
    async fn create(
        &self,
        _: McpSessionId,
        _: &McpSessionRecord,
        _: Duration,
    ) -> Result<bool, McpSessionStoreError> {
        unreachable!()
    }
    async fn get(
        &self,
        _: McpSessionId,
        _: Option<Duration>,
    ) -> Result<Option<McpSessionRecord>, McpSessionStoreError> {
        unreachable!()
    }
    async fn replace(
        &self,
        _: McpSessionId,
        _: &McpSessionRecord,
    ) -> Result<bool, McpSessionStoreError> {
        unreachable!()
    }
    async fn delete(&self, _: McpSessionId) -> Result<(), McpSessionStoreError> {
        unreachable!()
    }
}

#[test]
async fn bind_keeps_running_exactly_while_the_call_exists() {
    let rows = [
        CallEnds::BeforeBind,
        CallEnds::BetweenBindSteps,
        CallEnds::AfterBind,
        CallEnds::Never,
    ];
    let results: Vec<(CallEnds, bool, bool)> = stream::iter(rows)
        .then(|ends| async move {
            let (running, call_exists) = bind_when_the_call_ends(ends).await;
            (ends, running, call_exists)
        })
        .collect()
        .await;
    results.iter().for_each(|(ends, running, call_exists)| {
        assert_eq!(running, call_exists, "call ends {ends:?}");
    });
    lock_check::assert_no_violation();
}

#[test]
async fn a_call_that_ends_between_the_bind_steps_leaves_its_stream_not_running() {
    let fx = Fixture::new().await;
    let session = fx.initialize().await;
    let bind_gate = Arc::new(Semaphore::new(0));
    let in_bind = Arc::new(Semaphore::new(0));
    fx.replica.hooks().set(HookPoint::BetweenBindSteps, {
        let (bind_gate, in_bind) = (bind_gate.clone(), in_bind.clone());
        Arc::new(move || {
            let (bind_gate, in_bind) = (bind_gate.clone(), in_bind.clone());
            async move {
                in_bind.add_permits(1);
                bind_gate.acquire().await.unwrap().forget();
            }
            .boxed()
        })
    });
    let response = fx
        .send(fx.post(Some(&session), call(1, "gated", "g")))
        .await;
    let reader = tokio::spawn(async move {
        let mut body = body_of(response);
        let priming = next_frames(&mut body).await;
        (priming, body)
    });
    timeout(WAIT, in_bind.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    fx.probe.gate("g").open();
    fx.probe.gate("g").returned().await;
    fx.replica.calls().wait_until_empty().await;
    bind_gate.add_permits(1);
    let (priming, body) = reader.await.unwrap();
    let stream = super::event_store::parse_event_id(priming[0].id.as_deref().unwrap())
        .unwrap()
        .0
        .to_string();
    assert_eq!(
        fx.replica
            .events()
            .binding(&stream)
            .map(|(_, running)| running),
        Some(false)
    );
    assert_eq!(messages(&rest(body).await.unwrap()).len(), 1);
    fx.finish();
}

// ---------------------------------------------------------------------------------------------
// Golem's handler behind the layer

/// A deployment cache with one domain. `get` answers from its entry, which an invalidation
/// empties; the next `get` fills it with the current deployment.
struct OneDomainLookup {
    current: Mutex<Arc<McpDeployment>>,
    cached: Mutex<Option<Arc<McpDeployment>>>,
    invalidations: AtomicUsize,
}

impl OneDomainLookup {
    fn new(current: &Arc<McpDeployment>) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(current.clone()),
            cached: Mutex::new(None),
            invalidations: AtomicUsize::new(0),
        })
    }

    /// A new deployment becomes current. The cached entry stays until an invalidation, as it
    /// does until the registry's event reaches the replica.
    fn deploy(&self, deployment: &Arc<McpDeployment>) {
        *self.current.lock().unwrap() = deployment.clone();
    }

    fn invalidations(&self) -> usize {
        self.invalidations.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl McpCapabilityLookup for OneDomainLookup {
    async fn get(&self, _: &Domain) -> Result<Arc<McpDeployment>, McpCapabilitiesLookupError> {
        let current = self.current.lock().unwrap().clone();
        Ok(self.cached.lock().unwrap().get_or_insert(current).clone())
    }

    async fn invalidate(&self, _: &Domain) {
        self.invalidations.fetch_add(1, Ordering::SeqCst);
        *self.cached.lock().unwrap() = None;
    }

    async fn invalidate_all(&self) {
        self.invalidate(&Domain(String::new())).await;
    }
}

/// The harness's agent method as an MCP tool.
const AGENT_TOOL: &str = "mcp-agent-run";
/// The harness's native tool.
const NATIVE_TOOL: &str = "native";

/// The harness's deployment of `tools.example.com` at the first revision, and the same
/// deployment at the next revision.
fn harness_deployments(harness: &InvocationHarness) -> (Arc<McpDeployment>, Arc<McpDeployment>) {
    let first = Arc::new(McpDeployment::new(harness.compiled_mcp(
        "tools.example.com",
        DeploymentRevision::INITIAL,
        true,
    )));
    let next = at_revision(&first, DeploymentRevision::INITIAL.next().unwrap());
    (first, next)
}

fn call_tool(id: i64, name: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
           "params": {"name": name, "arguments": {}}})
}

/// The JSON-RPC messages of a whole response.
async fn answer(response: Response) -> Vec<Value> {
    assert_eq!(response.status(), StatusCode::OK);
    messages(&read_to_end(response).await)
}

const TOOL_SET_CHANGED: &str = "tool set changed; list tools again";

/// Whether `messages` are the answer to a call refused because the tool set changed: the
/// `tools/list_changed` notification, then the error.
fn is_tool_set_changed(messages: &[Value]) -> bool {
    matches!(
        messages,
        [notification, error]
            if notification["method"] == "notifications/tools/list_changed"
                && error["error"]["message"] == TOOL_SET_CHANGED
    )
}

/// Whether `messages` are a successful result.
fn is_result(messages: &[Value]) -> bool {
    matches!(messages, [result] if result.get("result").is_some())
}

#[test]
async fn any_replica_serves_any_request() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, _) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let session = fx.initialize().await;
    let (other_replica, _replica) = fx.second_replica();
    list_and_call(&fx, &other_replica, &session).await;
    fx.finish();
}

/// Lists the tools of `session` through `endpoint`, then calls the harness's agent tool there.
async fn list_and_call(fx: &Fixture, endpoint: &BoxEndpoint<'static>, session: &str) {
    let listed = answer(
        endpoint
            .call(fx.post(Some(session), tools_list(2)))
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<&str> = listed[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&AGENT_TOOL), "{names:?}");
    let called = answer(
        endpoint
            .call(fx.post(Some(session), call_tool(3, AGENT_TOOL)))
            .await
            .unwrap(),
    )
    .await;
    assert!(is_result(&called), "{called:?}");
}

#[test]
async fn session_survives_a_stopped_replica() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, _) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let (stopped, stopped_replica) = fx.second_replica();
    let session = initialize_on(&stopped, fx.post(None, initialize())).await;
    let weak = Arc::downgrade(&stopped_replica);
    drop(stopped_replica);
    drop(stopped);
    assert!(
        weak.upgrade().is_none(),
        "nothing keeps the stopped replica's memory"
    );

    let (started, _replica) = fx.second_replica();
    list_and_call(&fx, &started, &session).await;
    fx.finish();
}

#[test]
async fn a_deploy_between_list_and_call_gives_tool_set_changed_then_works_after_a_list() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let session = fx.initialize().await;
    answer(fx.send(fx.post(Some(&session), tools_list(2))).await).await;

    let after_deploy = answer(
        fx.send(fx.post_with(&session, call_tool(3, AGENT_TOOL), &next))
            .await,
    )
    .await;
    assert!(is_tool_set_changed(&after_deploy), "{after_deploy:?}");

    answer(fx.send(fx.post_with(&session, tools_list(4), &next)).await).await;
    assert_eq!(
        fx.record(&session).await.unwrap().deployment_revision,
        next.compiled.deployment_revision
    );
    let after_list = answer(
        fx.send(fx.post_with(&session, call_tool(5, AGENT_TOOL), &next))
            .await,
    )
    .await;
    assert!(is_result(&after_list), "{after_list:?}");
    fx.finish();
}

#[test]
async fn a_stale_replica_costs_one_tool_set_changed() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (stale, current) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&stale), stale.clone(), false).await;
    // Replica B already serves the new revision; replica A (the fixture's) still serves the old.
    let (b, _replica) = fx.second_replica();
    let session = initialize_on(&b, fx.authenticated(fx.post(None, initialize()), &current)).await;

    answer(fx.send(fx.post(Some(&session), tools_list(2))).await).await;
    assert_eq!(
        fx.record(&session).await.unwrap().deployment_revision,
        stale.compiled.deployment_revision,
        "a list on the stale replica pins the revision it served"
    );

    let calls_on_b = |id| b.call(fx.post_with(&session, call_tool(id, AGENT_TOOL), &current));
    let first_call = answer(calls_on_b(3).await.unwrap()).await;
    answer(
        b.call(fx.post_with(&session, tools_list(4), &current))
            .await
            .unwrap(),
    )
    .await;
    let second_call = answer(calls_on_b(5).await.unwrap()).await;

    assert!(is_tool_set_changed(&first_call), "{first_call:?}");
    assert!(is_result(&second_call), "{second_call:?}");
    assert_eq!(
        fx.record(&session).await.unwrap().deployment_revision,
        current.compiled.deployment_revision
    );
    fx.finish();
}

#[test]
async fn a_list_writes_the_pin_only_when_it_differs() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let session = fx.initialize().await;
    let replaces = |fx: &Fixture| {
        fx.store
            .ops()
            .iter()
            .filter(|op| **op == Op::Replace)
            .count()
    };

    answer(fx.send(fx.post(Some(&session), tools_list(2))).await).await;
    assert_eq!(
        replaces(&fx),
        0,
        "a list of the pinned revision writes nothing"
    );
    answer(fx.send(fx.post_with(&session, tools_list(3), &next)).await).await;
    assert_eq!(
        replaces(&fx),
        1,
        "a list of another revision writes the pin"
    );
    assert_eq!(
        fx.record(&session).await.unwrap().deployment_revision,
        next.compiled.deployment_revision
    );
    fx.finish();
}

#[test]
async fn a_rollback_gives_tool_set_changed_then_works_after_a_list() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (older, newer) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&older), older.clone(), false).await;
    let session = initialize_on(
        &fx.endpoint,
        fx.authenticated(fx.post(None, initialize()), &newer),
    )
    .await;

    let after_rollback = answer(
        fx.send(fx.post(Some(&session), call_tool(2, AGENT_TOOL)))
            .await,
    )
    .await;
    assert!(is_tool_set_changed(&after_rollback), "{after_rollback:?}");

    answer(fx.send(fx.post(Some(&session), tools_list(3))).await).await;
    assert_eq!(
        fx.record(&session).await.unwrap().deployment_revision,
        older.compiled.deployment_revision
    );
    let after_list = answer(
        fx.send(fx.post(Some(&session), call_tool(4, AGENT_TOOL)))
            .await,
    )
    .await;
    assert!(is_result(&after_list), "{after_list:?}");
    fx.finish();
}

#[test]
async fn tool_set_changed_reaches_the_sessions_get_streams_on_this_replica() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let session = fx.initialize().await;
    let other = fx.initialize().await;
    let mut get_stream = body_of(fx.send(fx.get(Some(&session), None)).await);
    let mut other_stream = body_of(fx.send(fx.get(Some(&other), None)).await);
    next_frames(&mut get_stream).await;
    next_frames(&mut other_stream).await;

    let refused = answer(
        fx.send(fx.post_with(&session, call_tool(2, AGENT_TOOL), &next))
            .await,
    )
    .await;
    assert!(is_tool_set_changed(&refused), "{refused:?}");

    let notified = messages(&next_frames(&mut get_stream).await);
    assert_eq!(notified.len(), 1);
    assert_eq!(notified[0]["method"], "notifications/tools/list_changed");
    assert!(
        timeout(Duration::from_millis(200), other_stream.next())
            .await
            .is_err(),
        "another session's stream gets nothing"
    );
    fx.finish();
}

/// Starts a native call on a fresh session and waits until its tool session is open.
async fn running_native_call(
    fx: &Fixture,
    harness: &InvocationHarness,
    id: i64,
) -> (String, BodyStream) {
    let session = fx.initialize().await;
    let mut body = body_of(
        fx.send(fx.post(Some(&session), call_tool(id, NATIVE_TOOL)))
            .await,
    );
    next_frames(&mut body).await;
    harness.native_sessions_opened(1).await;
    (session, body)
}

#[test]
async fn a_cancel_stops_a_native_tool_through_the_entry_token() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    harness.answer_native_sessions(crate::mcp::invoke::test_support::NativeSessions::Hold);
    let (first, _) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, false).await;
    let (session, body) = running_native_call(&fx, &harness, 4).await;
    let tail = harness.take_native_session_tail();

    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(4))).await.status(),
        StatusCode::ACCEPTED
    );

    timeout(WAIT, tail.collect::<Vec<_>>())
        .await
        .expect("the native tool stops and closes its session");
    let end = rest(body).await.expect("the call's body ends");
    assert!(messages(&end).is_empty());
    timeout(WAIT, fx.replica.calls().wait_until_empty())
        .await
        .expect("the call leaves the table");
    fx.finish();
}

#[test]
async fn a_cancelled_native_call_skips_the_deployment_changed_path() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    harness.answer_native_sessions(crate::mcp::invoke::test_support::NativeSessions::Hold);
    let (first, next) = harness_deployments(&harness);
    let lookup = OneDomainLookup::new(&first);
    let fx = Fixture::golem(&harness, lookup.clone(), first, false).await;
    let (session, _body) = running_native_call(&fx, &harness, 4).await;
    lookup.deploy(&next);

    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(4))).await.status(),
        StatusCode::ACCEPTED
    );

    timeout(WAIT, fx.replica.calls().wait_until_empty())
        .await
        .expect("the cancelled call returns");
    assert_eq!(lookup.invalidations(), 0, "the deployment was not checked");
    fx.finish();
}

#[test]
async fn a_failed_native_call_checks_the_deployment_at_the_time_of_failure() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let lookup = OneDomainLookup::new(&first);
    let fx = Fixture::golem(&harness, lookup.clone(), first.clone(), false).await;
    let session = fx.initialize().await;
    // The replica's cache still holds the first revision when the new one is deployed.
    let _ = lookup.get(&first.compiled.domain).await;
    lookup.deploy(&next);

    let failed = answer(
        fx.send(fx.post(Some(&session), call_tool(2, NATIVE_TOOL)))
            .await,
    )
    .await;

    assert!(is_tool_set_changed(&failed), "{failed:?}");
    assert_eq!(lookup.invalidations(), 1);
    fx.finish();
}

#[test]
async fn a_cancelled_list_sends_no_response() {
    let harness =
        InvocationHarness::with_run_method(golem_common::model::agent::AgentMode::Durable);
    let (first, next) = harness_deployments(&harness);
    let fx = Fixture::golem(&harness, OneDomainLookup::new(&first), first, true).await;
    let session = fx.initialize().await;
    let pin_write = Arc::new(Gate::new());
    *fx.store.replace_gate.lock().unwrap() = Some(pin_write.clone());

    let mut body = body_of(fx.send(fx.post_with(&session, tools_list(6), &next)).await);
    let priming = next_frames(&mut body).await[0].id.clone().unwrap();
    pin_write.started().await;
    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(6))).await.status(),
        StatusCode::ACCEPTED
    );
    let end = rest(body)
        .await
        .expect("the body ends while the pin write waits");
    assert!(messages(&end).is_empty());

    pin_write.open();
    pin_write.returned().await;
    // The `initialize` answer and the list's.
    fx.observed_events().terminals_attempted(2).await;
    let resumed = read_to_end(fx.send(fx.get(Some(&session), Some(&priming))).await).await;
    assert!(messages(&resumed).is_empty(), "{resumed:?}");
    fx.finish();
}

// ---------------------------------------------------------------------------------------------
// The stub's calls: heartbeat and cancel

#[test]
async fn a_cancelled_agent_method_call_sends_no_response() {
    let fx = Fixture::observed().await;
    let session = fx.initialize().await;
    let mut body = body_of(
        fx.send(fx.post(Some(&session), call(5, "gated", "d")))
            .await,
    );
    let priming = next_frames(&mut body).await[0].id.clone().unwrap();
    let gate = fx.probe.gate("d");
    gate.started().await;

    assert_eq!(
        fx.send(fx.post(Some(&session), cancel(5))).await.status(),
        StatusCode::ACCEPTED
    );
    let end = rest(body)
        .await
        .expect("the body ends while the gate is closed");
    assert!(messages(&end).is_empty());

    gate.open();
    assert_eq!(gate.returned().await, "opened");
    // The `initialize` answer and the call's.
    fx.observed_events().terminals_attempted(2).await;
    let resumed = read_to_end(fx.send(fx.get(Some(&session), Some(&priming))).await).await;
    assert!(messages(&resumed).is_empty(), "{resumed:?}");
    fx.finish();
}

/// The number of reads that extend `session`, in `ops`.
fn extending_reads(ops: &[Op], session: &str) -> usize {
    let session: McpSessionId = session.parse().unwrap();
    ops.iter()
        .filter(|op| {
            **op == Op::Get {
                session,
                extend: true,
            }
        })
        .count()
}

#[test]
fn a_long_call_keeps_the_session_alive_and_returns_its_result() {
    on_paused_time(async {
        let fx = Fixture::with_store(
            config(Duration::from_secs(1), Duration::from_secs(15)),
            Arc::new(ClockStore::default()),
            None,
        );
        let session = fx.initialize().await;
        let mut body = body_of(
            fx.send(fx.post(Some(&session), call(2, "gated", "long")))
                .await,
        );
        next_frames(&mut body).await;
        let gate = fx.probe.gate("long");
        gate.started().await;

        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            fx.record(&session).await.is_some(),
            "the session outlives three idle timeouts while its call runs"
        );
        gate.open();
        let answered = messages(&rest(body).await.expect("the call answers"));
        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0]["result"]["content"][0]["text"], "opened");
        assert_eq!(
            fx.send(fx.post(Some(&session), tools_list(3)))
                .await
                .status(),
            StatusCode::OK,
            "the session stays usable"
        );
        fx.finish();
    })
}

#[test]
fn a_panicking_call_stops_its_heartbeat() {
    on_paused_time(async {
        let fx = Fixture::with_store(
            config(Duration::from_secs(3), Duration::from_secs(15)),
            Arc::new(ClockStore::default()),
            None,
        );
        let session = fx.initialize().await;
        let _body = body_of(
            fx.send(fx.post(Some(&session), call(2, "panic", "p")))
                .await,
        );
        let gate = fx.probe.gate("p");
        gate.started().await;
        assert!(
            extending_reads(&fx.store.ops(), &session) >= 2,
            "the heartbeat runs"
        );

        gate.open();
        fx.replica.calls().wait_until_empty().await;
        let after_panic = extending_reads(&fx.store.ops(), &session);
        // Two heartbeat periods of one second each.
        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert_eq!(
            extending_reads(&fx.store.ops(), &session),
            after_panic,
            "no heartbeat after the call ended"
        );
        fx.finish();
    })
}

#[test]
fn the_heartbeat_ticks_when_the_call_starts() {
    on_paused_time(async {
        let fx = Fixture::with_store(
            config(Duration::from_secs(300), Duration::from_secs(15)),
            Arc::new(ClockStore::default()),
            None,
        );
        let session = fx.initialize().await;
        let before = fx.store.ops().len();
        let mut body = body_of(
            fx.send(fx.post(Some(&session), call(2, "gated", "g")))
                .await,
        );
        next_frames(&mut body).await;
        let gate = fx.probe.gate("g");
        gate.started().await;

        let extending = Op::Get {
            session: session.parse().unwrap(),
            extend: true,
        };
        assert_eq!(
            fx.store.ops()[before..],
            [extending.clone(), extending],
            "the admission read and the first heartbeat, before any period has passed, and no \
             other store call"
        );
        gate.open();
        rest(body).await.expect("the call answers");
        fx.finish();
    })
}
