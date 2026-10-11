use golem_rust::retry::{NamedPolicy, Policy, Predicate, Props, with_named_policy_async};
use golem_rust::{agent_definition, agent_implementation, with_idempotence_mode_async};
use std::time::Duration;

#[agent_definition]
pub trait HttpClient4 {
    fn new() -> Self;

    async fn transition_clock_probe(&self, post: bool) -> u16;

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

    /// Sends a raw wasip3 PUT request and keeps the body stream open until the first
    /// response-body read completes.
    async fn put_with_p3_body_open_until_response_read(&self) -> String;

    /// Sends a raw wasip3 PUT request with a declared small content-length but keeps the
    /// body stream open while awaiting response headers.
    async fn put_with_p3_declared_small_body_open_until_response(&self) -> String;

    /// Sends a GET request and reads the response body in chunks.
    async fn get_and_read_body_chunked(&self) -> String;

    /// Awaits response headers, then consumes the complete body without a guest timeout.
    async fn get_for_header_wait(&self, url: String) -> String;

    /// Waits for response headers using the synchronous WASI HTTP P2 response poll.
    fn get_for_p2_header_wait(&self, authority: String) -> String;

    /// Reads one response-body byte, then blocks on the remainder without a guest timeout.
    fn get_for_p2_body_wait(&self, authority: String) -> String;

    /// Reads one byte, then blocks while skipping the second response-body byte.
    fn get_for_p2_body_skip(&self, authority: String) -> String;

    /// Reads a streamed response to EOF without a guest timeout.
    async fn get_for_body_wait(&self, url: String) -> String;
    /// Sends a raw P2 GET, reads one byte, then blocking-reads the rest.
    fn get_and_read_body_p2_blocking(&mut self, authority: String) -> String;

    /// Splices a raw P2 TCP input into an already-dispatched HTTP request body.
    fn post_tcp_body_p2(&self, authority: String, tcp_port: u16, len: u64) -> String;

    /// Sends a raw WASI HTTP 0.2 GET and reads the response with blocking-read.
    async fn get_and_blocking_read_body_p2(&self) -> String;

    /// Sends the same GET with a guest-supplied Range header.
    async fn get_and_blocking_read_body_p2_with_range(&self) -> String;

    /// Sends the same GET while disabling the worker idempotence override.
    async fn get_and_blocking_read_body_p2_without_idempotence_override(&self) -> String;

    /// Sends the same request under an unbounded immediate retry policy.
    async fn get_and_blocking_read_body_p2_with_unbounded_retry(&self) -> String;

    /// Sends the same request under a periodic policy whose delay requires trap-based retry.
    async fn get_and_blocking_read_body_p2_with_delayed_retry(&self) -> String;

    /// Sends a raw WASI HTTP 0.2 GET and reads the response with non-blocking read.
    async fn get_and_read_body_p2(&self) -> String;

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

    /// Sends a GET carrying an explicit `Range` header, then reads the
    /// response body in chunks. A guest-set `Range` header disqualifies
    /// response-body resume, so a mid-body failure must recover via
    /// trap+replay instead.
    async fn get_with_range_header(&self) -> String;

    /// Sends a buffered POST with a retry policy that retries HTTP 500 responses.
    async fn post_with_status_retry_policy(&self) -> String;

    /// Sends the same status-retried POST through the WASI HTTP 0.2 API.
    async fn post_with_status_retry_policy_p2(&self) -> String;

    /// Sends a raw wasip3 POST whose bounded body is terminal before send.
    async fn p3_terminal_post(&self) -> String;

    /// Sends a GET request and drops the response without reading its body,
    /// returning only the status code.
    async fn get_and_drop_response(&self) -> String;

    /// Starts a GET request and cancels the still-pending response future
    /// before the server sends response headers.
    async fn get_and_cancel_before_response(&self) -> String;

    /// Starts a non-idempotent POST request and cancels the still-pending
    /// response future before the server sends response headers. Unlike the
    /// GET variant, the cancellable durable send is dropped mid-flight and
    /// recorded as a `Cancelled` oplog entry.
    async fn post_and_cancel_before_response(&self) -> String;

