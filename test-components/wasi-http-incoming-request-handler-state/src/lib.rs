mod bindings;

use std::sync::{LazyLock, Mutex};

pub use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};
use self::bindings::wasi::http::types::{IncomingBody, Method};

struct State {
    last: u64,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State { last: 0 }));

struct Component;

impl bindings::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        match request.method() {
            Method::Get => {
                let current = STATE.lock().unwrap().last;

                let headers = Fields::new();
                let resp = OutgoingResponse::new(headers);
                resp.set_status_code(200).unwrap();
                let body = resp.body().unwrap();
                {
                    let out = body.write().unwrap();
                    out.blocking_write_and_flush(&current.to_string().into_bytes()).unwrap();
                    drop(out);
                }

                OutgoingBody::finish(body, None).unwrap();
                ResponseOutparam::set(outparam, Ok(resp));
            },
            Method::Put => {
                let mut incoming_body_data: Vec<u8> = Vec::new();
                {
                    let incoming_body: IncomingBody = request.consume().unwrap();
                    let incoming_body_stream = incoming_body.stream().unwrap();
                    loop {
                        let item = match incoming_body_stream.blocking_read(1024) {
                            Ok(x) => x,
                            Err(_) => break,
                        };
                        if item.is_empty() {
                            break;
                        }
                        for i in item.into_iter() {
                            incoming_body_data.push(i);
                        }
                    }
                    drop(incoming_body_stream);
                    IncomingBody::finish(incoming_body);
                }
                let body_string = String::from_utf8(incoming_body_data).unwrap();
                println!("{}", body_string);
                let body_number: u64 = body_string.trim().parse().unwrap();

                STATE.lock().unwrap().last = body_number;

                let headers = Fields::new();
                let resp = OutgoingResponse::new(headers);
                resp.set_status_code(200).unwrap();
                ResponseOutparam::set(outparam, Ok(resp));
            },
            _ => panic!("unsupported method")
        }
    }
}

bindings::export!(Component with_types_in bindings);
