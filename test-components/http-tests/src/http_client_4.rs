use golem_rust::{agent_definition, agent_implementation, with_idempotence_mode};

#[agent_definition]
pub trait HttpClient4 {
    fn new() -> Self;

    /// Sends a POST request with assume_idempotence=false.
    /// POST is not idempotent, so inline retry should NOT happen.
    async fn post_non_idempotent(&self) -> String;

    /// Sends a GET request with assume_idempotence=false.
    /// GET is idempotent, so inline retry should still happen.
    async fn get_idempotent(&self) -> String;

    /// Sends a POST request with a large multi-chunk body.
    /// Used to test output stream retry when connection drops during body write.
    async fn post_large_body(&self) -> String;

    /// Sends a GET request and reads the response body in small fixed-size chunks.
    /// Used for Zone 2 retry testing.
    async fn get_and_read_body_chunked(&self) -> String;

    /// Sends a POST with a body composed of: 4 bytes "HEAD", then 1024 zero bytes
    /// via blocking_write_zeroes_and_flush, then 1024 bytes of 0xAB.
    /// Used for write_zeroes oplog reconstruction testing.
    async fn post_with_write_zeroes(&self) -> String;

    /// Sends a POST with a large body, but calls subscribe() on the output stream
    /// before writing. Used for output_stream_subscribed disqualification testing.
    async fn post_with_subscribe(&self) -> String;

    /// Sends a POST request and finishes the body with trailers.
    /// Used for has_outgoing_trailers disqualification testing.
    async fn post_with_trailers(&self) -> String;

    /// Sends a GET, then reads the response body using a mix of blocking_read and
    /// blocking_skip. Used for had_body_skip disqualification testing.
    async fn get_with_body_skip(&self) -> String;
}

struct HttpClient4Impl;

#[agent_implementation]
impl HttpClient4 for HttpClient4Impl {
    fn new() -> Self {
        Self
    }

    async fn post_non_idempotent(&self) -> String {
        with_idempotence_mode(false, || {
            do_post_request()
        })
    }

    async fn get_idempotent(&self) -> String {
        with_idempotence_mode(false, || {
            do_get_request()
        })
    }

    async fn post_large_body(&self) -> String {
        do_post_large_body()
    }

    async fn get_and_read_body_chunked(&self) -> String {
        do_get_chunked_read()
    }

    async fn post_with_write_zeroes(&self) -> String {
        do_post_with_write_zeroes()
    }

    async fn post_with_subscribe(&self) -> String {
        do_post_with_subscribe()
    }

    async fn post_with_trailers(&self) -> String {
        do_post_with_trailers()
    }

    async fn get_with_body_skip(&self) -> String {
        do_get_with_body_skip()
    }
}

fn do_post_request() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::from_list(&[(
        "X-Test".to_string(),
        "test-header".to_string().into(),
    )])
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

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write("test-body".as_bytes()).unwrap();
    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8(body).unwrap())
}

fn do_get_request() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::new();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Get)
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8(body).unwrap())
}

fn do_post_large_body() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::from_list(&[(
        "Content-Type".to_string(),
        "application/octet-stream".to_string().into(),
    )])
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

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    // Call handle() BEFORE writing body data so that hyper starts consuming
    // the body pipe. This prevents deadlock from the pipe channel filling up.
    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    // Write a large body in multiple chunks to force actual network I/O.
    // Each chunk is 64KB, total 256KB — large enough to overflow buffers
    // and force hyper to write to the TCP connection mid-stream.
    let chunk = vec![0xABu8; 64 * 1024];
    for _ in 0..4 {
        request_body_stream.blocking_write_and_flush(&chunk).unwrap();
    }
    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8_lossy(&body))
}

fn do_get_chunked_read() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::new();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Get)
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let status = incoming_response.status();

    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();
    let mut body = Vec::new();

    let mut eof = false;
    while !eof {
        match response_body_stream.blocking_read(256) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }

    format!("{} {}", status, String::from_utf8_lossy(&body))
}

fn do_post_with_write_zeroes() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::from_list(&[(
        "Content-Type".to_string(),
        "application/octet-stream".to_string().into(),
    )])
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

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    // Call handle() BEFORE writing body data so that hyper starts consuming
    // the body pipe. This prevents deadlock from the pipe channel filling up,
    // and ensures the output stream is associated with an HttpRequestState
    // for durable oplog persistence.
    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    request_body_stream
        .blocking_write_and_flush(b"HEAD")
        .unwrap();
    request_body_stream
        .blocking_write_zeroes_and_flush(1024)
        .unwrap();
    request_body_stream
        .blocking_write_and_flush(&vec![0xABu8; 1024])
        .unwrap();

    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8_lossy(&body))
}

fn do_post_with_subscribe() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::from_list(&[(
        "Content-Type".to_string(),
        "application/octet-stream".to_string().into(),
    )])
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

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();

    let pollable = request_body_stream.subscribe();
    drop(pollable);

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    // Call handle() BEFORE writing body data so that hyper starts consuming
    // the body pipe. This prevents deadlock from the pipe channel filling up.
    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let chunk = vec![0xABu8; 64 * 1024];
    for _ in 0..4 {
        request_body_stream.blocking_write_and_flush(&chunk).unwrap();
    }
    drop(request_body_stream);
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8_lossy(&body))
}

fn do_post_with_trailers() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::from_list(&[(
        "X-Test".to_string(),
        "test-header".to_string().into(),
    )])
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

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream
        .blocking_write_and_flush("test-body".as_bytes())
        .unwrap();
    drop(request_body_stream);

    let trailers = wasi::http::types::Fields::from_list(&[(
        "x-test-trailer".to_string(),
        b"trailer-value".to_vec(),
    )])
        .unwrap();
    wasi::http::types::OutgoingBody::finish(request_body, Some(trailers)).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let body = read_body(&incoming_response);
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8(body).unwrap())
}

fn do_get_with_body_skip() -> String {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = wasi::http::types::Fields::new();
    let request = wasi::http::types::OutgoingRequest::new(headers);
    request
        .set_method(&wasi::http::types::Method::Get)
        .unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request
        .set_scheme(Some(&wasi::http::types::Scheme::Http))
        .unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    wasi::http::types::OutgoingBody::finish(request_body, None).unwrap();

    let options = wasi::http::types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();

    let incoming_response = get_incoming_response(&future_incoming_response);
    let status = incoming_response.status();

    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();

    let first = response_body_stream.blocking_read(256).unwrap();
    let _ = response_body_stream.blocking_skip(256).unwrap();

    let mut body = first;
    let mut eof = false;
    while !eof {
        match response_body_stream.blocking_read(u64::MAX) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }

    format!("{} {}", status, String::from_utf8_lossy(&body))
}

fn read_body(incoming_response: &wasi::http::types::IncomingResponse) -> Vec<u8> {
    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();
    let mut body = Vec::new();

    let mut eof = false;
    while !eof {
        match response_body_stream.blocking_read(u64::MAX) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(wasi::io::streams::StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }
    body
}

fn get_incoming_response(
    future_incoming_response: &wasi::http::types::FutureIncomingResponse,
) -> wasi::http::types::IncomingResponse {
    match future_incoming_response.get() {
        Some(Ok(Ok(incoming_response))) => incoming_response,
        Some(Ok(Err(err))) => panic!("Error: {:?}", err),
        Some(Err(err)) => panic!("Error: {:?}", err),
        None => {
            let pollable = future_incoming_response.subscribe();
            let _ = wasi::io::poll::poll(&[&pollable]);
            get_incoming_response(future_incoming_response)
        }
    }
}