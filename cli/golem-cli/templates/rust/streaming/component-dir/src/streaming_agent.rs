use golem_rust::agentic::{spawn_local, AgentStream, Config, Secret};
use golem_rust::durable_streams::{
    DurableStreamWriter, ExternalDurableStream, ReadOptions, WriteOptions,
};
use golem_rust::{agent_definition, agent_implementation, endpoint, ConfigSchema};
use std::sync::Arc;

#[derive(ConfigSchema)]
pub struct StreamingConfig {
    #[config_schema(secret)]
    pub external_auth: Secret<String>,
}

#[agent_definition(mount = "/durable-stream-agents/{name}")]
pub trait StreamingAgent {
    fn new(name: String, #[agent_config] config: Config<StreamingConfig>) -> Self;

    async fn sum(&self, input: AgentStream<u32>) -> u32;

    fn produce(&self) -> AgentStream<u32>;

    fn transform(&self, prefix: String, input: AgentStream<u32>) -> AgentStream<String>;

    fn siblings(&self) -> Vec<AgentStream<u32>>;

    fn recoverable(&self) -> AgentStream<Result<u32, String>>;

    fn status(&self) -> String;

    #[endpoint(
        put = "/echo",
        durable_streams(
            input("input"),
            output("$result"),
            allow_external_writes = true,
        )
    )]
    fn durable_echo(&self, input: AgentStream<String>) -> AgentStream<String>;

    async fn append_external(
        &self,
        url: String,
        producer_id: String,
        values: Vec<String>,
        close: bool,
    ) -> Option<String>;

    async fn read_external(&self, url: String) -> Vec<String>;
}

struct StreamingAgentImpl {
    auth: Arc<golem_rust::schema::wit::wire::Secret>,
}

fn stream<T>(values: Vec<T>) -> AgentStream<T>
where
    T: golem_rust::IntoWire + golem_rust::FromWire + 'static,
{
    let (mut writer, stream) = AgentStream::new();
    spawn_local(async move {
        for value in values {
            // Dropping the reader makes the next write fail, which is how a
            // producer observes consumer cancellation.
            if writer.write_one(value).await.is_err() {
                break;
            }
        }
    });
    stream
}

#[agent_implementation]
impl StreamingAgent for StreamingAgentImpl {
    fn new(_name: String, #[agent_config] config: Config<StreamingConfig>) -> Self {
        let handle = config
            .get()
            .expect("config access failed")
            .external_auth
            .handle()
            .expect("secret capability access failed");
        Self {
            auth: Arc::new(handle.take().expect("secret capability already transferred")),
        }
    }

    async fn sum(&self, input: AgentStream<u32>) -> u32 {
        input
            .collect()
            .await
            .expect("the input stream should close normally")
            .into_iter()
            .sum()
    }

    fn produce(&self) -> AgentStream<u32> {
        stream(vec![1, 2, 3])
    }

    fn transform(&self, prefix: String, mut input: AgentStream<u32>) -> AgentStream<String> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            while let Some(value) = input
                .next()
                .await
                .expect("failed to read transform input stream")
            {
                if writer
                    .write_one(format!("{prefix}:{value}"))
                    .await
                    .is_err()
                {
                    // The output consumer stopped early; dropping `input`
                    // propagates cancellation to its producer.
                    break;
                }
            }
        });
        output
    }

    fn siblings(&self) -> Vec<AgentStream<u32>> {
        vec![stream(vec![10, 20]), stream(vec![30, 40])]
    }

    fn recoverable(&self) -> AgentStream<Result<u32, String>> {
        stream(vec![
            Ok(1),
            Err("this item could not be produced".to_string()),
            Ok(2),
        ])
    }

    fn status(&self) -> String {
        "ready".to_string()
    }

    fn durable_echo(&self, input: AgentStream<String>) -> AgentStream<String> {
        let (mut writer, output) = AgentStream::new();
        spawn_local(async move {
            let mut input = input;
            while let Some(value) = input.next().await.expect("failed to read input") {
                if writer.write_one(format!("echo:{value}")).await.is_err() {
                    break;
                }
            }
        });
        output
    }

    async fn append_external(
        &self,
        url: String,
        producer_id: String,
        values: Vec<String>,
        close: bool,
    ) -> Option<String> {
        let mut writer = DurableStreamWriter::new(
            url,
            "application/json",
            WriteOptions {
                producer_id: Some(producer_id),
                auth: Some(self.auth.clone()),
                ..WriteOptions::default()
            },
        )
        .expect("failed to create external Durable Stream writer");
        writer
            .append_json(&values, close)
            .await
            .map(|receipt| receipt.next_offset)
            .expect("failed to append external Durable Stream")
    }

    async fn read_external(&self, url: String) -> Vec<String> {
        let mut stream = ExternalDurableStream::<String>::json(
            url,
            ReadOptions {
                auth: Some(self.auth.clone()),
                ..ReadOptions::default()
            },
        );
        let mut values = Vec::new();
        while let Some(value) = stream.next().await.expect("failed to read external Durable Stream") {
            values.push(value);
        }
        values
    }
}
