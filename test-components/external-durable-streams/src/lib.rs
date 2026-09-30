use golem_rust::agentic::{AgentStream, Config, Secret};
use golem_rust::durable_streams::{
    Checkpoint, DurableStreamWriter, ExternalDurableStream, ReadOptions, Transport, WriteOptions,
};
use golem_rust::{ConfigSchema, agent_definition, agent_implementation};
use serde_json::value::RawValue;
use std::sync::Arc;

#[agent_definition]
pub trait ExternalDurableStreams {
    fn new(id: String) -> Self;
    async fn consume_json(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
    ) -> Result<Vec<String>, String>;
    async fn consume_json_delayed(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
        delay_ms: u64,
    ) -> Result<Vec<String>, String>;
    async fn consume_bytes(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
    ) -> Result<Vec<u8>, String>;
    async fn append_json(
        &self,
        url: String,
        producer_id: String,
        values: Vec<String>,
        close: bool,
    ) -> Result<Vec<Option<String>>, String>;
    async fn append_bytes(
        &self,
        url: String,
        producer_id: String,
        values: Vec<Vec<u8>>,
        close: bool,
    ) -> Result<Vec<Option<String>>, String>;
    fn output_json(&self, url: String) -> AgentStream<String>;
    fn output_bytes(&self, url: String) -> AgentStream<u8>;
    async fn consume_forwarded_json(
        &self,
        source_id: String,
        url: String,
    ) -> Result<Vec<String>, String>;
    async fn consume_forwarded_bytes(
        &self,
        source_id: String,
        url: String,
    ) -> Result<Vec<u8>, String>;
    async fn concurrent_reads(
        &self,
        url_a: String,
        url_b: String,
    ) -> Result<Vec<Vec<String>>, String>;
    async fn concurrent_appends(
        &self,
        url_a: String,
        url_b: String,
        producer_a: String,
        producer_b: String,
    ) -> Result<Vec<Option<String>>, String>;
    async fn append_non_idempotent(
        &self,
        url: String,
        producer_id: String,
    ) -> Result<Option<String>, String>;
    async fn cancelled_append(
        &self,
        url: String,
        producer_id: String,
        cancel_after_ms: u64,
    ) -> Result<Vec<Option<String>>, String>;
}

struct ExternalDurableStreamsImpl;

#[derive(ConfigSchema)]
pub struct AuthConfig {
    #[config_schema(secret)]
    pub bearer: Secret<String>,
}

#[agent_definition]
pub trait AuthenticatedDurableStreams {
    fn new(id: String, #[agent_config] config: Config<AuthConfig>) -> Self;
    async fn read(&self, url: String) -> Result<Vec<String>, String>;
    async fn append(&self, url: String) -> Result<Option<String>, String>;
}

struct AuthenticatedDurableStreamsImpl {
    auth: Arc<golem_rust::schema::wit::wire::Secret>,
}

#[agent_implementation]
impl AuthenticatedDurableStreams for AuthenticatedDurableStreamsImpl {
    fn new(_id: String, #[agent_config] config: Config<AuthConfig>) -> Self {
        let handle = config
            .get()
            .expect("config access failed")
            .bearer
            .handle()
            .expect("secret capability access failed");
        Self {
            auth: Arc::new(
                handle
                    .take()
                    .expect("secret capability already transferred"),
            ),
        }
    }

    async fn read(&self, url: String) -> Result<Vec<String>, String> {
        consume_json(
            url,
            ReadOptions {
                auth: Some(self.auth.clone()),
                ..ReadOptions::default()
            },
            u32::MAX,
            0,
        )
        .await
    }

    async fn append(&self, url: String) -> Result<Option<String>, String> {
        let mut writer = DurableStreamWriter::new(
            url,
            "application/json",
            WriteOptions {
                auth: Some(self.auth.clone()),
                ..WriteOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        writer
            .append_json(&["authenticated"], true)
            .await
            .map(|r| r.next_offset)
            .map_err(|e| e.to_string())
    }
}

fn options(offset: String, transport: String) -> Result<ReadOptions, String> {
    Ok(ReadOptions {
        checkpoint: Checkpoint {
            offset,
            cursor: None,
        },
        transport: match transport.as_str() {
            "catch-up" => Transport::CatchUp,
            "long-poll" => Transport::LongPoll,
            "sse" => Transport::Sse,
            _ => return Err("unknown transport".into()),
        },
        ..ReadOptions::default()
    })
}

fn writer(
    url: String,
    producer_id: String,
    content_type: &str,
) -> Result<DurableStreamWriter, String> {
    DurableStreamWriter::new(
        url,
        content_type,
        WriteOptions {
            producer_id: Some(producer_id),
            ..WriteOptions::default()
        },
    )
    .map_err(|e| e.to_string())
}

async fn consume_json(
    url: String,
    options: ReadOptions,
    limit: u32,
    delay_ms: u64,
) -> Result<Vec<String>, String> {
    let mut stream = ExternalDurableStream::<Box<RawValue>>::json(url, options);
    let mut values = Vec::new();
    for _ in 0..limit {
        let Some(value) = stream.next().await.map_err(|e| e.to_string())? else {
            break;
        };
        values.push(value.get().to_string());
        if delay_ms != 0 {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                delay_ms.saturating_mul(1_000_000),
            )
            .await;
        }
    }
    Ok(values)
}

#[agent_implementation]
impl ExternalDurableStreams for ExternalDurableStreamsImpl {
    fn new(_id: String) -> Self {
        Self
    }

    async fn consume_json(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
    ) -> Result<Vec<String>, String> {
        consume_json(url, options(offset, transport)?, limit, 0).await
    }

