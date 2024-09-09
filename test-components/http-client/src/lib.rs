mod bindings;

use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::wasi;

use crate::bindings::wasi::io::streams::StreamError;
use wasi::http::*;
use wasi::io::*;

struct Component;

struct State {
    incoming_response: Option<(types::IncomingResponse, Vec<u8>, types::FutureIncomingResponse)>,
}

static mut STATE: State = State {
    incoming_response: None,
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

impl Guest for Component {
    // sends a http request and constructs a result string from the response
    fn run() -> String {
        let future_incoming_response = send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);
        process_response(incoming_response, body)
    }

    // sends a http request then another one that triggers a restart, finally constructs a result string from the response
    fn run_with_interrupt() -> String {
        let future_incoming_response = send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        let body = read_body(&incoming_response);

        send_restart_request();
        process_response(incoming_response, body)
    }

    // sends a http request and stores the response in a global variable
    fn send_request() {
        with_state(|state| {
            let future_incoming_response = send_request();
            let incoming_response = get_incoming_response(&future_incoming_response);
            let body = read_body(&incoming_response);

            state.incoming_response = Some((incoming_response, body, future_incoming_response));
        })
    }

    fn process_response() -> String {
        with_state(|state| {
            let (incoming_response, body, future_incoming_response) = state.incoming_response.take().unwrap();
            let result = process_response(incoming_response, body);
            drop(future_incoming_response); // Need to keep it alive until the end of processing
            result
        })
    }
}

fn send_request() -> types::FutureIncomingResponse {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("X-Test".to_string(), "test-header".to_string().into())])
            .unwrap();
    let request = types::OutgoingRequest::new(headers);
    request.set_method(&types::Method::Post).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&format!("localhost:{port}"))).unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write("test-body".as_bytes()).unwrap();
    drop(request_body_stream);
    types::OutgoingBody::finish(request_body, None).unwrap();

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_between_bytes_timeout(Some(5000000000)).unwrap(); // 5s

    let future_incoming_response = outgoing_handler::handle(request, Some(options)).unwrap();

    future_incoming_response
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

fn process_response(incoming_response: types::IncomingResponse, body: Vec<u8>) -> String {
    let status = incoming_response.status();

    format!("{} {}", status, String::from_utf8(body).unwrap())
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
    request.set_authority(Some(&format!("localhost:{port}"))).unwrap();

    let request_body = request.body().unwrap();
    let request_body_stream = request_body.write().unwrap();
    request_body_stream.write("test-body".as_bytes()).unwrap();
    drop(request_body_stream);
    types::OutgoingBody::finish(request_body, None).unwrap();

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_between_bytes_timeout(Some(5000000000)).unwrap(); // 5s

    let future_incoming_response = outgoing_handler::handle(request, Some(options)).unwrap();
    let _ = get_incoming_response(&future_incoming_response);
}

fn get_incoming_response(
    future_incoming_response: &types::FutureIncomingResponse,
) -> types::IncomingResponse {
    let incoming_response = match future_incoming_response.get() {
        Some(Ok(Ok(incoming_response))) => {
            println!("Got incoming response");
            incoming_response
        },
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
    };
    incoming_response
}

bindings::export!(Component with_types_in bindings);
