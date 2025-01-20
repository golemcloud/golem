mod bindings;

pub use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingBody, OutgoingResponse, ResponseOutparam,
};

use self::bindings::wasi::http::types::{IncomingBody, Method, Scheme};

struct Component;

impl bindings::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(request: IncomingRequest, outparam: ResponseOutparam) {
        let hdrs = Fields::new();

        for (header_name, header_value) in request.headers().entries() {
            hdrs.append(&format!("echo-{header_name}"), &header_value).unwrap();
        }

        {
            let scheme_string = match request.scheme().unwrap() {
                Scheme::Http => "http".to_string(),
                Scheme::Https => "https".to_string(),
                Scheme::Other(inner) => inner
            };

            let location_string = format!("{}://{}{}", scheme_string, request.authority().unwrap(), request.path_with_query().unwrap());
            hdrs.append(&"x-location".to_string(), &location_string.into_bytes()).unwrap();
        }

        {
            let method_string = match request.method() {
                Method::Get => "GET".to_string(),
                Method::Connect => "Connect".to_string(),
                Method::Post => "POST".to_string(),
                Method::Put => "PUT".to_string(),
                Method::Delete => "DELETE".to_string(),
                Method::Head => "HEAD".to_string(),
                Method::Options => "OPTIONS".to_string(),
                Method::Patch => "PATCH".to_string(),
                Method::Trace => "TRACE".to_string(),
                Method::Other(inner) => inner
            };
            hdrs.append(&"x-method".to_string(), &method_string.into_bytes()).unwrap();
        }


        let incoming_body: IncomingBody = request.consume().unwrap();

        let mut incoming_body_data: Vec<u8> = Vec::new();
        {
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
        }

        let mut outgoing_trailers = None;
        {
            let future_trailers = IncomingBody::finish(incoming_body);
            future_trailers.subscribe().block();
            let trailers = future_trailers.get().unwrap().unwrap().unwrap();
            if let Some(trailers) = trailers {
                let actual_outgoing_trailers = Fields::new();
                for (trailer_name, trailer_value) in trailers.entries() {
                    actual_outgoing_trailers.append(&format!("echo-{trailer_name}"), &trailer_value).unwrap();
                }
                outgoing_trailers = Some(actual_outgoing_trailers);
            }
        }

        let resp = OutgoingResponse::new(hdrs);
        let body = resp.body().unwrap();

        ResponseOutparam::set(outparam, Ok(resp));

        {
            let out = body.write().unwrap();
            out.blocking_write_and_flush(&incoming_body_data).unwrap();
            drop(out);
        }

        OutgoingBody::finish(body, outgoing_trailers).unwrap();
    }
}

bindings::export!(Component with_types_in bindings);
