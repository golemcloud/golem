use golem_rust::bindings::golem::agent::host::{Datetime, RpcError};
use golem_rust::{agent_definition, agent_implementation, PromiseId, Schema, Uuid};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Schema)]
pub enum State {
    Initial,
    Ongoing,
}

#[derive(Debug, Clone, Schema)]
pub struct Payload {
    pub field1: String,
    pub field2: Uuid,
    pub field3: State,
}

#[agent_definition]
pub trait RustParent {
    fn new(name: String) -> Self;

    async fn spawn_child(&self, data: String) -> Uuid;
    async fn call_ts_agent(&self, name: String) -> f64;
}

struct RustParentImpl {
    _name: String,
}

#[agent_implementation]
impl RustParent for RustParentImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn spawn_child(&self, data: String) -> Uuid {
        let uuid = Uuid::new_v4();
        let payload = Payload {
            field1: data,
            field2: uuid,
            field3: State::Initial,
        };
        let mut child = RustChildClient::get_(uuid.clone());
        child.set(payload).await;
        uuid
    }

    async fn call_ts_agent(&self, name: String) -> f64 {
        let client = SimpleChildAgentClient::get(name);
        client.value().await
    }
}

#[agent_definition]
pub trait RustChild {
    fn new(id: Uuid) -> Self;
    fn set(&mut self, payload: Payload);
    fn get(&self) -> Option<Payload>;
}

struct RustChildImpl {
    _id: Uuid,
    payload: Option<Payload>,
}

#[agent_implementation]
impl RustChild for RustChildImpl {
    fn new(id: Uuid) -> Self {
        Self {
            _id: id,
            payload: None,
        }
    }

    fn set(&mut self, payload: Payload) {
        self.payload = Some(payload);
    }

    fn get(&self) -> Option<Payload> {
        self.payload.clone()
    }
}

#[agent_definition]
pub trait SimpleChildAgent {
    fn new(name: String) -> Self;
    fn value(&self) -> f64;
}
// implemented in `golem-it-agent-rpc`

#[agent_definition]
pub trait Counter {
    fn new(id: String) -> Self;
    fn get_value(&self) -> String;
}

struct CounterImpl {
    id: String,
}

#[agent_implementation]
impl Counter for CounterImpl {
    fn new(id: String) -> Self {
        Self { id }
    }

    fn get_value(&self) -> String {
        format!("counter-{}", self.id)
    }
}

// -- Scheduled invocation agents --

fn datetime_200ms_from_now() -> Datetime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let scheduled = now + Duration::from_millis(200);
    Datetime {
        seconds: scheduled.as_secs(),
        nanoseconds: scheduled.subsec_nanos(),
    }
}

#[agent_definition]
pub trait ScheduledInvocationServer {
    fn new(name: String) -> Self;
    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
}

struct ScheduledInvocationServerImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl ScheduledInvocationServer for ScheduledInvocationServerImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }
}

#[agent_definition]
pub trait ScheduledInvocationClient {
    fn new(name: String) -> Self;

    /// Schedule inc_global_by on the server agent 200ms in the future
    fn test1(&self, server_agent_name: String);

    /// Schedule inc_global_by on the server agent 200ms in the future, then cancel it
    fn test2(&self, server_agent_name: String);

    /// Schedule inc_global_by on self 200ms in the future
    fn test3(&mut self);

    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
}

struct ScheduledInvocationClientImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl ScheduledInvocationClient for ScheduledInvocationClientImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn test1(&self, server_agent_name: String) {
        let mut server = ScheduledInvocationServerClient::get(server_agent_name);
        let scheduled_for = datetime_200ms_from_now();
        server.schedule_inc_global_by(1, scheduled_for);
    }

    fn test2(&self, server_agent_name: String) {
        let mut server = ScheduledInvocationServerClient::get(server_agent_name);
        let scheduled_for = datetime_200ms_from_now();
        let token = server.schedule_cancelable_inc_global_by(1, scheduled_for);
        token.cancel();
    }

    fn test3(&mut self) {
        let mut self_client = ScheduledInvocationClientClient::get(self._name.clone());
        let scheduled_for = datetime_200ms_from_now();
        self_client.schedule_inc_global_by(1, scheduled_for);
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }
}

// -- RPC test agents (replacing old caller/counters components) --

#[agent_definition]
pub trait RpcCounter {
    fn new(name: String) -> Self;
    fn inc_by(&mut self, value: u64);
    fn get_value(&self) -> u64;
    fn get_args(&self) -> Vec<String>;
    fn get_env(&self) -> Vec<(String, String)>;
}

