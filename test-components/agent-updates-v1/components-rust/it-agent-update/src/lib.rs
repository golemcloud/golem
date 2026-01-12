use golem_rust::{agent_definition, agent_implementation, description};

#[agent_definition]
#[description("Counter agent V1")]
pub trait CounterAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }
}

#[agent_definition]
pub trait Caller {
    fn new() -> Self;
    async fn call(&self, name: String) -> u32;
}

struct CallerImpl {}

#[agent_implementation]
impl Caller for CallerImpl {
    fn new() -> Self {
        Self {}
    }

    async fn call(&self, name: String) -> u32 {
        let mut client = CounterAgentClient::get(name);
        client.increment().await
    }
}
