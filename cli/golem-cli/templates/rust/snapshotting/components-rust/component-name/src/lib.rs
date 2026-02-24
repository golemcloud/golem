use golem_rust::{agent_definition, agent_implementation, endpoint};

#[agent_definition]
pub trait CounterAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,                                                                                                                                                                                                        }

#[agent_implementation(
    mount = "/counters/{name}"
)]
impl CounterAgent for CounterImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            count: 0,
        }
    }

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32 {
        self.count += 1;
        log::info!("The new value is {}", self.count);
        self.count
    }

    async fn load_snapshot(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let arr: [u8; 4] = bytes
            .try_into()
            .map_err(|_| "Expected a 4-byte long snapshot")?;
        self.count = u32::from_be_bytes(arr);
        Ok(())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_be_bytes().to_vec())
    }
}