struct RpcCounterImpl {
    _name: String,
    value: u64,
}

#[agent_implementation]
impl RpcCounter for RpcCounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            value: 0,
        }
    }

    fn inc_by(&mut self, value: u64) {
        self.value += value;
    }

    fn get_value(&self) -> u64 {
        self.value
    }

    fn get_args(&self) -> Vec<String> {
        std::env::args().collect()
    }

    fn get_env(&self) -> Vec<(String, String)> {
        std::env::vars().collect()
    }
}

#[derive(Debug, Clone, Schema)]
pub enum TimelineNode {
    Leaf,
}

#[agent_definition]
pub trait RpcGlobalState {
    fn new(name: String) -> Self;
    fn inc_global_by(&mut self, value: u64);
    fn get_global_value(&self) -> u64;
    fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode;
    fn bug_golem1265(&self, s: String) -> Result<(), String>;
}

struct RpcGlobalStateImpl {
    _name: String,
    global: u64,
}

#[agent_implementation]
impl RpcGlobalState for RpcGlobalStateImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            global: 0,
        }
    }

    fn inc_global_by(&mut self, value: u64) {
        self.global += value;
    }

    fn get_global_value(&self) -> u64 {
        self.global
    }

    fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode {
        node
    }

    fn bug_golem1265(&self, s: String) -> Result<(), String> {
        log::info!("Got {s}");
        Ok(())
    }
}

#[agent_definition]
pub trait RpcCaller {
    fn new(name: String) -> Self;

    /// test1: Create 3 counter agents, increment them, return their values
    async fn test1(&self) -> Vec<(String, u64)>;

    /// test2: Use a persistent counter agent, increment on each call
    async fn test2(&mut self) -> u64;

    /// test3: Use a global state agent, increment on each call
    async fn test3(&self) -> u64;

    /// test4: Get args and env from a counter agent (context inheritance)
    async fn test4(&self) -> (Vec<String>, Vec<(String, String)>);

    /// test5: Create 3 counter agents in separate workers, increment them independently, return values
    async fn test5(&self) -> Vec<u64>;

    /// bug-wasm-rpc-i32: Pass a variant through RPC
    async fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode;

    /// bug-golem1265: Pass a string through RPC and get Result back
    async fn bug_golem1265(&self, s: String) -> Result<(), String>;
}

struct RpcCallerImpl {
    name: String,
    counter_name: Option<String>,
}

#[agent_implementation]
impl RpcCaller for RpcCallerImpl {
    fn new(name: String) -> Self {
        Self {
            name,
            counter_name: None,
        }
    }

    async fn test1(&self) -> Vec<(String, u64)> {
        let counter_prefix = format!("{}_test1", self.name);

        let mut counter1 = RpcCounterClient::get(format!("{counter_prefix}_counter1"));
        let mut counter2 = RpcCounterClient::get(format!("{counter_prefix}_counter2"));
        let mut counter3 = RpcCounterClient::get(format!("{counter_prefix}_counter3"));

        counter1.inc_by(1).await;
        counter1.inc_by(1).await;
        counter1.inc_by(1).await;

        counter2.inc_by(2).await;
        counter2.inc_by(1).await;

        counter3.inc_by(3).await;

        let value1 = counter1.get_value().await;
        let value2 = counter2.get_value().await;
        let value3 = counter3.get_value().await;

        vec![
            (format!("{counter_prefix}_counter3"), value3),
            (format!("{counter_prefix}_counter2"), value2),
            (format!("{counter_prefix}_counter1"), value1),
        ]
    }

    async fn test2(&mut self) -> u64 {
        let counter_name = match &self.counter_name {
            Some(n) => n.clone(),
            None => {
                let n = format!("{}_test2_counter", self.name);
                self.counter_name = Some(n.clone());
                n
            }
        };
        let mut counter = RpcCounterClient::get(counter_name);
        counter.inc_by(1).await;
        counter.get_value().await
    }

    async fn test3(&self) -> u64 {
        let mut global = RpcGlobalStateClient::get(format!("{}_test3", self.name));
        global.inc_global_by(1).await;
        global.get_global_value().await
    }

    async fn test4(&self) -> (Vec<String>, Vec<(String, String)>) {
        let counter = RpcCounterClient::get(format!("{}_test4_counter", self.name));
        let args = counter.get_args().await;
        let env = counter.get_env().await;
        (args, env)
    }

