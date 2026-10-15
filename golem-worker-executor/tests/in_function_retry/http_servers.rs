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
use tokio::sync::{Notify, mpsc};
use tracing::Instrument;

pub(crate) struct PartialResponseDropGate {
    reached: tokio::sync::oneshot::Receiver<()>,
    release: Arc<tokio::sync::Semaphore>,
}

impl PartialResponseDropGate {
    pub(crate) async fn reached(&mut self) {
        (&mut self.reached)
            .await
            .expect("partial response server stopped before reaching the drop gate");
    }

    pub(crate) fn release(&self) {
        self.release.add_permits(1);
    }
}

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

#[derive(Debug)]
pub(crate) struct CapturedStatusRetryRequest {
    pub(crate) request_line: String,
    pub(crate) content_length: usize,
    pub(crate) body: Vec<u8>,
    pub(crate) idempotency_key: Option<String>,
}

pub(crate) struct WithheldRetryResponse {
    accepted: tokio::sync::oneshot::Receiver<()>,
    peer_closed: mpsc::UnboundedReceiver<()>,
    resumed_accepted: tokio::sync::oneshot::Receiver<()>,
    release_resumed: Arc<tokio::sync::Notify>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CapturedBodyResumeRequest {
    pub(crate) request_line: String,
    pub(crate) range: Option<String>,
}

impl WithheldRetryResponse {
    pub(crate) async fn accepted(&mut self) {
        (&mut self.accepted)
            .await
            .expect("retry server stopped before accepting the replacement request");
    }

    pub(crate) async fn peer_closed(&mut self) {
        self.peer_closed
            .recv()
            .await
            .expect("retry server stopped before observing replacement cancellation");
    }

    pub(crate) async fn resumed_accepted(&mut self) {
        (&mut self.resumed_accepted)
            .await
            .expect("retry server stopped before accepting the reconstructed request");
    }

    pub(crate) fn release_resumed(&self) {
        self.release_resumed.notify_one();
    }
}

/// The first request receives HTTP 500. The replacement request is fully read but receives no
/// response headers, and the server reports when its peer closes. Later requests receive HTTP 200
/// so the interrupted invocation can reconstruct and finish after resume.
pub(crate) async fn start_withheld_status_retry_http_server() -> (
    u16,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<CapturedStatusRetryRequest>>>,
    WithheldRetryResponse,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let accepted_tx = Arc::new(Mutex::new(Some(accepted_tx)));
    let (peer_closed_tx, peer_closed_rx) = mpsc::unbounded_channel();
    let (resumed_accepted_tx, resumed_accepted_rx) = tokio::sync::oneshot::channel();
    let resumed_accepted_tx = Arc::new(Mutex::new(Some(resumed_accepted_tx)));
    let release_resumed = Arc::new(tokio::sync::Notify::new());

    spawn(
        {
            let counter = counter.clone();
            let requests = requests.clone();
            let release_resumed = release_resumed.clone();
            async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(connection) => connection,
                        Err(_) => break,
                    };
                    let counter = counter.clone();
                    let requests = requests.clone();
                    let accepted_tx = accepted_tx.clone();
                    let peer_closed_tx = peer_closed_tx.clone();
                    let resumed_accepted_tx = resumed_accepted_tx.clone();
                    let release_resumed = release_resumed.clone();
                    spawn(async move {
                        let mut data = Vec::new();
                        let mut buf = [0u8; 4096];
                        let (header_end, content_length) = loop {
                            match stream.read(&mut buf).await {
                                Ok(0) => return,
                                Ok(n) => data.extend_from_slice(&buf[..n]),
                                Err(_) => return,
                            }
                            if let Some(position) =
                                data.windows(4).position(|window| window == b"\r\n\r\n")
                            {
                                let header_end = position + 4;
                                let header_text = String::from_utf8_lossy(&data[..header_end]);
                                let content_length = parse_content_length(&header_text).unwrap_or(0);
                                break (header_end, content_length);
                            }
                        };
                        while data.len().saturating_sub(header_end) < content_length {
                            match stream.read(&mut buf).await {
                                Ok(0) => return,
                                Ok(n) => data.extend_from_slice(&buf[..n]),
                                Err(_) => return,
                            }
                        }

                        let header_text = String::from_utf8_lossy(&data[..header_end]);
                        let request_line = header_text.lines().next().unwrap_or_default().to_string();
                        let idempotency_key = header_text.lines().find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("idempotency-key")
                                    .then(|| value.trim().to_string())
                            })
                        });
                        requests.lock().unwrap().push(CapturedStatusRetryRequest {
                            request_line,
                            content_length,
                            body: data[header_end..header_end + content_length].to_vec(),
                            idempotency_key,
                        });

                        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;