    /// Sends a GET request, reads one response-body chunk, then drops the body
    /// stream before EOF.
    async fn get_and_drop_body_after_first_chunk(&self) -> String;

    /// Sends a GET request, starts reading a response-body chunk, then cancels
    /// that pending read before the server sends any body bytes.
    async fn get_and_cancel_pending_body_read(&self) -> String;

    /// Sends a GET request whose response body delivers one chunk and then
    /// stalls, starts reading a body chunk, and cancels that pending read only
    /// once a second request (`GET /cancel-signal`) completes — the test
    /// server withholds that response until the exact moment it wants the
    /// pending read dropped. The body stream is then dropped as well, and a
    /// third request (`GET /cancel-done`) confirms to the test server that
    /// both the pending read future and the body stream are gone.
    async fn get_and_cancel_body_read_after_signal(&self) -> String;

    /// Starts a raw wasip3 response-body read, waits for a second request to
    /// complete, then cancels the read and verifies that a concurrently
    /// completed transfer still returns its byte count and destination bytes.
    async fn get_and_cancel_body_read_with_bytes(&self) -> String;

    /// Sends a POST via raw wasip3 `wasi:http` whose declared `content-length`
    /// is larger than the bytes actually written, and returns the request-body
    /// transmission future's result (together with the send outcome). The
    /// short body is a deterministic transmission error
    /// (`HttpRequestBodySize`), which must replay identically.
    async fn post_with_short_body_transmission_error(&self) -> String;

    /// Sends a raw wasip3 GET over `https` to a plain-HTTP server, which fails
    /// the TLS handshake with a permanent (non-retried) `ErrorCode`. The error
    /// returned by `client::send` is stored in agent state and returned.
    async fn send_with_permanent_error(&mut self) -> String;

    /// Returns the error stored by the last `send_with_permanent_error` call
    /// (rebuilt from the oplog on replay).
    fn stored_send_error(&self) -> String;

    /// Sends a raw wasip3 GET and stores the full response — status code, the
    /// `x-resp-test` response header, and the body — in agent state, formatted
    /// as a single string, which is also returned.
    async fn get_and_store_full_response(&mut self) -> String;

    /// Reads the full response but leaves its trailers future unconsumed.
    async fn get_and_ignore_trailers(&mut self) -> String;

    /// Cancels a pending body read after its first chunk and leaves trailers unconsumed.
    async fn get_and_cancel_body_ignoring_trailers(&mut self) -> String;

    /// Returns the response stored by the last `get_and_store_full_response`
    /// call (rebuilt from the oplog on replay).
    fn stored_full_response(&self) -> String;

    /// Spawns a guest task that performs a full HTTP GET (send and body read)
    /// and returns from the export immediately without awaiting it: the
    /// durable HTTP calls happen only after the export has returned.
    async fn get_in_spawned_task_after_return(&self) -> String;
}

#[agent_definition(mode = "ephemeral")]
pub trait EphemeralHttpClient4 {
    fn new() -> Self;

    /// Awaits response headers, then consumes the complete body without a guest timeout.
    async fn get_for_header_wait(&self, url: String) -> String;

    /// Waits for response headers using the synchronous WASI HTTP P2 response poll.
    fn get_for_p2_header_wait(&self, authority: String) -> String;

    /// Reads one response-body byte, then blocks on the remainder without a guest timeout.
    fn get_for_p2_body_wait(&self, authority: String) -> String;

    /// Reads one byte, then blocks while skipping the second response-body byte.
    fn get_for_p2_body_skip(&self, authority: String) -> String;

    /// Reads a streamed response to EOF without a guest timeout.
    async fn get_for_body_wait(&self, url: String) -> String;
}

struct EphemeralHttpClient4Impl;

#[agent_implementation]
impl EphemeralHttpClient4 for EphemeralHttpClient4Impl {
    fn new() -> Self {
        Self
    }

    async fn get_for_header_wait(&self, url: String) -> String {
        do_get_for_header_wait(url).await
    }

