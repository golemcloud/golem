// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use tracing::Instrument;

/// Parses the `Content-Length` header from raw HTTP request header text.
/// Returns `Some(0)` if the header is present but malformed, mirroring the
/// previous inline `unwrap_or(0)` behaviour, and `None` if it is absent.
fn parse_content_length(headers: &str) -> Option<usize> {
    headers
        .lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .map(|cl_line| {
            cl_line
                .split(':')
                .nth(1)
                .unwrap()
                .trim()
                .parse()
                .unwrap_or(0)
        })
}

/// Starts a raw TCP server that drops the first `fail_count` connections (producing
/// ConnectionTerminated errors), then serves a valid HTTP 200 response on subsequent
/// connections.
///
/// On the success path the server reads the full HTTP request before responding.
/// This avoids a race in hyper's HTTP/1 client dispatcher where a response that
/// arrives before `sendRequest` registers the callback is rejected with
/// `Canceled(UnexpectedMessage)`, causing spurious extra retries on busy CI
/// machines.
///
/// Returns `(port, connection_counter)`.
pub(crate) async fn start_failing_http_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    // Immediately close the connection — produces ConnectionTerminated
                    drop(stream);
                } else {
                    // Read the full HTTP request before responding to avoid a
                    // hyper dispatcher race (see doc comment above).
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl) = parse_content_length(headers) {
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }

                    let body = "response is test-header test-body";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

pub(crate) async fn start_status_code_retry_http_server(
    fail_count: usize,
) -> (u16, Arc<AtomicUsize>, Arc<Mutex<Vec<Option<String>>>>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let idempotency_keys = Arc::new(Mutex::new(Vec::new()));
    let idempotency_keys_clone = idempotency_keys.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };

                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                        Err(_) => break,
                    }
                    if String::from_utf8_lossy(&data).contains("\r\n\r\n") {
                        break;
                    }
                }

                let header_end = data
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                    .unwrap_or(data.len());
                let header_text = String::from_utf8_lossy(&data[..header_end]);
                let idempotency_key = header_text.lines().find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        if name.eq_ignore_ascii_case("idempotency-key") {
                            Some(value.trim().to_string())
                        } else {
                            None
                        }
                    })
                });
                idempotency_keys_clone
                    .lock()
                    .unwrap()
                    .push(idempotency_key);
                let content_length = header_text
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("Content-Length:")
                            .or_else(|| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);

                let attempt = counter_clone.fetch_add(1, Ordering::SeqCst) + 1;
                let (status, reason, body) = if attempt <= fail_count {
                    (500, "Internal Server Error", "retry-me")
                } else {
                    (200, "OK", "status-retry-ok")
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = stream.write_all(response.as_bytes()).await;

                let body_start = header_end + 4;
                let mut body_bytes_read = data.len().saturating_sub(body_start);
                while body_bytes_read < content_length {
                    match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => body_bytes_read += n,
                        Err(_) => break,
                    }
                }

                let _ = stream.shutdown().await;
            }
        }
        .in_current_span(),
    );

    (port, counter, idempotency_keys)
}

pub(crate) async fn start_body_dropping_http_server(fail_count: usize) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    // Read a small amount (HTTP headers) then drop,
                    // forcing the client's body write to fail.
                    let mut buf = [0u8; 512];
                    let _ = stream.read(&mut buf).await;
                    drop(stream);
                } else {
                    // Read the full request (headers + body), then respond
                    let mut data = Vec::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        // Check if we've received the end of the HTTP body.
                        // For simplicity, look for Content-Length and verify.
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl) = parse_content_length(headers) {
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked request bodies end with a final zero-size chunk and a
                                // blank line. Accept optional trailers by checking for terminal
                                // "\r\n\r\n" in the body section.
                                let body_data = &data_str[header_end + 4..];
                                if body_data.ends_with("\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let body = format!("received {} bytes", data.len());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

pub(crate) async fn read_http_request_body_len(stream: &mut tokio::net::TcpStream) -> usize {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }

        let data_str = String::from_utf8_lossy(&data);
        if let Some(header_end) = data_str.find("\r\n\r\n") {
            let headers = &data_str[..header_end];
            let body_start = header_end + 4;
            if let Some(content_length) = parse_content_length(headers) {
                if data.len() >= body_start + content_length {
                    return content_length;
                }
            } else if headers
                .lines()
                .any(|line| line.to_lowercase().contains("transfer-encoding: chunked"))
            {
                let body_data = &data[body_start..];
                if body_data.ends_with(b"\r\n\r\n") {
                    return decoded_chunked_body_len(body_data);
                }
            }
        }
    }

    let data_str = String::from_utf8_lossy(&data);
    data_str
        .find("\r\n\r\n")
        .map(|header_end| data.len().saturating_sub(header_end + 4))
        .unwrap_or(0)
}