    async fn test5(&self) -> Vec<u64> {
        let counter_prefix = format!("{}_test5", self.name);

        let mut counter1 = RpcCounterClient::get(format!("{counter_prefix}_counter1"));
        let mut counter2 = RpcCounterClient::get(format!("{counter_prefix}_counter2"));
        let mut counter3 = RpcCounterClient::get(format!("{counter_prefix}_counter3"));

        counter1.inc_by(1).await;
        counter1.inc_by(1).await;
        counter1.inc_by(1).await;

        counter2.inc_by(2).await;
        counter2.inc_by(1).await;

        counter3.inc_by(3).await;

        let value1 = counter1.get_value().await;
        let value2 = counter2.get_value().await;
        let value3 = counter3.get_value().await;

        vec![value1, value2, value3]
    }

    async fn bug_wasm_rpc_i32(&self, node: TimelineNode) -> TimelineNode {
        let global = RpcGlobalStateClient::get(format!("{}_bug32", self.name));
        global.bug_wasm_rpc_i32(node).await
    }

    async fn bug_golem1265(&self, s: String) -> Result<(), String> {
        let global = RpcGlobalStateClient::get(format!("{}_bug1265", self.name));
        global.bug_golem1265(s).await
    }
}

#[agent_definition]
pub trait RpcBlockingCounter {
    fn new(name: String) -> Self;
    fn inc_by(&mut self, value: u64);
    fn get_value(&self) -> u64;
    /// Creates a promise and returns its ID so the test can complete it later
    fn create_promise(&self) -> PromiseId;
    /// Blocks on a previously created promise
    fn await_promise(&self, promise_id: PromiseId);
}

struct RpcBlockingCounterImpl {
    _name: String,
    value: u64,
}

#[agent_implementation]
impl RpcBlockingCounter for RpcBlockingCounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            value: 0,
        }
    }

    fn inc_by(&mut self, value: u64) {
        self.value += value;
    }

    fn get_value(&self) -> u64 {
        self.value
    }

    fn create_promise(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    fn await_promise(&self, promise_id: PromiseId) {
        golem_rust::blocking_await_promise(&promise_id);
    }
}

// -- RPC auth parity test agent --

/// Mirror of the WIT `rpc-error` variant with `Schema` so it can be returned
/// from an agent method and pattern-matched in integration tests.
#[derive(Debug, Clone, Schema)]
pub enum RpcCallOutcome {
    Ok,
    Denied { details: String },
    NotFound { details: String },
    ProtocolError { details: String },
    RemoteInternalError { details: String },
}

impl From<RpcError> for RpcCallOutcome {
    fn from(e: RpcError) -> Self {
        match e {
            RpcError::Denied(details) => Self::Denied { details },
            RpcError::NotFound(details) => Self::NotFound { details },
            RpcError::ProtocolError(details) => Self::ProtocolError { details },
            RpcError::RemoteInternalError(details) => Self::RemoteInternalError { details },
            RpcError::RemoteAgentError(_) => Self::RemoteInternalError {
                details: "remote agent error".to_string(),
            },
        }
    }
}

/// Agent used to test RPC authorization parity (local vs remote path).
/// All methods return `RpcCallOutcome` so integration tests can do typed assertions
/// on the exact error variant rather than string matching.
#[agent_definition]
pub trait RpcAuthTester {
    fn new(name: String) -> Self;

    /// Attempt to call `inc_by(1)` on an `RpcCounter` agent with the given name.
    /// Returns `RpcCallOutcome::Ok` on success or a typed denial/error on failure.
    async fn try_call_counter(&self, counter_name: String) -> RpcCallOutcome;
}

struct RpcAuthTesterImpl {
    _name: String,
}

#[agent_implementation]
impl RpcAuthTester for RpcAuthTesterImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn try_call_counter(&self, counter_name: String) -> RpcCallOutcome {
        use golem_rust::agentic::Schema;
        use golem_rust::bindings::golem::agent::host::WasmRpc;

        // Build the constructor data value: RpcCounter::new(counter_name: String)
        let constructor = Schema::to_data_value(counter_name)
            .expect("Failed to encode constructor parameter");

        // Connect to the RpcCounter agent in the same component.
        // WasmRpc::new resolves the component_id from the registered agent type.
        let rpc = WasmRpc::new("RpcCounter", &constructor, None, &[]);

        // Invoke inc-by(1u64)
        let arg = Schema::to_data_value(1u64)
            .expect("Failed to encode method parameter");

        match rpc.invoke_and_await("inc-by", &arg) {
            Ok(_) => RpcCallOutcome::Ok,
            Err(e) => RpcCallOutcome::from(e),
        }
    }
}