    fn get_for_p2_header_wait(&self, authority: String) -> String {
        do_get_for_p2_header_wait(authority)
    }

    fn get_for_p2_body_wait(&self, authority: String) -> String {
        do_get_for_p2_body_wait(authority)
    }

    fn get_for_p2_body_skip(&self, authority: String) -> String {
        do_get_for_p2_body_skip(authority)
    }

    async fn get_for_body_wait(&self, url: String) -> String {
        do_get_for_body_wait(url).await
    }
}

fn do_get_for_p2_body_wait(authority: String) -> String {
    use wasi::http::{outgoing_handler, types};
    use wasi::io::streams::StreamError;

    let request = types::OutgoingRequest::new(types::Fields::new());
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&authority)).unwrap();
    request.set_path_with_query(Some("/body-wait")).unwrap();
    types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();
    let response = outgoing_handler::handle(request, None).unwrap();
    let headers = loop {
        match response.get() {
            Some(Ok(Ok(headers))) => break headers,
            Some(Ok(Err(error))) => panic!("HTTP response failed: {error:?}"),
            Some(Err(error)) => panic!("HTTP response failed: {error:?}"),
            None => {
                let pollable = response.subscribe();
                let _ = wasi::io::poll::poll(&[&pollable]);
            }
        }
    };
    let status = headers.status();
    let body = headers.consume().unwrap();
    let stream = body.stream().unwrap();
    let first = stream.blocking_read(1).unwrap();
    assert_eq!(first, b"h");
    let mut rest = Vec::new();
    loop {
        match stream.blocking_read(1) {
            Ok(bytes) => rest.extend_from_slice(&bytes),
            Err(StreamError::Closed) => break,
            Err(error) => panic!("P2 body read failed: {error:?}"),
        }
    }
    format!("{status} h{}", String::from_utf8_lossy(&rest))
}

fn do_get_for_p2_body_skip(authority: String) -> String {
    use wasi::http::{outgoing_handler, types};
    use wasi::io::streams::StreamError;

    let request = types::OutgoingRequest::new(types::Fields::new());
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&authority)).unwrap();
    request.set_path_with_query(Some("/body-wait")).unwrap();
    types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();
    let response = outgoing_handler::handle(request, None).unwrap();
    let headers = loop {
        match response.get() {
            Some(Ok(Ok(headers))) => break headers,
            Some(Ok(Err(error))) => panic!("HTTP response failed: {error:?}"),
            Some(Err(error)) => panic!("HTTP response failed: {error:?}"),
            None => {
                let pollable = response.subscribe();
                let _ = wasi::io::poll::poll(&[&pollable]);
            }
        }
    };
    let status = headers.status();
    let body = headers.consume().unwrap();
    let stream = body.stream().unwrap();
    assert_eq!(stream.blocking_read(1).unwrap(), b"h");
    let skipped = stream.blocking_skip(1).unwrap();
    assert_eq!(skipped, 1);
    assert!(matches!(stream.blocking_read(1), Err(StreamError::Closed)));
    format!("{status} h skipped {skipped}")
}

async fn do_get_for_body_wait(url: String) -> String {
    let response = wasi_fetch::Client::new()
        .get(&url)
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

fn do_get_for_p2_header_wait(authority: String) -> String {
    use wasi::http::{outgoing_handler, types};

    let request = types::OutgoingRequest::new(types::Fields::new());
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&authority)).unwrap();
    request.set_path_with_query(Some("/header-wait")).unwrap();
    types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();
    let response = outgoing_handler::handle(request, None).unwrap();
    loop {
        match response.get() {
            Some(Ok(Ok(headers))) => return headers.status().to_string(),
            Some(Ok(Err(error))) => panic!("HTTP response failed: {error:?}"),
            Some(Err(error)) => panic!("HTTP response failed: {error:?}"),
            None => {
                let pollable = response.subscribe();
                let _ = wasi::io::poll::poll(&[&pollable]);
            }
        }
    }
}

