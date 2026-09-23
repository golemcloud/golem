use golem_rust::agentic::{spawn_local, AgentStream};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait StreamingAgent {
    fn new(name: String) -> Self;

    async fn sum(&self, input: AgentStream<u32>) -> u32;

    fn produce(&self) -> AgentStream<u32>;

    fn transform(&self, prefix: String, input: AgentStream<u32>) -> AgentStream<String>;

    fn siblings(&self) -> Vec<AgentStream<u32>>;

    fn recoverable(&self) -> AgentStream<Result<u32, String>>;

    fn status(&self) -> String;
}

struct StreamingAgentImpl;

fn stream<T>(values: Vec<T>) -> AgentStream<T>
where
    T: golem_rust::IntoSchema + golem_rust::FromSchema + 'static,
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
    fn new(_name: String) -> Self {
        Self
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
}
