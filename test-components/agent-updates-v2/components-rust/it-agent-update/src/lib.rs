use golem_rust::{agent_definition, agent_implementation, description};

#[agent_definition]
#[description("Counter agent V2")]
pub trait CounterAgent {
    // CHANGE from v1: 'name: String' replaced with a 'name: u64'
    fn new(name: u64) -> Self;

    fn increment(&mut self) -> u32;
    fn decrement(&mut self) -> Option<u32>;
}

struct CounterImpl {
    _id: u64,
    count: u32,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: u64) -> Self {
        Self {
            _id: name,
            count: 0,
        }
    }

    fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    fn decrement(&mut self) -> Option<u32> {
        if self.count > 0 {
            self.count -= 1;
            Some(self.count)
        } else {
            None
        }
    }
}

#[agent_definition]
pub trait NewCaller {
    fn new() -> Self;
    async fn call(&self, name: u64) -> u32;
}

struct NewCallerImpl {}

#[agent_implementation]
impl NewCaller for NewCallerImpl {
    fn new() -> Self {
        Self {}
    }

    async fn call(&self, name: u64) -> u32 {
        let mut client = CounterAgentClient::get(name);
        client.increment().await
    }
}
