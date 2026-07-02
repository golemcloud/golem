use golem_rust::retry::{NamedPolicy, Policy, Predicate, Props, with_named_policy_async};
use golem_rust::{agent_definition, agent_implementation, with_idempotence_mode_async};

#[agent_definition]
pub trait HttpClient4 {
    fn new() -> Self;

    /// Sends a POST request with assume_idempotence=false.
    async fn post_non_idempotent(&self) -> String;

    /// Sends a GET request with assume_idempotence=false.
    async fn get_idempotent(&self) -> String;

    /// Sends a POST request with a large multi-chunk body.
    async fn post_large_body(&self) -> String;

    /// Sends a GET request and reads the response body in chunks.
    async fn get_and_read_body_chunked(&self) -> String;

    /// Sends a POST with a body composed of: 4 bytes "HEAD", then 1024 zero bytes,
    /// then 1024 bytes of 0xAB.
    async fn post_with_write_zeroes(&self) -> String;

    /// Sends a POST with a large body.
    async fn post_with_subscribe(&self) -> String;

    /// Sends a POST request and finishes the body with trailers.
    async fn post_with_trailers(&self) -> String;

    /// Sends a GET, then reads the response body.
    async fn get_with_body_skip(&self) -> String;

    /// Sends a buffered POST with a retry policy that retries HTTP 500 responses.
    async fn post_with_status_retry_policy(&self) -> String;
}

struct HttpClient4Impl;

#[agent_implementation]
impl HttpClient4 for HttpClient4Impl {
    fn new() -> Self {
        Self
    }

    async fn post_non_idempotent(&self) -> String {
        with_idempotence_mode_async(false, || do_post_request()).await
    }

    async fn get_idempotent(&self) -> String {
        with_idempotence_mode_async(false, || do_get_request()).await
    }

    async fn post_large_body(&self) -> String {
        do_post_body(vec![0xABu8; 4 * 64 * 1024]).await
    }

    async fn get_and_read_body_chunked(&self) -> String {
        do_get_chunked_read().await
    }

    async fn post_with_write_zeroes(&self) -> String {
        let mut body = Vec::new();
        body.extend_from_slice(b"HEAD");
        body.extend_from_slice(&[0u8; 1024]);
        body.extend_from_slice(&[0xABu8; 1024]);
        do_post_body(body).await
    }

    async fn post_with_subscribe(&self) -> String {
        do_post_body(vec![0xABu8; 4 * 64 * 1024]).await
    }

    async fn post_with_trailers(&self) -> String {
        do_post_body(b"test-body".to_vec()).await
    }

    async fn get_with_body_skip(&self) -> String {
        do_get_request().await
    }

    async fn post_with_status_retry_policy(&self) -> String {
        let policy = NamedPolicy::named(
            "http-status-retry-test",
            Policy::immediate().max_retries(10),
        )
        .applies_when(Predicate::eq(Props::STATUS_CODE, 500u16));

        with_named_policy_async(&policy, || async {
            let port = std::env::var("PORT").unwrap_or("9999".to_string());
            let response = wasi_fetch::Client::new()
                .post(&format!("http://localhost:{port}/"))
                .body("test-body")
                .send()
                .await
                .expect("Request failed");
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .text()
                .await
                .expect("Response body read failed");
            format!("{status} {body}")
        })
        .await
        .unwrap()
    }
}

async fn do_post_request() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .post(&format!("http://localhost:{port}/"))
        .header("X-Test", "test-header")
        .body("test-body")
        .send()
        .await
        .expect("Request failed");

    let status = response.status().as_u16();
    let body = response
        .into_body()
        .text()
        .await
        .expect("Response body read failed");
    format!("{status} {body}")
}

async fn do_get_request() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/"))
        .send()
        .await
        .expect("Request failed");

    let status = response.status().as_u16();
    let body = response
        .into_body()
        .text()
        .await
        .expect("Response body read failed");
    format!("{status} {body}")
}

async fn do_post_body(body: Vec<u8>) -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .post(&format!("http://localhost:{port}/"))
        .header("Content-Type", "application/octet-stream")
        .body(body)
        .send()
        .await
        .expect("Request failed");

    let status = response.status().as_u16();
    let body = response.into_body().bytes().await;
    format!("{status} {}", String::from_utf8_lossy(&body))
}

async fn do_get_chunked_read() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/"))
        .send()
        .await
        .expect("Request failed");

    let status = response.status().as_u16();
    let mut stream = response.into_body();
    let mut body = Vec::new();
    while let Some(chunk) = stream.chunk().await {
        body.extend_from_slice(&chunk);
    }
    format!("{status} {}", String::from_utf8_lossy(&body))
}
