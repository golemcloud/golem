use golem_rust::agentic::SnapshotRestoreContext;
use golem_rust::{SchemaValue, agent_definition, agent_implementation, endpoint};

#[agent_definition(mount = "/snapshot-counters/{name}")]
pub trait CounterWithSnapshotAgent {
    // The agent constructor, it's parameters identify the agent
    fn new(name: String) -> Self;

    #[endpoint(post = "/increment")]
    fn increment(&mut self) -> u32;
}

struct CounterImpl {
    _name: String,
    count: u32,
}

#[agent_implementation(mount = "/snapshot-counters/{name}")]
impl CounterWithSnapshotAgent for CounterImpl {
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

    async fn load_snapshot(
        bytes: Vec<u8>,
        context: SnapshotRestoreContext,
    ) -> Result<Self, String> {
        let arr: [u8; 4] = bytes
            .try_into()
            .map_err(|_| "Expected a 4-byte long snapshot")?;
        let name = match context.parameters {
            SchemaValue::Record { fields } => match fields.as_slice() {
                [SchemaValue::String(name)] => name.clone(),
                _ => return Err("Expected a string agent name".to_string()),
            },
            _ => return Err("Expected agent parameters to be a record".to_string()),
        };
        Ok(Self {
            _name: name,
            count: u32::from_be_bytes(arr),
        })
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(self.count.to_be_bytes().to_vec())
    }
}