fn decoded_chunked_body_len(mut body: &[u8]) -> usize {
    let mut result = 0;
    loop {
        let Some(line_end) = body.windows(2).position(|window| window == b"\r\n") else {
            return result;
        };
        let size_line = String::from_utf8_lossy(&body[..line_end]);
        let size_hex = size_line.split(';').next().unwrap_or("0").trim();
        let size = usize::from_str_radix(size_hex, 16).unwrap_or(0);
        body = &body[line_end + 2..];
        if size == 0 {
            return result;
        }
        if body.len() < size + 2 {
            return result;
        }
        result += size;
        body = &body[size + 2..];
    }
}

/// Drives two different inline retry phases for a streaming POST body:
///
/// 1. The first connection is closed after one 64KiB chunk has been received,
///    so the next body write fails and output-stream inline retry rebuilds the
///    request.
/// 2. The second connection receives the full rebuilt body and then closes
///    before sending any response, so `FutureIncomingResponse::get()` performs
///    awaiting-response inline retry.
/// 3. The third connection must receive the full body again. Before the fix,
///    the awaiting-response retry reconstructed only chunks after the previous
///    retry error and resent a suffix of the body.
pub(crate) async fn start_body_retry_then_response_retry_http_server()
-> (u16, Arc<Mutex<Vec<usize>>>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body_lengths = Arc::new(Mutex::new(Vec::new()));
    let body_lengths_clone = body_lengths.clone();

    spawn(
        async move {
            let mut attempt = 0usize;
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                attempt += 1;

                match attempt {
                    1 => {
                        let mut data = Vec::new();
                        let mut buf = [0u8; 8192];
                        let mut body_start = None;
                        while data.len().saturating_sub(body_start.unwrap_or(data.len()))
                            < 64 * 1024
                        {
                            match stream.read(&mut buf).await {
                                Ok(0) => break,
                                Ok(n) => {
                                    data.extend_from_slice(&buf[..n]);
                                    if body_start.is_none()
                                        && let Some(header_end) =
                                            String::from_utf8_lossy(&data).find("\r\n\r\n")
                                    {
                                        body_start = Some(header_end + 4);
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                        body_lengths_clone.lock().unwrap().push(
                            body_start
                                .map(|start| data.len().saturating_sub(start))
                                .unwrap_or(0),
                        );
                        drop(stream);
                    }
                    2 => {
                        let body_len = read_http_request_body_len(&mut stream).await;
                        body_lengths_clone.lock().unwrap().push(body_len);
                        drop(stream);
                    }
                    _ => {
                        let body_len = read_http_request_body_len(&mut stream).await;
                        body_lengths_clone.lock().unwrap().push(body_len);
                        let body = format!("received {body_len} body bytes");
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body,
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    }
                }
            }
        }
        .in_current_span(),
    );

    (port, body_lengths)
}

/// Starts a raw TCP server that responds to both GET and POST requests.
/// The first `fail_count` connections are dropped immediately.
/// Subsequent connections get a valid HTTP 200 response.
/// Returns `(port, connection_counter)`.
pub(crate) async fn start_failing_http_server_any_method(
    fail_count: usize,
) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                if n < fail_count {
                    drop(stream);
                } else {
                    // Read the full request
                    let mut data = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            // For GET requests (no body), we can respond immediately
                            if headers.starts_with("GET ") {
                                break;
                            }
                            // For POST, check Content-Length or Transfer-Encoding
                            if let Some(cl) = parse_content_length(headers) {
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            } else if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked encoding: a chunked message always ends
                                // with "0\r\n" (final chunk) + optional trailers
                                // + "\r\n" (blank line). So the body is complete
                                // when it ends with "\r\n\r\n" (either "0\r\n\r\n"
                                // for no trailers, or "trailer: val\r\n\r\n").
                                let body_data = &data_str[header_end + 4..];
                                if body_data.ends_with("\r\n\r\n") {
                                    break;
                                }
                            } else {
                                // No Content-Length or chunked encoding, assume no body
                                break;
                            }
                        }
                    }
                    let body = "response ok";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}