async fn do_get_for_header_wait(url: String) -> String {
    let response = wasi_fetch::Client::new()
        .get(&url)
        .send()
        .await
        .expect("Request failed");
    let status = response.status().as_u16();
    let body = response.into_body().bytes().await;
    format!("{status} {}", String::from_utf8_lossy(&body))
}

struct HttpClient4Impl {
    last_send_error: Option<String>,
    last_full_response: Option<String>,
}

#[agent_implementation]
impl HttpClient4 for HttpClient4Impl {
    fn new() -> Self {
        Self {
            last_send_error: None,
            last_full_response: None,
        }
    }

    async fn transition_clock_probe(&self, post: bool) -> u16 {
        use futures_concurrency::prelude::*;
        use golem_rust::wasip3::http::{client, types};
        use golem_rust::wasip3::wit_future;
        let port = std::env::var("PORT").unwrap();
        let headers = types::Fields::from_list(&[]).unwrap();
        let (tx, rx) = wit_future::new(|| Ok(None));
        let (request, transmit) = types::Request::new(headers, None, rx, None);
        request
            .set_method(&if post {
                types::Method::Post
            } else {
                types::Method::Get
            })
            .unwrap();
        request.set_scheme(Some(&types::Scheme::Http)).unwrap();
        request
            .set_authority(Some(&format!("127.0.0.1:{port}")))
            .unwrap();
        request.set_path_with_query(Some("/transition")).unwrap();
        let send = async {
            let response = client::send(request).await.unwrap();
            let status = response.get_status_code();
            drop(response);
            status
        };
        let finish = async {
            tx.write(Ok(None)).await.unwrap();
            transmit.await.unwrap();
        };
        let clock = golem_rust::wasip3::clocks::monotonic_clock::wait_for(1_000_000);
        let (status, (), ()) = (send, finish, clock).join().await;
        status
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

    async fn put_with_p3_body_open_until_response_read(&self) -> String {
        do_put_with_p3_body_open_until_response_read().await
    }

    async fn put_with_p3_declared_small_body_open_until_response(&self) -> String {
        do_put_with_p3_declared_small_body_open_until_response().await
    }

    async fn get_and_read_body_chunked(&self) -> String {
        do_get_chunked_read().await
    }

    async fn get_for_header_wait(&self, url: String) -> String {
        do_get_for_header_wait(url).await
    }

    fn get_for_p2_header_wait(&self, authority: String) -> String {
        do_get_for_p2_header_wait(authority)
    }

    fn get_for_p2_body_wait(&self, authority: String) -> String {
        do_get_for_p2_body_wait(authority)
    }

    fn get_for_p2_body_skip(&self, authority: String) -> String {
        do_get_for_p2_body_skip(authority)
    }

    async fn get_for_body_wait(&self, url: String) -> String {
        do_get_for_body_wait(url).await
    }

    fn get_and_read_body_p2_blocking(&mut self, authority: String) -> String {
        let response = do_get_and_read_body_p2_blocking(authority);
        self.last_full_response = Some(response.clone());
        response
    }

    fn post_tcp_body_p2(&self, authority: String, tcp_port: u16, len: u64) -> String {
        use wasi::http::{outgoing_handler, types};
        use wasi::sockets::network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};

        let request = types::OutgoingRequest::new(types::Fields::new());
        request.set_method(&types::Method::Post).unwrap();
        request.set_scheme(Some(&types::Scheme::Http)).unwrap();
        request.set_authority(Some(&authority)).unwrap();
        request.set_path_with_query(Some("/")).unwrap();
        let body = request.body().unwrap();
        let output = body.write().unwrap();
        let response = outgoing_handler::handle(request, None).unwrap();

        // Keep raw socket readiness inside the live attempt of the HTTP batch.
        let socket =
            wasi::sockets::tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4).unwrap();
        socket
            .start_connect(
                &wasi::sockets::instance_network::instance_network(),
                IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    address: (127, 0, 0, 1),
                    port: tcp_port,
                }),
            )
            .unwrap();
        socket.subscribe().block();
        let (input, _output) = socket.finish_connect().unwrap();

        let mut written = 0;
        while written < len {
            written += output.blocking_splice(&input, len - written).unwrap();
        }
        drop(output);
        types::OutgoingBody::finish(body, None).unwrap();
        let response = get_incoming_response_p2(&response);
        let status = response.status();
        let body = response.consume().unwrap();
        let stream = body.stream().unwrap();
        let mut bytes = Vec::new();
        loop {
            match stream.blocking_read(1024) {
                Ok(chunk) => bytes.extend_from_slice(&chunk),
                Err(wasi::io::streams::StreamError::Closed) => break,
                Err(error) => panic!("P2 response read failed: {error:?}"),
            }
        }
        format!("{status} {}", String::from_utf8(bytes).unwrap())
    }

    async fn get_and_blocking_read_body_p2(&self) -> String {
        do_get_and_blocking_read_body_p2()
    }

    async fn get_and_blocking_read_body_p2_with_range(&self) -> String {
        do_get_and_blocking_read_body_p2_impl(true)
    }

    async fn get_and_blocking_read_body_p2_without_idempotence_override(&self) -> String {
        with_idempotence_mode_async(false, || async { do_get_and_blocking_read_body_p2() }).await
    }

    async fn get_and_blocking_read_body_p2_with_unbounded_retry(&self) -> String {
        let policy = NamedPolicy::named("unbounded-http-recovery-test", Policy::immediate());
        with_named_policy_async(&policy, || async { do_get_and_blocking_read_body_p2() })
            .await
            .unwrap()
    }

    async fn get_and_blocking_read_body_p2_with_delayed_retry(&self) -> String {
        let policy = NamedPolicy::named(
            "delayed-http-recovery-test",
            Policy::periodic(Duration::from_secs(2)).max_retries(2),
        );
        with_named_policy_async(&policy, || async { do_get_and_blocking_read_body_p2() })
            .await
            .unwrap()
    }

    async fn get_and_read_body_p2(&self) -> String {
        do_get_and_read_body_p2()
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

    async fn get_with_range_header(&self) -> String {
        do_get_chunked_read_with_range().await
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

    async fn post_with_status_retry_policy_p2(&self) -> String {
        let policy = NamedPolicy::named(
            "http-status-retry-test-p2",
            Policy::immediate().max_retries(10),
        )
        .applies_when(Predicate::eq(Props::STATUS_CODE, 500u16));

        with_named_policy_async(&policy, || async { do_p2_terminal_post() })
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

    async fn post_and_cancel_before_response(&self) -> String {
        do_post_and_cancel_before_response().await
    }

    async fn get_and_drop_body_after_first_chunk(&self) -> String {
        do_get_and_drop_body_after_first_chunk().await
    }

    async fn get_and_cancel_pending_body_read(&self) -> String {
        do_get_and_cancel_pending_body_read().await
    }

    async fn get_and_cancel_body_read_after_signal(&self) -> String {
        do_get_and_cancel_body_read_after_signal().await
    }

    async fn get_and_cancel_body_read_with_bytes(&self) -> String {
        do_get_and_cancel_body_read_with_bytes().await
    }

    async fn post_with_short_body_transmission_error(&self) -> String {
        do_post_with_short_body_transmission_error().await
    }

    async fn send_with_permanent_error(&mut self) -> String {
        let result = do_send_with_permanent_error().await;
        self.last_send_error = Some(result.clone());
        result
    }

    fn stored_send_error(&self) -> String {
        self.last_send_error
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    async fn get_and_store_full_response(&mut self) -> String {
        let result = do_get_full_response(false, false).await;
        self.last_full_response = Some(result.clone());
        result
    }

    async fn get_and_ignore_trailers(&mut self) -> String {
        let result = do_get_full_response(true, false).await;
        self.last_full_response = Some(result.clone());
        result
    }

    async fn get_and_cancel_body_ignoring_trailers(&mut self) -> String {
        let result = do_get_full_response(true, true).await;
        self.last_full_response = Some(result.clone());
        result
    }

    fn stored_full_response(&self) -> String {
        self.last_full_response
            .clone()
            .unwrap_or_else(|| "none".to_string())
    }

    async fn get_in_spawned_task_after_return(&self) -> String {
        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        wit_bindgen::spawn_local(async move {
            let response = wasi_fetch::Client::new()
                .get(&format!("http://localhost:{port}/spawned"))
                .send()
                .await
                .expect("Request failed");
            let _ = response
                .into_body()
                .text()
                .await
                .expect("Response body read failed");
        });
        "spawned".to_string()
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

async fn do_post_and_cancel_before_response() -> String {
    use futures_concurrency::prelude::*;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let request = async {
        let result = wasi_fetch::Client::new()
            .post(&format!("http://localhost:{port}/delayed-response"))
            .body("cancel-me")
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
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(250_000_000).await;
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

async fn do_get_and_cancel_body_read_after_signal() -> String {
    use futures_concurrency::prelude::*;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/gated-body"))
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
        // The test server withholds this response until the exact moment it
        // wants the pending chunk read above to be dropped.
        let signal = wasi_fetch::Client::new()
            .get(&format!("http://localhost:{port}/cancel-signal"))
            .send()
            .await
            .expect("Cancel-signal request failed");
        let signal_status = signal.status().as_u16();
        drop(signal);
        format!("cancelled-after-signal({status}, {signal_status})")
    };

    let result = (read, cancel).race().await;

    // The race's loser (the pending chunk read) is dropped when the winner
    // resolves; dropping the body itself then also drops the host-side reply
    // path of the still-in-flight chunk demand. By the time this request
    // reaches the test server, no bytes persisted for that demand can be
    // delivered anymore.
    drop(body);
    let done = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/cancel-done"))
        .send()
        .await
        .expect("Cancel-done request failed");
    let done_status = done.status().as_u16();
    drop(done);
    format!("{result} done={done_status}")
}

async fn do_get_and_cancel_body_read_with_bytes() -> String {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::wit_future;
    use std::future::Future;
    use std::task::Poll;

    const EXPECTED: &[u8] = b"gated-first-chunk";

    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"cancel-read".to_vec())]).unwrap();
    let (_request_done_tx, request_done_rx) = wit_future::new(|| Ok(None));
    let (request, _transmit) = types::Request::new(headers, None, request_done_rx, None);
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/gated-body")).unwrap();

    let response = client::send(request).await.expect("Request failed");
    let status = response.get_status_code();
    let (_response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut read = Box::pin(body.read(Vec::with_capacity(EXPECTED.len())));

    futures_util::future::poll_fn(|cx| match read.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready((result, buffer)) => {
            panic!("body read completed before cancellation: {result:?}, {buffer:?}")
        }
    })
    .await;

    let signal = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/cancel-signal"))
        .send()
        .await
        .expect("Cancel-signal request failed");
    let signal_status = signal.status().as_u16();
    drop(signal);

    let (result, buffer) = read.as_mut().cancel();
    assert_eq!(result, StreamResult::Complete(EXPECTED.len()));
    assert_eq!(buffer, EXPECTED);
    drop(read);
    drop(body);
    drop(trailers);

    format!(
        "cancel-read({status}, {signal_status}, {})={}",
        EXPECTED.len(),
        String::from_utf8_lossy(&buffer)
    )
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

async fn do_send_with_permanent_error() -> String {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"permanent-error".to_vec())]).unwrap();
    let (_trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    // `https` against a plain-HTTP server: the TLS handshake fails with a
    // permanent `ErrorCode` (e.g. `TlsProtocolError`), which is returned to
    // the guest instead of triggering worker-level retries.
    let (request, _transmit) = types::Request::new(headers, None, trailers_rx, None);
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Https)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();

    match client::send(request).await {
        Ok(response) => {
            let status = response.get_status_code();
            drop(response);
            format!("unexpected-success({status})")
        }
        Err(err) => format!("send-error({err:?})"),
    }
}

async fn do_get_full_response(ignore_trailers: bool, cancel_body: bool) -> String {
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::wit_future;

    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"full-response".to_vec())]).unwrap();
    let (_trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));

    let (request, _transmit) = types::Request::new(headers, None, trailers_rx, None);
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request.set_path_with_query(Some("/full-response")).unwrap();

    let response = client::send(request).await.expect("Request failed");
    let status = response.get_status_code();
    let header = response
        .get_headers()
        .get(&"x-resp-test".to_string())
        .into_iter()
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .collect::<Vec<_>>()
        .join(",");
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
                if cancel_body && n > 0 {
                    use std::future::Future;
                    use std::task::Poll;

                    let mut pending = Box::pin(body.read(Vec::with_capacity(1024)));
                    futures_util::future::poll_fn(|cx| {
                        assert!(pending.as_mut().poll(cx).is_pending());
                        Poll::Ready(())
                    })
                    .await;
                    let (result, _) = pending.as_mut().cancel();
                    assert_eq!(result, StreamResult::Cancelled);
                    break;
                }
            }
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }
    drop(body);
    if ignore_trailers {
        // Model a guest that never reads or drops the returned future handle.
        std::mem::forget(trailers);
    } else {
        trailers.await.expect("response trailers failed");
    }
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    format!(
        "status={status};x-resp-test={header};body={}",
        String::from_utf8_lossy(&body_bytes)
    )
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
                let chunk: Vec<u8> = (0..chunk_len).map(|j| ((i * 31 + j) % 251) as u8).collect();
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

