use wasi::http::outgoing_handler;
use wasi::http::types::{
    FutureIncomingResponse, IncomingResponse, Method, OutgoingBody, OutgoingRequest, RequestOptions,
    Scheme,
};
use wasi::io::streams::StreamError;

/// Sends an outgoing HTTP request through the synchronous `wasi:http` API and
/// returns the pending response future without waiting for it.
///
/// This path is used by the synchronous host-API tests: the requests are
/// recorded durably in the oplog and the worker executor injects the Golem
/// invocation-context trace headers, neither of which is available through the
/// asynchronous `wasi-fetch` client.
pub fn send(
    method: Method,
    authority: &str,
    path: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
) -> FutureIncomingResponse {
    let headers = wasi::http::types::Fields::new();
    if let Some(content_type) = content_type {
        headers
            .set(&"content-type".to_string(), &[content_type.as_bytes().to_vec()])
            .unwrap();
    }

    let request = OutgoingRequest::new(headers);
    request.set_method(&method).unwrap();
    request.set_path_with_query(Some(path)).unwrap();
    request.set_scheme(Some(&Scheme::Http)).unwrap();
    request.set_authority(Some(authority)).unwrap();

    let request_body = request.body().unwrap();
    if let Some(bytes) = body {
        let stream = request_body.write().unwrap();
        for chunk in bytes.chunks(4096) {
            stream.blocking_write_and_flush(chunk).unwrap();
        }
        drop(stream);
    }
    OutgoingBody::finish(request_body, None).unwrap();

    let options = RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options.set_between_bytes_timeout(Some(5_000_000_000)).unwrap();

    outgoing_handler::handle(request, Some(options)).unwrap()
}

/// Blocks until the pending response future resolves and returns the incoming
/// response, panicking on transport or protocol errors.
pub fn get_incoming_response(future: &FutureIncomingResponse) -> IncomingResponse {
    match future.get() {
        Some(Ok(Ok(incoming_response))) => incoming_response,
        Some(Ok(Err(err))) => panic!("HTTP error code: {err:?}"),
        Some(Err(err)) => panic!("HTTP error: {err:?}"),
        None => {
            future.subscribe().block();
            get_incoming_response(future)
        }
    }
}

/// Reads the full body of an incoming response.
pub fn read_body(response: &IncomingResponse) -> Vec<u8> {
    let response_body = response.consume().unwrap();
    let stream = response_body.stream().unwrap();
    let mut body = Vec::new();
    loop {
        match stream.blocking_read(u64::MAX) {
            Ok(mut chunk) => body.append(&mut chunk),
            Err(StreamError::Closed) => break,
            Err(err) => panic!("Error reading body: {err:?}"),
        }
    }
    body
}

/// Sends a request and waits for the complete response, returning its status and
/// body bytes.
pub fn request(
    method: Method,
    authority: &str,
    path: &str,
    body: Option<&[u8]>,
    content_type: Option<&str>,
) -> (u16, Vec<u8>) {
    let future = send(method, authority, path, body, content_type);
    let response = get_incoming_response(&future);
    let status = response.status();
    let body = read_body(&response);
    (status, body)
}
