use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait HttpClient {
    fn new() -> Self;

    async fn run(&self) -> String;
    async fn run_with_interrupt(&self) -> String;
    fn send_request(&mut self);
    fn process_response(&mut self) -> String;
}

struct HttpClientImpl {
    incoming_response: Option<(wasi::http::types::IncomingResponse, Vec<u8>, wasi::http::types::FutureIncomingResponse)>,
}

#[agent_implementation]
impl HttpClient for HttpClientImpl {
    fn new() -> Self {
        Self {
            incoming_response: None,
        }
    }

    async fn run(&self) -> String {
        let future_incoming_response = do_send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);
        format_response(incoming_response, body)
    }

    async fn run_with_interrupt(&self) -> String {
        let future_incoming_response = do_send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);

        send_restart_request();
        format_response(incoming_response, body)
    }

    fn send_request(&mut self) {
        let future_incoming_response = do_send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);

        self.incoming_response = Some((incoming_response, body, future_incoming_response));
    }

    fn process_response(&mut self) -> String {
        let (incoming_response, body, future_incoming_response) =
            self.incoming_response.take().unwrap();
        let result = format_response(incoming_response, body);
        drop(future_incoming_response); // Need to keep it alive until the end of processing
        result
    }
}

fn do_send_request() -> wasi::http::types::FutureIncomingResponse {
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
    options.set_connect_timeout(Some(5_000_000_000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap(); // 5s
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap(); // 5s

    wasi::http::outgoing_handler::handle(request, Some(options)).unwrap()
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

fn format_response(
    incoming_response: wasi::http::types::IncomingResponse,
    body: Vec<u8>,
) -> String {
    let status = incoming_response.status();
    format!("{} {}", status, String::from_utf8(body).unwrap())
}

fn send_restart_request() {
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
    request.set_path_with_query(Some("/restart")).unwrap();
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
    options.set_connect_timeout(Some(5_000_000_000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap(); // 5s
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap(); // 5s

    let future_incoming_response =
        wasi::http::outgoing_handler::handle(request, Some(options)).unwrap();
    let _ = get_incoming_response(&future_incoming_response);
}

fn get_incoming_response(
    future_incoming_response: &wasi::http::types::FutureIncomingResponse,
) -> wasi::http::types::IncomingResponse {
    match future_incoming_response.get() {
        Some(Ok(Ok(incoming_response))) => {
            println!("Got incoming response");
            incoming_response
        }
        Some(Ok(Err(err))) => {
            println!("Returned with error code: {err:?}");
            panic!("Error: {:?}", err)
        }
        Some(Err(err)) => {
            println!("Returned with error: {err:?}");
            panic!("Error: {:?}", err)
        }
        None => {
            println!("No incoming response yet, polling");
            let pollable = future_incoming_response.subscribe();
            let _ = wasi::io::poll::poll(&[&pollable]);
            get_incoming_response(future_incoming_response)
        }
    }
}
