use golem_rust::agentic::Config;
use golem_rust::{ConfigSchema, agent_definition, agent_implementation, endpoint};
use streaming_tool_guest_client::StreamingClient;

const PAYLOAD: &str = "__SIZE_PAYLOAD__";

#[derive(ConfigSchema)]
pub struct Settings {
    pub label: String,
}

#[agent_definition(mount = "/counters/{name}")]
pub trait CounterAgent {
    fn new(name: String, #[agent_config] config: Config<Settings>) -> Self;
    #[endpoint(get = "/value")]
    fn value(&self) -> u32;
    fn add(&mut self, by: u32) -> u32;
    fn configured(&self) -> String;
    fn payload_length(&self) -> u32;
    fn slice(&self, start: u32, length: u32) -> String;
    fn dependency(&self, start: u32, length: u32) -> String;
    async fn rpc_add(&self, by: u32) -> u32;
    async fn tool(&self) -> String;
}

struct CounterImpl {
    name: String,
    count: u32,
    config: Config<Settings>,
}

#[agent_implementation]
impl CounterAgent for CounterImpl {
    fn new(name: String, #[agent_config] config: Config<Settings>) -> Self {
        Self {
            name,
            count: 0,
            config,
        }
    }

    fn value(&self) -> u32 {
        self.count
    }

    fn add(&mut self, by: u32) -> u32 {
        self.count += by;
        self.count
    }

    fn configured(&self) -> String {
        self.config.get().unwrap().label
    }

    fn payload_length(&self) -> u32 {
        PAYLOAD.len() as u32
    }

    fn slice(&self, start: u32, length: u32) -> String {
        PAYLOAD[start as usize..(start + length) as usize].to_string()
    }

    fn dependency(&self, start: u32, length: u32) -> String {
        hex::encode(&PAYLOAD.as_bytes()[start as usize..(start + length) as usize])
    }

    async fn rpc_add(&self, by: u32) -> u32 {
        SizeHelperClient::upsert(self.name.clone()).add(by).await
    }

    async fn tool(&self) -> String {
        StreamingClient::default()
            .no_stream("size-fixture".to_string())
            .await
            .unwrap()
    }
}

#[agent_definition]
pub trait SizeHelper {
    fn new(name: String) -> Self;
    fn add(&mut self, by: u32) -> u32;
    fn value(&self) -> u32;
}

struct Helper {
    count: u32,
}

#[agent_implementation]
impl SizeHelper for Helper {
    fn new(_name: String) -> Self {
        Self { count: 0 }
    }
    fn add(&mut self, by: u32) -> u32 {
        self.count += by;
        self.count
    }
    fn value(&self) -> u32 {
        self.count
    }
}