                        match attempt {
                            1 => {
                                let body = "retry-me";
                                let response = format!(
                                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.shutdown().await;
                            }
                            2 => {
                                if let Some(accepted_tx) = accepted_tx.lock().unwrap().take() {
                                    let _ = accepted_tx.send(());
                                }
                                loop {
                                    match stream.read(&mut buf).await {
                                        Ok(0) | Err(_) => {
                                            let _ = peer_closed_tx.send(());
                                            break;
                                        }
                                        Ok(_) => {}
                                    }
                                }
                            }
                            3 => {
                                if let Some(resumed_accepted_tx) =
                                    resumed_accepted_tx.lock().unwrap().take()
                                {
                                    let _ = resumed_accepted_tx.send(());
                                }
                                release_resumed.notified().await;
                                let body = "status-retry-ok";
                                let response = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                );
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.shutdown().await;
                            }
                            _ => unreachable!("unexpected extra status retry request"),
                        }
                    });
                }
            }
        }
        .in_current_span(),
    );

    (
        port,
        counter,
        requests,
        WithheldRetryResponse {
            accepted: accepted_rx,
            peer_closed: peer_closed_rx,
            resumed_accepted: resumed_accepted_rx,
            release_resumed,
        },
    )
}

/// The first request receives a partial body and is disconnected. The replacement Range request
/// is accepted but receives no response headers, and the server reports when its peer closes.
/// A reconstructed original request receives the full matching body after the test releases its
/// gate.
pub(crate) async fn start_withheld_body_resume_http_server() -> (
    u16,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<CapturedBodyResumeRequest>>>,
    WithheldRetryResponse,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let accepted_tx = Arc::new(Mutex::new(Some(accepted_tx)));
    let (peer_closed_tx, peer_closed_rx) = mpsc::unbounded_channel();
    let (resumed_accepted_tx, resumed_accepted_rx) = tokio::sync::oneshot::channel();
    let resumed_accepted_tx = Arc::new(Mutex::new(Some(resumed_accepted_tx)));
    let release_resumed = Arc::new(Notify::new());

    spawn(
        {
            let counter = counter.clone();
            let requests = requests.clone();
            let release_resumed = release_resumed.clone();
            async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(connection) => connection,
                        Err(_) => break,
                    };
                    let counter = counter.clone();
                    let requests = requests.clone();
                    let accepted_tx = accepted_tx.clone();
                    let peer_closed_tx = peer_closed_tx.clone();
                    let resumed_accepted_tx = resumed_accepted_tx.clone();
                    let release_resumed = release_resumed.clone();
                    spawn(async move {
                        let mut request = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            match stream.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => request.extend_from_slice(&buf[..n]),
                            }
                            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                        }

                        let request = String::from_utf8_lossy(&request);
                        let request_line = request.lines().next().unwrap_or_default().to_string();
                        let range = request.lines().find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("range")
                                    .then(|| value.trim().to_string())
                            })
                        });
                        requests
                            .lock()
                            .unwrap()
                            .push(CapturedBodyResumeRequest {
                                request_line,
                                range,
                            });

                        let attempt = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        match attempt {
                            1 => {
                                let response = b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nres";
                                let _ = stream.write_all(response).await;
                                let _ = stream.flush().await;
                            }
                            2 => {
                                if let Some(accepted_tx) = accepted_tx.lock().unwrap().take() {
                                    let _ = accepted_tx.send(());
                                }
                                loop {
                                    match stream.read(&mut buf).await {
                                        Ok(0) | Err(_) => {
                                            let _ = peer_closed_tx.send(());
                                            break;
                                        }
                                        Ok(_) => {}
                                    }
                                }
                            }
                            3 => {
                                if let Some(resumed_accepted_tx) =
                                    resumed_accepted_tx.lock().unwrap().take()
                                {
                                    let _ = resumed_accepted_tx.send(());
                                }
                                release_resumed.notified().await;
                                let response = b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nresumed-body";
                                let _ = stream.write_all(response).await;
                                let _ = stream.shutdown().await;
                            }
                            _ => unreachable!("unexpected extra body resume request"),
                        }
                    });
                }
            }
        }
        .in_current_span(),
    );

    (
        port,
        counter,
        requests,
        WithheldRetryResponse {
            accepted: accepted_rx,
            peer_closed: peer_closed_rx,
            resumed_accepted: resumed_accepted_rx,
            release_resumed,
        },
    )
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
/// responds 206 with remaining bytes; `resume_status` 416 refuses a Range request,
/// and other statuses send the full body. The body is `body_size` bytes of sequential
/// values (i % 256), except that the 416 case starts with `h` for the P2 reader.
/// Returns `(port, connection_counter, range_counter)`.
pub(crate) async fn start_partial_response_http_server(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
) -> (u16, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (port, connection_counter, range_counter, _) = start_partial_response_http_server_inner(
        fail_count,
        prefix_len,
        body_size,
        initial_status,
        resume_status,
        resume_supports_range,
        0,
        false,
    )
    .await;
    (port, connection_counter, range_counter)
}

