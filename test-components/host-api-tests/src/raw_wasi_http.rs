use golem_rust::{agent_definition, agent_implementation};
use wasi::http::outgoing_handler;
use wasi::http::types;
use wasi::io::poll;
use wasi::io::streams::StreamError;

#[agent_definition]
pub trait RawWasiHttp {
    fn new(name: String) -> Self;
    fn run(&self) -> String;
    fn run_with_interrupt(&self) -> String;
    fn send_request(&mut self);
    fn process_response(&mut self) -> String;
}

pub struct RawWasiHttpImpl {
    _name: String,
    stored_response: Option<(Vec<u8>, u16)>,
}

#[agent_implementation]
impl RawWasiHttp for RawWasiHttpImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            stored_response: None,
        }
    }

    fn run(&self) -> String {
        let future_incoming_response = send_http_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);
        let status = incoming_response.status();
        format!("{} {}", status, String::from_utf8(body).unwrap())
    }

    fn run_with_interrupt(&self) -> String {
        let future_incoming_response = send_http_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);

        send_restart_request();
        let status = incoming_response.status();
        format!("{} {}", status, String::from_utf8(body).unwrap())
    }

    fn send_request(&mut self) {
        let future_incoming_response = send_http_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);
        let status = incoming_response.status();
        self.stored_response = Some((body, status));
    }

    fn process_response(&mut self) -> String {
        let (body, status) = self.stored_response.take().unwrap();
        format!("{} {}", status, String::from_utf8(body).unwrap())
    }
}

fn send_http_request() -> types::FutureIncomingResponse {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("X-Test".to_string(), "test-header".to_string().into())])
            .unwrap();
    let request = types::OutgoingRequest::new(headers);
    request.set_method(&types::Method::Post).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write("test-body".as_bytes()).unwrap();
    drop(request_body_stream);
    types::OutgoingBody::finish(request_body, None).unwrap();

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap();
    options.set_first_byte_timeout(Some(5000000000)).unwrap();
    options.set_between_bytes_timeout(Some(5000000000)).unwrap();

    outgoing_handler::handle(request, Some(options)).unwrap()
}

fn read_body(incoming_response: &types::IncomingResponse) -> Vec<u8> {
    let response_body = incoming_response.consume().unwrap();
    let response_body_stream = response_body.stream().unwrap();
    let mut body = Vec::new();

    let mut eof = false;
    while !eof {
        match response_body_stream.blocking_read(u64::MAX) {
            Ok(mut body_chunk) => {
                body.append(&mut body_chunk);
            }
            Err(StreamError::Closed) => {
                eof = true;
            }
            Err(err) => panic!("Error: {:?}", err),
        }
    }
    body
}

fn get_incoming_response(
    future_incoming_response: &types::FutureIncomingResponse,
) -> types::IncomingResponse {
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
            let _ = poll::poll(&[&pollable]);
            get_incoming_response(future_incoming_response)
        }
    }
}

fn send_restart_request() {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("X-Test".to_string(), "test-header".to_string().into())])
            .unwrap();
    let request = types::OutgoingRequest::new(headers);
    request.set_method(&types::Method::Post).unwrap();
    request.set_path_with_query(Some("/restart")).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write("test-body".as_bytes()).unwrap();
    drop(request_body_stream);
    types::OutgoingBody::finish(request_body, None).unwrap();

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap();
    options.set_first_byte_timeout(Some(5000000000)).unwrap();
    options.set_between_bytes_timeout(Some(5000000000)).unwrap();

    let future_incoming_response = outgoing_handler::handle(request, Some(options)).unwrap();
    let _ = get_incoming_response(&future_incoming_response);
}
