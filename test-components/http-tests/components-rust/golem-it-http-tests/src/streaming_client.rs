use futures_concurrency::prelude::*;
use golem_rust::{agent_definition, agent_implementation};
use wstd::http::{Client, Request};

#[agent_definition]
pub trait StreamingClient {
    fn new() -> Self;

    async fn streaming_http_read(&self) -> String;
    async fn parallel_streaming_http_reads(&self, n: u64) -> String;
    async fn raw_streaming_http_read(&self) -> String;
    async fn parallel_raw_streaming_http_reads(&self, n: u64) -> String;
    async fn streaming_http_then_sleep(&self) -> String;
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

    /// Mimics the wasm-rquickjs/golem-wasi-http pattern: manually calls
    /// subscribe() + AsyncPollable::wait_for() + read() in a loop, then
    /// drops the stream and body without calling finish(). This is the
    /// pattern used by the production component that triggers the oplog
    /// mismatch bug.
    async fn raw_streaming_http_read(&self) -> String {
        match send_raw_streaming_request().await {
            Ok(s) => s,
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Mimics the production pattern: multiple concurrent raw WASI HTTP
    /// streaming reads running inside wstd's reactor. This exercises
    /// nonblock_check_pollables / block_on_pollables interleaving with
    /// WaitFor::poll ready() calls across concurrent tasks.
    async fn parallel_raw_streaming_http_reads(&self, n: u64) -> String {
        let r1 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_raw_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let r2 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_raw_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let r3 = async {
            let mut result = String::new();
            for _ in 0..n {
                result.push_str(&format!("{:?}\n", send_raw_streaming_request().await));
            }
            Ok::<_, String>(result)
        };
        let timeout = async {
            wstd::task::sleep(wstd::time::Duration::from_secs(60)).await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = ((r1, r2, r3, timeout)).race().await;
        result
    }

    /// Reproducer for the FutureTrailers non-durable bug.
    ///
    /// Uses raw WASI HTTP bindings to read a streaming response, then explicitly
    /// calls incoming_body.finish() → future_trailers.subscribe() → ready() → get().
    async fn streaming_http_then_sleep(&self) -> String {
        // Step 1: raw streaming HTTP read WITH explicit finish()/trailers path
        let body = match send_raw_streaming_request_with_trailers().await {
            Ok(s) => s,
            Err(e) => return format!("Error in streaming read: {e}"),
        };

        // Step 2: sleep — produces additional durable ready/poll entries
        wstd::task::sleep(wstd::time::Duration::from_millis(100)).await;

        format!("body_len={},slept", body.len())
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
            wstd::task::sleep(wstd::time::Duration::from_secs(60)).await;
            Err("Timeout".to_string())
        };
        let (Ok(result) | Err(result)) = ((r1, r2, r3, timeout)).race().await;
        result
    }
}

/// Send a request to the streaming endpoint and read the response body
/// chunk by chunk using wstd APIs.
async fn send_streaming_request() -> Result<String, String> {
    let port = std::env::var("PORT").expect("Requires a PORT env var set");
    let request = Request::get(format!("http://localhost:{port}/streaming-chunks"))
        .body(())
        .map_err(|e| e.to_string())?;

    let mut response = Client::new()
        .send(request)
        .await
        .map_err(|e| e.to_string())?;
    let body = response.body_mut();
    let contents = body.str_contents().await.map_err(|e| e.to_string())?;
    Ok(contents.to_string())
}

/// Mimics the wasm-rquickjs/golem-wasi-http streaming read pattern:
/// Uses raw WASI HTTP + subscribe()/wait_for()/read() in a loop,
/// then drops the stream and body without calling finish().
async fn send_raw_streaming_request() -> Result<String, String> {
    use wasi::http::outgoing_handler;
    use wasi::http::types::{Fields, OutgoingRequest, Scheme};
    use wasi::io::streams::StreamError;

    let port = std::env::var("PORT").expect("Requires a PORT env var set");

    let headers = Fields::new();
    let request = OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Get)
        .map_err(|_| "set_method")?;
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|_| "set_scheme")?;
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .map_err(|_| "set_authority")?;
    request
        .set_path_with_query(Some("/streaming-chunks"))
        .map_err(|_| "set_path")?;

