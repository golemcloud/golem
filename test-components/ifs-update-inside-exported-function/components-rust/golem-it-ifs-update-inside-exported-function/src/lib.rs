#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem_it::ifs_update_inside_exported_function_exports::golem_it_ifs_update_inside_exported_function_api::*;
use crate::bindings::wasi;
use crate::bindings::wasi::io::streams::StreamError;
use wasi::http::*;
use wasi::io::*;

struct Component;

impl Guest for Component {
    fn run() -> (String, String) {
        let before = std::fs::read_to_string("/foo.txt").unwrap();
        let future_incoming_response = send_request();
        let incoming_response = get_incoming_response(&future_incoming_response);
        read_body(&incoming_response);
        let after = std::fs::read_to_string("/foo.txt").unwrap();
        (before, after)
    }
}


fn send_request() -> types::FutureIncomingResponse {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers = types::Fields::new();
    let request = types::OutgoingRequest::new(headers);
    request.set_method(&types::Method::Get).unwrap();
    request.set_path_with_query(Some("/")).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request.set_authority(Some(&format!("localhost:{port}"))).unwrap();

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_first_byte_timeout(Some(5000000000)).unwrap(); // 5s
    options.set_between_bytes_timeout(Some(5000000000)).unwrap(); // 5s

    let future_incoming_response = outgoing_handler::handle(request, Some(options)).unwrap();

    future_incoming_response
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

bindings::export!(Component with_types_in bindings);