async fn do_put_with_p3_body_open_until_response_read() -> String {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_bindgen::StreamResult;
    use golem_rust::wasip3::{wit_future, wit_stream};

    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"open-until-response-read".to_vec())])
            .unwrap();
    let (mut body_tx, body_rx) = wit_stream::new();
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, Some(body_rx), trailers_rx, None);
    request.set_method(&types::Method::Put).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    request
        .set_path_with_query(Some("/early-response"))
        .unwrap();

    let (send_result, remaining) = (async { client::send(request).await }, async {
        body_tx.write_all(b"hello".to_vec()).await
    })
        .join()
        .await;
    assert!(remaining.is_empty(), "request body receiver closed early");

    let response = send_result.expect("Request failed");
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, response_trailers) = types::Response::consume_body(response, response_done_rx);
    let (read_result, buffer) = body.read(Vec::with_capacity(1024)).await;
    let response_bytes = match read_result {
        StreamResult::Complete(len) => buffer[..len].to_vec(),
        StreamResult::Dropped => Vec::new(),
        StreamResult::Cancelled => panic!("response body read was cancelled"),
    };

    drop(body_tx);
    trailers_tx
        .write(Ok(None))
        .await
        .expect("failed to finish request trailers");
    let transmit_result = transmit.await;
    drop(body);
    response_trailers.await.expect("response trailers failed");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");

    format!(
        "{status} {} transmit={transmit_result:?}",
        String::from_utf8_lossy(&response_bytes)
    )
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

