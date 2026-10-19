use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/counters/{name}")]
pub trait CounterAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;

    #[endpoint(get = "/value")]
    fn value(&self) -> u32;

    #[endpoint(post = "/add")]
    fn add(&mut self, by: u32) -> u32;

    #[endpoint(post = "/reset")]
    fn reset(&mut self);
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

    fn value(&self) -> u32 {
        self.count
    }

    fn add(&mut self, by: u32) -> u32 {
        self.count += by;
        self.count
    }

    fn reset(&mut self) {
        self.count = 0;
    }
}
