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