fn do_get_and_read_body_p2_blocking(authority: String) -> String {
    use wasi::http::{outgoing_handler, types};
    use wasi::io::streams::StreamError;

    let request = types::OutgoingRequest::new(types::Fields::new());
    request.set_method(&types::Method::Get).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&authority)).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();
    let response = outgoing_handler::handle(request, None).unwrap();
    let response = loop {
        match response.get() {
            Some(Ok(Ok(response))) => break response,
            Some(Ok(Err(error))) => panic!("HTTP response failed: {error:?}"),
            Some(Err(error)) => panic!("HTTP response failed: {error:?}"),
            None => {
                let pollable = response.subscribe();
                let _ = wasi::io::poll::poll(&[&pollable]);
            }
        }
    };
    let status = response.status();
    let body = response.consume().unwrap();
    let stream = body.stream().unwrap();
    let mut bytes = stream.blocking_read(1).unwrap();
    loop {
        match stream.blocking_read(1) {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(StreamError::Closed) => break,
            Err(error) => panic!("P2 body read failed: {error:?}"),
        }
    }
    format!("{status} {}", String::from_utf8_lossy(&bytes))
}

fn do_get_and_blocking_read_body_p2() -> String {
    do_get_and_blocking_read_body_p2_impl(false)
}

fn do_get_and_blocking_read_body_p2_impl(with_range: bool) -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let headers = if with_range {
        wasi::http::types::Fields::from_list(&[("range".to_string(), b"bytes=0-1023".to_vec())])
            .unwrap()
    } else {
        wasi::http::types::Fields::new()
    };
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request.set_method(&wasi::http::types::Method::Get).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    wasi::http::types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();

    let future = wasi::http::outgoing_handler::handle(request, None).unwrap();
    let response = get_incoming_response_p2(&future);
    let status = response.status();
    assert_eq!(status, 200, "unexpected HTTP status");
    let incoming_body = response.consume().unwrap();
    let stream = incoming_body.stream().unwrap();
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(256) {
            Ok(mut chunk) => bytes.append(&mut chunk),
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(error) => panic!("Error: {error:?}"),
        }
    }
    format!("{status} {}", String::from_utf8_lossy(&bytes))
}

