use golem_rust::agentic::{AgentStream, spawn_local};
use golem_rust::schema::{FromSchema, IntoSchema};
use golem_rust::{agent_definition, agent_implementation, endpoint};

#[derive(IntoSchema, FromSchema)]
pub struct EchoOutput {
    pub output: AgentStream<String>,
}

#[agent_definition(mount = "/durable-stream-agents/{id}")]
pub trait DurableStreamAgent {
    fn new(id: String) -> Self;

    #[endpoint(put = "/json/{count}?delay_ms={delay_ms}")]
    fn json(&self, count: u32, delay_ms: u64) -> AgentStream<String>;

    #[endpoint(put = "/bytes/{count}?delay_ms={delay_ms}")]
    fn bytes(&self, count: u32, delay_ms: u64) -> AgentStream<u8>;

    #[endpoint(put = "/delayed-output/{handle_delay_ms}?delay_ms={delay_ms}")]
    async fn delayed_output(&self, handle_delay_ms: u64, delay_ms: u64) -> AgentStream<String>;

    #[endpoint(put = "/echo")]
    fn echo(&self, input: AgentStream<String>) -> EchoOutput;

    #[endpoint(put = "/shadow/{output}")]
    fn shadow(&self, output: String) -> EchoOutput;

    #[endpoint(put = "/bound/{path}?count={count}", headers("x-label" = "label"))]
    fn bound(
        &self,
        message: String,
        count: u32,
        label: String,
        path: String,
    ) -> AgentStream<String>;

    #[endpoint(put = "/isolated-a/{count}")]
    fn isolated_a(&self, count: u32) -> AgentStream<String>;

    #[endpoint(put = "/isolated-b/{count}")]
    fn isolated_b(&self, count: u32) -> AgentStream<String>;
}

struct DurableStreamAgentImpl;

#[agent_implementation]
impl DurableStreamAgent for DurableStreamAgentImpl {
    fn new(_id: String) -> Self {
        Self
    }

    fn json(&self, count: u32, delay_ms: u64) -> AgentStream<String> {
        stream_with_delay((0..count).map(|i| format!("message-{i:04}")), delay_ms)
    }

    fn bytes(&self, count: u32, delay_ms: u64) -> AgentStream<u8> {
        stream_with_delay((0..count).map(|i| (i % 251) as u8), delay_ms)
    }

    async fn delayed_output(&self, handle_delay_ms: u64, delay_ms: u64) -> AgentStream<String> {
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(
            handle_delay_ms.saturating_mul(1_000_000),
        )
        .await;
        let (mut writer, stream) = AgentStream::new();
        spawn_local(async move {
            // Keep the producer active while testing the reader's wait deadline.
            let mut remaining = delay_ms;
            while remaining > 0 {
                let step = remaining.min(100);
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(step * 1_000_000).await;
                remaining -= step;
            }
            let _ = writer.write_one("delayed-value".to_string()).await;
        });
        stream
    }

    fn echo(&self, mut input: AgentStream<String>) -> EchoOutput {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Ok(Some(value)) = input.next().await {
                if writer.write_one(value).await.is_err() {
                    break;
                }
            }
        });
        EchoOutput { output }
    }

    fn shadow(&self, output: String) -> EchoOutput {
        EchoOutput {
            output: stream_with_delay([output], 0),
        }
    }

    fn bound(
        &self,
        message: String,
        count: u32,
        label: String,
        path: String,
    ) -> AgentStream<String> {
        stream_with_delay(
            (0..count).map(|i| format!("{path}|{label}|{i}|{message}")),
            0,
        )
    }

    fn isolated_a(&self, count: u32) -> AgentStream<String> {
        stream_with_delay((0..count).map(|i| format!("a-{i}")), 0)
    }

    fn isolated_b(&self, count: u32) -> AgentStream<String> {
        stream_with_delay((0..count).map(|i| format!("b-{i}")), 0)
    }
}

fn stream_with_delay<T: golem_rust::schema::IntoSchema + 'static>(
    values: impl IntoIterator<Item = T>,
    delay_ms: u64,
) -> AgentStream<T> {
    let values = values.into_iter().collect::<Vec<_>>();
    let (mut writer, stream) = AgentStream::new();
    spawn_local(async move {
        for value in values {
            if delay_ms != 0 {
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                    delay_ms.saturating_mul(1_000_000),
                )
                .await;
            }
            if writer.write_one(value).await.is_err() {
                break;
            }
        }
    });
    stream
}

#[agent_definition(ephemeral, mount = "/ephemeral-stream-agents")]
pub trait EphemeralStreamAgent {
    fn new() -> Self;

    #[endpoint(put = "/identity/{label}")]
    fn identity(&self, label: String) -> AgentStream<String>;
}

struct EphemeralStreamAgentImpl;

#[agent_implementation]
impl EphemeralStreamAgent for EphemeralStreamAgentImpl {
    fn new() -> Self {
        Self
    }

    fn identity(&self, label: String) -> AgentStream<String> {
        stream_with_delay([label, golem_rust::agentic::get_agent_id().agent_id], 0)
    }
}

#[agent_definition(mount = "/phantom-stream-agents", phantom_agent = true)]
pub trait PhantomStreamAgent {
    fn new() -> Self;

    #[endpoint(put = "/identity/{label}")]
    fn identity(&self, label: String) -> AgentStream<String>;
}

struct PhantomStreamAgentImpl;

#[agent_implementation]
impl PhantomStreamAgent for PhantomStreamAgentImpl {
    fn new() -> Self {
        Self
    }

    fn identity(&self, label: String) -> AgentStream<String> {
        stream_with_delay([label, golem_rust::agentic::get_agent_id().agent_id], 0)
    }
}
