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

    /// Sends a raw wasip3 PUT request with a body larger than the inline replay buffer.
    async fn put_with_p3_oversized_body(&self) -> String;

    /// Sends a raw wasip3 PUT request with a small body but keeps the body stream open
    /// while awaiting response headers.
    async fn put_with_p3_small_body_open_until_response(&self) -> String;

    /// Sends a raw wasip3 PUT request with a declared small content-length but keeps the
    /// body stream open while awaiting response headers.
    async fn put_with_p3_declared_small_body_open_until_response(&self) -> String;

    /// Sends a GET request and reads the response body in chunks.
    async fn get_and_read_body_chunked(&self) -> String;

    /// Sends a buffered POST with a body composed of: 4 bytes "HEAD", then 1024
    /// zero bytes, then 1024 bytes of 0xAB. The name is historical (the body was
    /// once produced with the wasip2 `output-stream::write-zeroes` API); the body
    /// layout is kept so server-side validation of the exact bytes still applies.
    async fn post_with_write_zeroes(&self) -> String;

    /// Sends a buffered POST with a large multi-chunk body. The name is
    /// historical (the writes were once interleaved with wasip2
    /// `output-stream::subscribe` polling); it now only exercises the plain
    /// large-body path.
    async fn post_with_subscribe(&self) -> String;

    /// Sends a POST request and finishes the body with trailers.
    async fn post_with_trailers(&self) -> String;

    /// Sends a raw wasip3 PUT request and finishes the body with trailers.
    async fn put_with_p3_trailers(&self) -> String;

    /// Sends a raw wasip3 POST request and finishes the body with trailers.
    async fn post_with_p3_trailers(&self) -> String;

    /// Sends a raw wasip3 POST streaming a deterministic multi-chunk body via
    /// `wit_stream` (no declared content-length), then echoes the response.
    async fn post_with_p3_streamed_body(&self) -> String;

    /// Sends a raw wasip3 POST streaming a deterministic multi-megabyte body
    /// via `wit_stream` (each chunk larger than the oplog inline payload limit
    /// in the tests using it), then echoes the response.
    async fn post_with_p3_large_streamed_body(&self) -> String;

    /// Sends a GET, then reads the response body.
    async fn get_with_body_skip(&self) -> String;

    /// Sends a buffered POST with a retry policy that retries HTTP 500 responses.
    async fn post_with_status_retry_policy(&self) -> String;

    /// Sends a raw wasip3 POST whose bounded body is terminal before send.
    async fn p3_terminal_post(&self) -> String;

    /// Sends a GET request and drops the response without reading its body,
    /// returning only the status code.
    async fn get_and_drop_response(&self) -> String;

    /// Starts a GET request and cancels the still-pending response future
    /// before the server sends response headers.
    async fn get_and_cancel_before_response(&self) -> String;

    /// Sends a GET request, reads one response-body chunk, then drops the body
    /// stream before EOF.
    async fn get_and_drop_body_after_first_chunk(&self) -> String;

    /// Sends a GET request, starts reading a response-body chunk, then cancels
    /// that pending read before the server sends any body bytes.
    async fn get_and_cancel_pending_body_read(&self) -> String;

    /// Sends a POST via raw wasip3 `wasi:http` whose declared `content-length`
    /// is larger than the bytes actually written, and returns the request-body
    /// transmission future's result (together with the send outcome). The
    /// short body is a deterministic transmission error
    /// (`HttpRequestBodySize`), which must replay identically.
    async fn post_with_short_body_transmission_error(&self) -> String;
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

    async fn put_with_p3_oversized_body(&self) -> String {
        do_put_with_p3_oversized_body().await
    }

    async fn put_with_p3_small_body_open_until_response(&self) -> String {
        do_put_with_p3_small_body_open_until_response().await
    }

    async fn put_with_p3_declared_small_body_open_until_response(&self) -> String {
        do_put_with_p3_declared_small_body_open_until_response().await
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

    async fn put_with_p3_trailers(&self) -> String {
        do_put_with_p3_trailers().await
    }

    async fn post_with_p3_trailers(&self) -> String {
        do_post_with_p3_trailers().await
    }

    async fn post_with_p3_streamed_body(&self) -> String {
        do_post_with_p3_streamed_body(8, 8 * 1024).await
    }

    async fn post_with_p3_large_streamed_body(&self) -> String {
        do_post_with_p3_streamed_body(16, 128 * 1024).await
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

        with_named_policy_async(&policy, || async { do_p3_terminal_post().await })
            .await
            .unwrap()
    }

    async fn p3_terminal_post(&self) -> String {
        do_p3_terminal_post().await
    }

    async fn get_and_drop_response(&self) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());

        let response = wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/"))
            .send()
            .await
            .expect("Request failed");

        let status = response.status().as_u16();
        drop(response);
        format!("{status}")
    }

    async fn get_and_cancel_before_response(&self) -> String {
        do_get_and_cancel_before_response().await
    }

    async fn get_and_drop_body_after_first_chunk(&self) -> String {
        do_get_and_drop_body_after_first_chunk().await
    }

    async fn get_and_cancel_pending_body_read(&self) -> String {
        do_get_and_cancel_pending_body_read().await
    }

    async fn post_with_short_body_transmission_error(&self) -> String {
        do_post_with_short_body_transmission_error().await
    }
}