fn do_get_and_read_body_p2() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let request = wasi::http::types::OutgoingRequest::new(wasi::http::types::Fields::new());
    request.set_method(&wasi::http::types::Method::Get).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();
    wasi::http::types::OutgoingBody::finish(request.body().unwrap(), None).unwrap();

    let future = wasi::http::outgoing_handler::handle(request, None).unwrap();
    let response = get_incoming_response_p2(&future);
    let status = response.status();
    assert_eq!(status, 200, "unexpected HTTP status");
    let incoming_body = response.consume().unwrap();
    let stream = incoming_body.stream().unwrap();
    let mut bytes = Vec::new();
    loop {
        match stream.read(256) {
            Ok(mut chunk) if !chunk.is_empty() => bytes.append(&mut chunk),
            Ok(_) => {
                let pollable = stream.subscribe();
                let _ = wasi::io::poll::poll(&[&pollable]);
            }
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(error) => panic!("Error: {error:?}"),
        }
    }
    format!("{status} {}", String::from_utf8_lossy(&bytes))
}

fn get_incoming_response_p2(
    future: &wasi::http::types::FutureIncomingResponse,
) -> wasi::http::types::IncomingResponse {
    match future.get() {
        Some(Ok(Ok(response))) => response,
        Some(Ok(Err(error))) => panic!("Error: {error:?}"),
        Some(Err(error)) => panic!("Error: {error:?}"),
        None => {
            let pollable = future.subscribe();
            let _ = wasi::io::poll::poll(&[&pollable]);
            get_incoming_response_p2(future)
        }
    }
}

