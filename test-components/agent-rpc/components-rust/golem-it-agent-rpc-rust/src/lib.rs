use golem_rust::{agent_definition, agent_implementation, Schema, Uuid};
use golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime;
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
        let mut self_client =
            ScheduledInvocationClientClient::get(self._name.clone());
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