async fn do_get_and_cancel_before_response() -> String {
    use futures_concurrency::prelude::*;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let request = async {
        let result = wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/delayed-response"))
            .send()
            .await;
        match result {
            Ok(response) => {
                let status = response.status().as_u16();
                drop(response);
                format!("completed({status})")
            }
            Err(error) => format!("error({error:?})"),
        }
    };
    let cancel = async {
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(50_000_000).await;
        "cancelled-before-response".to_string()
    };

    (request, cancel).race().await
}

async fn do_get_and_drop_body_after_first_chunk() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/slow-body"))
        .send()
        .await
        .expect("Request failed");
    let status = response.status().as_u16();
    let mut body = response.into_body();
    let first = body.chunk().await.unwrap_or_default();
    let len = first.len();
    drop(body);
    format!("{status} first-chunk={len}")
}

async fn do_get_and_cancel_pending_body_read() -> String {
    use futures_concurrency::prelude::*;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/stalled-body"))
        .send()
        .await
        .expect("Request failed");
    let status = response.status().as_u16();
    let mut body = response.into_body();

    let read = async {
        let chunk = body.chunk().await.unwrap_or_default();
        format!("read({status}, {})", chunk.len())
    };
    let cancel = async {
        let mut yielded = false;
        futures_util::future::poll_fn(|cx| {
            if yielded {
                std::task::Poll::Ready(())
            } else {
                yielded = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        })
        .await;
        format!("cancelled-during-body-read({status})")
    };

    (read, cancel).race().await
}

async fn do_post_with_short_body_transmission_error() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("content-length".to_string(), b"1024".to_vec())]).unwrap();

    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (send_result, transmit_result, ()) = (
        async { client::send(request).await },
        async { transmit.await },
        async {
            // Write fewer bytes than `content-length` declares, then close the
            // body stream: the mismatch fails the transmission future
            // deterministically with `HttpRequestBodySize`.
            let remaining = body_tx.write_all(b"short".to_vec()).await;
            assert!(remaining.is_empty());
            let _ = trailers_tx.write(Ok(None)).await;
            drop(body_tx);
        },
    )
        .join()
        .await;

    // The send outcome is not asserted by the tests (whether the response head
    // arrives before the aborted upload is a race), only the transmission
    // result is; both are durable so both replay deterministically.
    let send = match send_result {
        Ok(response) => {
            let status = response.get_status_code();
            drop(response);
            format!("Ok({status})")
        }
        Err(err) => format!("Err({err:?})"),
    };
    format!("send={send} transmit={transmit_result:?}")
}