/// Starts a TCP server that sends partial responses, then supports Range-based resume.
/// First `fail_count` connections: sends `initial_status` headers + `prefix_len` bytes then drops.
/// Subsequent connections: if `resume_supports_range` is true and a Range header is present,
/// responds 206 with remaining bytes; otherwise responds with `resume_status` and the full body.
/// The body is `body_size` bytes of sequential values (i % 256).
/// Returns `(port, connection_counter, range_counter)`.
pub(crate) async fn start_partial_response_http_server(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
) -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let range_counter = Arc::new(AtomicUsize::new(0));
    let range_counter_clone = range_counter.clone();

    // Generate the full body (deterministic pattern)
    let full_body: Vec<u8> = (0..body_size).map(|i| (i % 256) as u8).collect();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);
                let full_body = full_body.clone();

                if n < fail_count {
                    // Read request headers first to make failure timing deterministic.
                    let mut req_buf = [0u8; 4096];
                    let mut req_header_data = Vec::new();
                    loop {
                        match stream.read(&mut req_buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                req_header_data.extend_from_slice(&req_buf[..n]);
                                if req_header_data.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }

                    // Send headers + partial body, then drop
                    let initial_reason = match initial_status {
                        200 => "OK",
                        201 => "Created",
                        _ => panic!("unsupported initial status: {initial_status}"),
                    };
                    let headers = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        initial_status,
                        initial_reason,
                        body_size,
                    );
                    let _ = stream.write_all(headers.as_bytes()).await;
                    let _ = stream.write_all(&full_body[..prefix_len]).await;
                    let _ = stream.flush().await;
                    // Wait for the client to receive the partial data before dropping
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(stream);
                } else {
                    // Read request headers to check for Range
                    let mut buf = [0u8; 4096];
                    let mut header_data = Vec::new();
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => {
                                header_data.extend_from_slice(&buf[..n]);
                                if header_data.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    let header_str = String::from_utf8_lossy(&header_data);

                    // Parse Range header
                    let range_start = header_str.lines().find_map(|line| {
                        if line.to_lowercase().starts_with("range:") {
                            // Parse "Range: bytes=N-"
                            let val = line.split(':').nth(1)?.trim();
                            let rest = val.strip_prefix("bytes=")?;
                            let dash_pos = rest.find('-')?;
                            rest[..dash_pos].parse::<usize>().ok()
                        } else {
                            None
                        }
                    });

                    if range_start.is_some() {
                        range_counter_clone.fetch_add(1, Ordering::SeqCst);
                    }

                    if resume_supports_range && let Some(start) = range_start {
                        if start <= body_size {
                            // 206 Partial Content
                            let remaining = &full_body[start..];
                            let content_range =
                                format!("bytes {}-{}/{}", start, body_size - 1, body_size);
                            let response = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                content_range,
                                remaining.len(),
                            );
                            let _ = stream.write_all(response.as_bytes()).await;
                            let _ = stream.write_all(remaining).await;
                        } else {
                            // Invalid range
                            let response = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            let _ = stream.write_all(response.as_bytes()).await;
                        }
                    } else {
                        // Full body response (for response-body resumption
                        // matching-status skip path)
                        let resume_reason = match resume_status {
                            200 => "OK",
                            201 => "Created",
                            _ => panic!("unsupported resume status: {resume_status}"),
                        };
                        let response = format!(
                            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            resume_status,
                            resume_reason,
                            body_size,
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.write_all(&full_body).await;
                    }
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter, range_counter)
}

