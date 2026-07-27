use std::time::Duration;

/// HTTP methods used by the test helpers.
#[derive(Debug, Clone, Copy)]
pub enum Method {
    Get,
    Post,
    Delete,
}

/// Sends an outgoing HTTP request through the P3 `wasi:http` based `wasi-fetch`
/// client and waits for the complete response, returning its status and body
/// bytes.
///
/// This path is used by the host-API tests: the requests are recorded durably
/// in the oplog and the worker executor injects the Golem invocation-context
/// trace headers.
pub async fn request_async(
    method: Method,
    authority: &str,
    path: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
) -> (u16, Vec<u8>) {
    let url = format!("http://{authority}{path}");
    let client = wasi_fetch::Client::new();
    let mut builder = match method {
        Method::Get => client.get(&url),
        Method::Post => client.post(&url),
        Method::Delete => client.delete(&url),
    }
    .timeout(Duration::from_secs(5))
    .between_bytes_timeout(Duration::from_secs(5));

    if let Some(content_type) = content_type {
        builder = builder.header("content-type", content_type);
    }
    if let Some(bytes) = body {
        builder = builder.body(bytes.to_vec());
    }

    let response = builder.send().await.expect("HTTP request failed");
    let status = response.status().as_u16();
    let body = response.into_body().bytes().await;
    (status, body.to_vec())
}

/// Blocking wrapper around [`request_async`] for synchronous agent methods.
pub fn request(
    method: Method,
    authority: &str,
    path: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
) -> (u16, Vec<u8>) {
    wit_bindgen::block_on(request_async(method, authority, path, body, content_type))
}