pub(crate) async fn start_recovery_gated_partial_response_http_server(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
) -> (
    u16,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    PartialResponseDropGate,
) {
    let (port, connection_counter, range_counter, gate) = start_partial_response_http_server_inner(
        fail_count,
        prefix_len,
        body_size,
        initial_status,
        resume_status,
        resume_supports_range,
        0,
        true,
    )
    .await;
    (
        port,
        connection_counter,
        range_counter,
        gate.expect("requested partial response drop gate"),
    )
}

pub(crate) async fn start_gated_partial_response_http_server_with_resume_send_failures(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
    resume_send_failures: usize,
) -> (
    u16,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    PartialResponseDropGate,
) {
    let (port, connection_counter, range_counter, gate) = start_partial_response_http_server_inner(
        fail_count,
        prefix_len,
        body_size,
        initial_status,
        resume_status,
        resume_supports_range,
        resume_send_failures,
        true,
    )
    .await;
    (
        port,
        connection_counter,
        range_counter,
        gate.expect("requested partial response drop gate"),
    )
}

async fn start_partial_response_http_server_inner(
    fail_count: usize,
    prefix_len: usize,
    body_size: usize,
    initial_status: u16,
    resume_status: u16,
    resume_supports_range: bool,
    resume_send_failures: usize,
    gated: bool,
) -> (
    u16,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Option<PartialResponseDropGate>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let range_counter = Arc::new(AtomicUsize::new(0));
    let range_counter_clone = range_counter.clone();
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let server_release = release.clone();
    let mut reached_tx = gated.then_some(reached_tx);

    // The gated fault fixture labels every eight-byte record with its absolute position. This
    // keeps the body ASCII while ensuring equal-sized read chunks are not interchangeable.
    let mut full_body: Vec<u8> = if gated {
        (0..body_size)
            .map(|offset| {
                let record = offset / 8;
                let column = offset % 8;
                if column == 7 {
                    b'\n'
                } else {
                    b"0123456789abcdef"[(record >> ((6 - column) * 4)) & 0xf]
                }
            })
            .collect()
    } else {
        (0..body_size).map(|i| (i % 256) as u8).collect()
    };
    if resume_status == 416
        && let Some(first) = full_body.first_mut()
    {
        *first = b'h';
    }

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
                    if n == 0 && let Some(reached_tx) = reached_tx.take() {
                        let _ = reached_tx.send(());
                        let permit = server_release
                            .acquire()
                            .await
                            .expect("partial response drop gate was closed");
                        permit.forget();
                    } else {
                        // Wait for the client to receive the partial data before dropping
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                    drop(stream);
                } else if n < fail_count + resume_send_failures {
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

                    if resume_status == 416 && range_start.is_some() {
                        let response = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes()).await;
                    } else if resume_supports_range && let Some(start) = range_start {
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
                            416 => "Range Not Satisfiable",
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

    let gate = gated.then_some(PartialResponseDropGate {
        reached: reached_rx,
        release,
    });
    (port, counter, range_counter, gate)
}

pub(crate) async fn start_gated_partial_response_http_server(
    prefix_len: usize,
    body: Vec<u8>,
    replacement_status: u16,
) -> (
    u16,
    mpsc::UnboundedReceiver<Option<usize>>,
    Arc<AtomicUsize>,
    Arc<Notify>,
    Arc<Notify>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let replacement_ready = Arc::new(Notify::new());
    let replacement_ready_clone = replacement_ready.clone();
    let release_replacement = Arc::new(Notify::new());
    let release_replacement_clone = release_replacement.clone();

    spawn(
        async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(connection) => connection,
                    Err(_) => break,
                };
                let request_number = counter_clone.fetch_add(1, Ordering::SeqCst);
                let body = body.clone();
                let request_tx = request_tx.clone();
                let replacement_ready = replacement_ready_clone.clone();
                let release_replacement = release_replacement_clone.clone();

                spawn(
                    async move {
                        let mut request = Vec::new();
                        let mut buffer = [0u8; 4096];
                        loop {
                            match stream.read(&mut buffer).await {
                                Ok(0) => return,
                                Ok(read) => {
                                    request.extend_from_slice(&buffer[..read]);
                                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                                        break;
                                    }
                                }
                                Err(_) => return,
                            }
                        }

                        let request = String::from_utf8_lossy(&request);
                        let range_start = request.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if !name.eq_ignore_ascii_case("range") {
                                return None;
                            }
                            value
                                .trim()
                                .strip_prefix("bytes=")?
                                .strip_suffix('-')?
                                .parse::<usize>()
                                .ok()
                        });
                        let _ = request_tx.send(range_start);

                        if request_number == 0 {
                            let headers = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(headers.as_bytes()).await;
                            let _ = stream.write_all(&body[..prefix_len]).await;
                            let _ = stream.flush().await;
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            return;
                        }

                        if request_number >= 2 {
                            let body = br#"{"percentage":0.25,"message":"permit released"}"#;
                            let headers = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = stream.write_all(headers.as_bytes()).await;
                            let _ = stream.write_all(body).await;
                            let _ = stream.shutdown().await;
                            return;
                        }

                        let start = range_start.unwrap_or(0);
                        let remaining = if replacement_status == 206 {
                            &body[start..]
                        } else {
                            &[]
                        };
                        let headers = if replacement_status == 206 {
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                start,
                                body.len() - 1,
                                body.len(),
                                remaining.len()
                            )
                        } else {
                            format!(
                                "HTTP/1.1 {replacement_status} Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                        };
                        let _ = stream.write_all(headers.as_bytes()).await;
                        let _ = stream.flush().await;
                        if request_number == 1 {
                            replacement_ready.notify_one();
                            release_replacement.notified().await;
                        }
                        let _ = stream.write_all(remaining).await;
                        let _ = stream.shutdown().await;
                    }
                    .in_current_span(),
                );
            }
        }
        .in_current_span(),
    );

    (
        port,
        request_rx,
        counter,
        replacement_ready,
        release_replacement,
    )
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