fn do_p2_terminal_post() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());
    let headers = wasi::http::types::Fields::from_list(&[
        ("x-test".to_string(), b"test-header".to_vec()),
        ("content-length".to_string(), b"9".to_vec()),
    ])
    .unwrap();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Post)
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let body = request.body().unwrap();
    let stream = body.write().unwrap();
    let options = wasi::http::types::RequestOptions::new();
    options
        .set_first_byte_timeout(Some(30_000_000_000))
        .unwrap();
    let future = wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();
    stream.blocking_write_and_flush(b"test-body").unwrap();
    drop(stream);
    wasi::http::types::OutgoingBody::finish(body, None).unwrap();

    let response = get_incoming_response_p2(&future);
    let status = response.status();
    let body = response.consume().unwrap();
    let stream = body.stream().unwrap();
    let mut bytes = Vec::new();
    loop {
        match stream.blocking_read(256) {
            Ok(mut chunk) => bytes.append(&mut chunk),
            Err(wasi::io::streams::StreamError::Closed) => break,
            Err(error) => panic!("P2 body read failed: {error:?}"),
        }
    }
    format!("{status} {}", String::from_utf8_lossy(&bytes))
}

async fn do_get_chunked_read_with_range() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let response = wasi_fetch::Client::new()
        .get(&format!("http://localhost:{port}/"))
        .header("Range", "bytes=0-")
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