/// Decodes an HTTP chunked transfer-encoded body into raw bytes.
pub(crate) fn decode_chunked_body(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        // Find the end of the chunk size line
        let crlf = data[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p);
        let crlf = match crlf {
            Some(p) => p,
            None => break,
        };
        let size_str = String::from_utf8_lossy(&data[pos..crlf]);
        let chunk_size = match usize::from_str_radix(size_str.trim(), 16) {
            Ok(s) => s,
            Err(_) => break,
        };
        if chunk_size == 0 {
            break; // Terminal chunk
        }
        let chunk_start = crlf + 2;
        let chunk_end = chunk_start + chunk_size;
        if chunk_end > data.len() {
            // Incomplete chunk — take what we have
            result.extend_from_slice(&data[chunk_start..]);
            break;
        }
        result.extend_from_slice(&data[chunk_start..chunk_end]);
        pos = chunk_end + 2; // Skip trailing \r\n after chunk data
    }
    result
}

/// Starts a TCP server for testing write_zeroes body reconstruction.
/// First `fail_count` connections: reads some data then drops (simulates body write failure).
/// Subsequent connections: reads full request body and responds with a validation summary.
/// Returns `(port, connection_counter)`.
pub(crate) async fn start_write_zeroes_validation_server(
    fail_count: usize,
) -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(conn) => conn,
                    Err(_) => break,
                };
                let n = counter_clone.fetch_add(1, Ordering::SeqCst);

                if n < fail_count {
                    // Read a small amount then drop
                    let mut buf = [0u8; 512];
                    let _ = stream.read(&mut buf).await;
                    drop(stream);
                } else {
                    // Read the full request, handling both content-length and
                    // chunked transfer encoding (used by streaming bodies).
                    let mut data = Vec::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) => break,
                            Ok(n) => data.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                        let data_str = String::from_utf8_lossy(&data);
                        if let Some(header_end) = data_str.find("\r\n\r\n") {
                            let headers = &data_str[..header_end];
                            if let Some(cl) = parse_content_length(headers) {
                                let body_start = header_end + 4;
                                if data.len() >= body_start + cl {
                                    break;
                                }
                            }
                            // Check for chunked transfer encoding terminator
                            if headers
                                .lines()
                                .any(|l| l.to_lowercase().contains("transfer-encoding: chunked"))
                            {
                                // Chunked encoding ends with "0\r\n\r\n"
                                if data.ends_with(b"0\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }

                    // Extract body — decode chunked encoding if needed
                    let header_end_pos = String::from_utf8_lossy(&data)
                        .find("\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(data.len());
                    let headers_str = String::from_utf8_lossy(&data[..header_end_pos]);
                    let is_chunked = headers_str
                        .to_lowercase()
                        .contains("transfer-encoding: chunked");
                    let raw_body = &data[header_end_pos..];
                    let request_body: Vec<u8> = if is_chunked {
                        decode_chunked_body(raw_body)
                    } else {
                        raw_body.to_vec()
                    };
                    let request_body = &request_body[..];

                    // Validate: "HEAD" + 1024 zeroes + 1024 * 0xAB
                    let expected_len = 4 + 1024 + 1024;
                    let valid = request_body.len() == expected_len
                        && &request_body[..4] == b"HEAD"
                        && request_body[4..4 + 1024].iter().all(|&b| b == 0)
                        && request_body[4 + 1024..].iter().all(|&b| b == 0xAB);

                    let body = if valid {
                        format!("body-ok len={}", request_body.len())
                    } else {
                        format!("body-bad len={}", request_body.len())
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body,
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            }
        }
        .in_current_span(),
    );

    (port, counter)
}