async fn do_put_with_p3_trailers() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = types::Fields::from_list(&[
        ("x-test".to_string(), b"test-header".to_vec()),
        ("trailer".to_string(), b"x-test-trailer".to_vec()),
    ])
    .unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Put).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (send_result, transmit_result, ()) = (
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(b"test-body".to_vec()).await;
            assert!(remaining.is_empty());
            drop(body_tx);
            let trailers = types::Fields::from_list(&[(
                "x-test-trailer".to_string(),
                b"trailer-value".to_vec(),
            )])
            .unwrap();
            let _ = trailers_tx.write(Ok(Some(trailers))).await;
        },
    )
        .join()
        .await;

    let response = send_result.expect("Request failed");
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut body_bytes = Vec::new();
    let mut buffer = Vec::with_capacity(1024);
    loop {
        let (result, next_buffer) = body.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(n) => {
                body_bytes.extend_from_slice(&buffer[..n]);
                buffer.clear();
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }
    drop(body);
    trailers.await.expect("response trailers failed");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    format!(
        "{status} {} transmit={transmit_result:?}",
        String::from_utf8_lossy(&body_bytes)
    )
}

async fn do_post_with_p3_trailers() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = types::Fields::from_list(&[
        ("x-test".to_string(), b"test-header".to_vec()),
        ("trailer".to_string(), b"x-test-trailer".to_vec()),
    ])
    .unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (send_result, transmit_result, ()) = (
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(b"test-body".to_vec()).await;
            assert!(remaining.is_empty());
            drop(body_tx);
            let trailers = types::Fields::from_list(&[(
                "x-test-trailer".to_string(),
                b"trailer-value".to_vec(),
            )])
            .unwrap();
            let _ = trailers_tx.write(Ok(Some(trailers))).await;
        },
    )
        .join()
        .await;

    let response = send_result.expect("Request failed");
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut body_bytes = Vec::new();
    let mut buffer = Vec::with_capacity(1024);
    loop {
        let (result, next_buffer) = body.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(n) => {
                body_bytes.extend_from_slice(&buffer[..n]);
                buffer.clear();
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }
    drop(body);
    trailers.await.expect("response trailers failed");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    format!(
        "{status} {} transmit={transmit_result:?}",
        String::from_utf8_lossy(&body_bytes)
    )
}

/// Streams `chunk_count` chunks of `chunk_len` bytes each, where byte `j` of
/// chunk `i` is `(i * 31 + j) % 251`. The worker-executor tests reconstruct
/// the same sequence to assert the server received the body byte-identically.
async fn do_post_with_p3_streamed_body(chunk_count: usize, chunk_len: usize) -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"streamed-body".to_vec())]).unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (send_result, transmit_result, ()) = (
        async { client::send(request).await },
        async { transmit.await },
        async {
            for i in 0..chunk_count {
                let chunk: Vec<u8> = (0..chunk_len)
                    .map(|j| ((i * 31 + j) % 251) as u8)
                    .collect();
                let remaining = body_tx.write_all(chunk).await;
                assert!(remaining.is_empty(), "request body receiver closed early");
            }
            drop(body_tx);
            let _ = trailers_tx.write(Ok(None)).await;
        },
    )
        .join()
        .await;

    let response = send_result.expect("Request failed");
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut body_bytes = Vec::new();
    let mut buffer = Vec::with_capacity(1024);
    loop {
        let (result, next_buffer) = body.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(n) => {
                body_bytes.extend_from_slice(&buffer[..n]);
                buffer.clear();
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }
    drop(body);
    trailers.await.expect("response trailers failed");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    format!(
        "{status} {} transmit={transmit_result:?}",
        String::from_utf8_lossy(&body_bytes)
    )
}