    let future_response =
        outgoing_handler::handle(request, None).map_err(|e| format!("handle error: {e:?}"))?;

    let pollable = future_response.subscribe();
    wstd::runtime::AsyncPollable::new(pollable)
        .wait_for()
        .await;

    let response = future_response
        .get()
        .ok_or("future response not ready")?
        .map_err(|_| "future response error")?
        .map_err(|e| format!("HTTP error: {e:?}"))?;

    let incoming_body = response.consume().map_err(|_| "Failed to consume body")?;
    let stream = incoming_body
        .stream()
        .map_err(|_| "Failed to get stream")?;

    let mut result: Vec<u8> = Vec::new();
    loop {
        let pollable = stream.subscribe();
        wstd::runtime::AsyncPollable::new(pollable)
            .wait_for()
            .await;

        match stream.read(4096) {
            Ok(chunk) => {
                result.extend_from_slice(&chunk);
            }
            Err(StreamError::Closed) => {
                drop(stream);
                drop(incoming_body);
                break;
            }
            Err(StreamError::LastOperationFailed(err)) => {
                return Err(format!("Stream read failed: {}", err.to_debug_string()));
            }
        }
    }

    String::from_utf8(result).map_err(|e| e.to_string())
}

/// Like send_raw_streaming_request but additionally calls finish() → subscribe() →
/// ready() → get() on the FutureTrailers after reading the stream.
async fn send_raw_streaming_request_with_trailers() -> Result<String, String> {
    use wasi::http::outgoing_handler;
    use wasi::http::types::{Fields, IncomingBody, OutgoingRequest, Scheme};
    use wasi::io::streams::StreamError;

    let port = std::env::var("PORT").expect("Requires a PORT env var set");

    let headers = Fields::new();
    let request = OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Get)
        .map_err(|_| "set_method")?;
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|_| "set_scheme")?;
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .map_err(|_| "set_authority")?;
    request
        .set_path_with_query(Some("/streaming-chunks"))
        .map_err(|_| "set_path")?;

    let future_response =
        outgoing_handler::handle(request, None).map_err(|e| format!("handle error: {e:?}"))?;

    let pollable = future_response.subscribe();
    wstd::runtime::AsyncPollable::new(pollable)
        .wait_for()
        .await;

    let response = future_response
        .get()
        .ok_or("future response not ready")?
        .map_err(|_| "future response error")?
        .map_err(|e| format!("HTTP error: {e:?}"))?;

    let incoming_body = response.consume().map_err(|_| "Failed to consume body")?;
    let stream = incoming_body
        .stream()
        .map_err(|_| "Failed to get stream")?;

    let mut result: Vec<u8> = Vec::new();
    loop {
        let pollable = stream.subscribe();
        wstd::runtime::AsyncPollable::new(pollable)
            .wait_for()
            .await;

        match stream.read(4096) {
            Ok(chunk) => {
                result.extend_from_slice(&chunk);
            }
            Err(StreamError::Closed) => {
                drop(stream);
                break;
            }
            Err(StreamError::LastOperationFailed(err)) => {
                return Err(format!("Stream read failed: {}", err.to_debug_string()));
            }
        }
    }

    let future_trailers = IncomingBody::finish(incoming_body);

    loop {
        let pollable = future_trailers.subscribe();
        wstd::runtime::AsyncPollable::new(pollable)
            .wait_for()
            .await;

        if let Some(_trailers_result) = future_trailers.get() {
            break;
        }
    }

    String::from_utf8(result).map_err(|e| e.to_string())
}