    async fn consume_json_delayed(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
        delay_ms: u64,
    ) -> Result<Vec<String>, String> {
        consume_json(url, options(offset, transport)?, limit, delay_ms).await
    }

    async fn consume_bytes(
        &self,
        url: String,
        offset: String,
        transport: String,
        limit: u32,
    ) -> Result<Vec<u8>, String> {
        let mut stream = ExternalDurableStream::bytes(url, options(offset, transport)?);
        let mut values = Vec::new();
        for _ in 0..limit {
            let Some(value) = stream.next().await.map_err(|e| e.to_string())? else {
                break;
            };
            values.push(value);
        }
        Ok(values)
    }

    async fn append_json(
        &self,
        url: String,
        producer_id: String,
        values: Vec<String>,
        close: bool,
    ) -> Result<Vec<Option<String>>, String> {
        let mut writer = writer(url, producer_id, "application/json")?;
        let mut offsets = Vec::new();
        let count = values.len();
        for (index, value) in values.into_iter().enumerate() {
            let value: Box<RawValue> = serde_json::from_str(&value).map_err(|e| e.to_string())?;
            offsets.push(
                writer
                    .append_json(&[value], close && index + 1 == count)
                    .await
                    .map_err(|e| e.to_string())?
                    .next_offset,
            );
        }
        if count == 0 && close {
            offsets.push(writer.close().await.map_err(|e| e.to_string())?.next_offset);
        }
        Ok(offsets)
    }

    async fn append_bytes(
        &self,
        url: String,
        producer_id: String,
        values: Vec<Vec<u8>>,
        close: bool,
    ) -> Result<Vec<Option<String>>, String> {
        let mut writer = writer(url, producer_id, "application/octet-stream")?;
        let mut offsets = Vec::new();
        let count = values.len();
        for (index, value) in values.into_iter().enumerate() {
            offsets.push(
                writer
                    .append_bytes(&value, close && index + 1 == count)
                    .await
                    .map_err(|e| e.to_string())?
                    .next_offset,
            );
        }
        if count == 0 && close {
            offsets.push(writer.close().await.map_err(|e| e.to_string())?.next_offset);
        }
        Ok(offsets)
    }

    fn output_json(&self, url: String) -> AgentStream<String> {
        ExternalDurableStream::json(url, ReadOptions::default()).into_agent_stream()
    }

    fn output_bytes(&self, url: String) -> AgentStream<u8> {
        ExternalDurableStream::bytes(url, ReadOptions::default()).into_agent_stream()
    }

    async fn consume_forwarded_json(
        &self,
        source_id: String,
        url: String,
    ) -> Result<Vec<String>, String> {
        let source = ExternalDurableStreamsClient::get(source_id);
        source.output_json(url).await.collect().await
    }

    async fn consume_forwarded_bytes(
        &self,
        source_id: String,
        url: String,
    ) -> Result<Vec<u8>, String> {
        let source = ExternalDurableStreamsClient::get(source_id);
        source.output_bytes(url).await.collect().await
    }

    async fn concurrent_reads(
        &self,
        url_a: String,
        url_b: String,
    ) -> Result<Vec<Vec<String>>, String> {
        let (a, b) = futures::join!(
            consume_json(url_a, ReadOptions::default(), 1, 0),
            consume_json(url_b, ReadOptions::default(), 1, 0),
        );
        Ok(vec![a?, b?])
    }

    async fn concurrent_appends(
        &self,
        url_a: String,
        url_b: String,
        producer_a: String,
        producer_b: String,
    ) -> Result<Vec<Option<String>>, String> {
        let mut a = writer(url_a, producer_a, "application/json")?;
        let mut b = writer(url_b, producer_b, "application/json")?;
        let (a, b) = futures::join!(
            a.append_json(&["left"], true),
            b.append_json(&["right"], true)
        );
        Ok(vec![
            a.map_err(|e| e.to_string())?.next_offset,
            b.map_err(|e| e.to_string())?.next_offset,
        ])
    }

    async fn append_non_idempotent(
        &self,
        url: String,
        producer_id: String,
    ) -> Result<Option<String>, String> {
        let mut writer = writer(url, producer_id, "application/json")?;
        let _guard = golem_rust::use_idempotence_mode(false);
        writer
            .append_json(&["non-idempotent"], true)
            .await
            .map(|r| r.next_offset)
            .map_err(|e| e.to_string())
    }

    async fn cancelled_append(
        &self,
        url: String,
        producer_id: String,
        cancel_after_ms: u64,
    ) -> Result<Vec<Option<String>>, String> {
        let mut writer = writer(url, producer_id, "application/json")?;
        let result = {
            let append = Box::pin(writer.append_json(&["original"], false));
            let timer = Box::pin(golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                cancel_after_ms.saturating_mul(1_000_000),
            ));
            match futures::future::select(append, timer).await {
                futures::future::Either::Left(_) => {
                    return Err("append completed before cancellation".into());
                }
                futures::future::Either::Right(((), pending)) => {
                    drop(pending);
                    "cancelled".to_string()
                }
            }
        };
        let rejected = writer
            .append_json(&["different"], false)
            .await
            .err()
            .ok_or("pending append was replaced")?;
        let original = writer.retry_pending().await.map_err(|e| e.to_string())?;
        let next = writer
            .append_json(&["different"], true)
            .await
            .map_err(|e| e.to_string())?;
        Ok(vec![
            Some(result),
            Some(format!("{:?}", rejected.kind)),
            original.next_offset,
            next.next_offset,
        ])
    }
}