async fn do_put_with_p3_oversized_body() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"oversized-body".to_vec())]).unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Put).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (send_result, transmit_result, ()) = (
        async { client::send(request).await },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(vec![0xCDu8; 2 * 1024 * 1024]).await;
            assert!(remaining.is_empty(), "request body receiver closed early");
            drop(body_tx);
            let _ = trailers_tx.write(Ok(None)).await;
        },
    )
        .join()
        .await;

    let response = send_result.expect("Request failed");
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut body_bytes = Vec::new();
    let mut buffer = Vec::with_capacity(1024);
    loop {
        let (result, next_buffer) = body.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(n) => {
                body_bytes.extend_from_slice(&buffer[..n]);
                buffer.clear();
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }
    drop(body);
    trailers.await.expect("response trailers failed");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    format!(
        "{status} {} transmit={transmit_result:?}",
        String::from_utf8_lossy(&body_bytes)
    )
}

async fn do_put_with_p3_small_body_open_until_response() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::clocks::monotonic_clock;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"open-small-body".to_vec())]).unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (_trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, _transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Put).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request
        .set_path_with_query(Some("/early-response"))
        .unwrap();

    let send = async {
        match client::send(request).await {
            Ok(response) => {
                let status = response.get_status_code();
                drop(response);
                format!("completed({status})")
            }
            Err(err) => format!("error({err:?})"),
        }
    };

    let hold_body_open = async {
        let remaining = body_tx.write_all(b"hello".to_vec()).await;
        assert!(remaining.is_empty(), "request body receiver closed early");
        monotonic_clock::wait_for(200_000_000).await;
        "timed-out-before-response".to_string()
    };

    (send, hold_body_open).race().await
}

async fn do_put_with_p3_declared_small_body_open_until_response() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::clocks::monotonic_clock;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = types::Fields::from_list(&[
        ("x-test".to_string(), b"declared-open-small-body".to_vec()),
        ("content-length".to_string(), b"5".to_vec()),
    ])
    .unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (_trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, _transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Put).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request
        .set_path_with_query(Some("/early-response"))
        .unwrap();

    let send = async {
        match client::send(request).await {
            Ok(response) => {
                let status = response.get_status_code();
                drop(response);
                format!("completed({status})")
            }
            Err(err) => format!("error({err:?})"),
        }
    };

    let hold_body_open = async {
        let remaining = body_tx.write_all(b"hello".to_vec()).await;
        assert!(remaining.is_empty(), "request body receiver closed early");
        monotonic_clock::wait_for(200_000_000).await;
        "timed-out-before-response".to_string()
    };

    (send, hold_body_open).race().await
}

async fn do_p3_terminal_post() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = types::Fields::from_list(&[
        ("x-test".to_string(), b"test-header".to_vec()),
        ("content-length".to_string(), b"9".to_vec()),
    ])
    .unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Post).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    let (response_text, transmit_result, ()) = (
        async {
            let response = client::send(request).await.expect("Request failed");
            let status = response.get_status_code();
            let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
            let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
            let mut body_bytes = Vec::new();
            let mut buffer = Vec::with_capacity(1024);
            loop {
                let (result, next_buffer) = body.read(buffer).await;
                buffer = next_buffer;
                match result {
                    StreamResult::Complete(n) => {
                        body_bytes.extend_from_slice(&buffer[..n]);
                        buffer.clear();
                    }
                    StreamResult::Dropped => break,
                    StreamResult::Cancelled => panic!("response body read was cancelled"),
                }
            }
            drop(body);
            trailers.await.expect("response trailers failed");
            response_done_tx
                .write(Ok(()))
                .await
                .expect("failed to acknowledge response body");
            format!("{status} {}", String::from_utf8_lossy(&body_bytes))
        },
        async { transmit.await },
        async {
            let remaining = body_tx.write_all(b"test-body".to_vec()).await;
            assert!(remaining.is_empty());
            drop(body_tx);
            trailers_tx
                .write(Ok(None))
                .await
                .expect("failed to close request trailers");
        },
    )
        .join()
        .await;
    assert!(transmit_result.is_ok());
    response_text
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
