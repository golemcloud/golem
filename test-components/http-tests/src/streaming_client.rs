use futures_concurrency::prelude::*;
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait StreamingClient {
    fn new() -> Self;

    async fn streaming_http_read(&self) -> String;
    async fn parallel_streaming_http_reads(&self, n: u64) -> String;
    async fn raw_streaming_http_read(&self) -> String;
    async fn parallel_raw_streaming_http_reads(&self, n: u64) -> String;
    async fn streaming_http_then_sleep(&self) -> String;
    async fn slow_body_stream(&self) -> u64;
    async fn slow_body_stream_with_timeout(&self, timeout_ms: u64) -> Option<u64>;
}

struct StreamingClientImpl;

#[agent_implementation]
impl StreamingClient for StreamingClientImpl {
    fn new() -> Self {
        Self
    }

    async fn streaming_http_read(&self) -> String {
        match send_streaming_request().await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn raw_streaming_http_read(&self) -> String {
        match send_streaming_request().await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn parallel_raw_streaming_http_reads(&self, n: u64) -> String {
        self.parallel_streaming_http_reads(n).await
    }

    async fn streaming_http_then_sleep(&self) -> String {
        let body = match send_streaming_request().await {
            Ok(s) => s,
            Err(e) => return format!("Error in streaming read: {e}"),
        };

        // Sleep — produces additional durable entries after the streaming read
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(100_000_000).await;

        format!("body_len={},slept", body.len())
    }

    async fn slow_body_stream(&self) -> u64 {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        match wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/big-byte-array"))
            .send()
            .await
        {
            Ok(response) => response.into_body().bytes().await.len() as u64,
            Err(err) => {
                println!("Request failed: {:?}", err);
                0
            }
        }
    }

    async fn slow_body_stream_with_timeout(&self, timeout_ms: u64) -> Option<u64> {
        let http_fut = async { Some(self.slow_body_stream().await) };
        let timer_fut = async {
            println!("!!! TIMER STARTED with {timeout_ms}");
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(
                timeout_ms.saturating_mul(1_000_000),
            )
            .await;
            println!("!!! TIMER ELAPSED");
            None
        };

        (http_fut, timer_fut).race().await
    }

    async fn parallel_streaming_http_reads(&self, n: u64) -> String {
        let r1 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let r2 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let r3 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let timeout = async {
            golem_rust::wasip3::clocks::monotonic_clock::wait_for(60_000_000_000).await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = ((r1, r2, r3, timeout)).race().await;
        result
    }
}

/// Send a request to the streaming endpoint and read the response body
/// chunk by chunk using the wasi-fetch async client.
async fn send_streaming_request() -> Result<String, String> {
    let port = std::env::var("PORT").expect("Requires a PORT env var set");

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/streaming-chunks"))
        .send()
        .await
        .map_err(|e| format!("{e:?}"))?;

    let mut stream = response.into_body();
    let mut result: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.chunk().await {
        result.extend_from_slice(&chunk);
    }

    String::from_utf8(result).map_err(|e| e.to_string())
}
