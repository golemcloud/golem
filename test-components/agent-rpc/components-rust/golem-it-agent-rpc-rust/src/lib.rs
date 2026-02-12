use golem_rust::{agent_definition, agent_implementation, Schema, Uuid};

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
