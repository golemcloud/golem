#[allow(static_mut_refs)]
mod bindings;

use bindings::wasi::http::types::{
    Fields, IncomingRequest, OutgoingResponse, ResponseOutparam,
};

struct Component;

impl bindings::exports::wasi::http::incoming_handler::Guest for Component {
    fn handle(_request: IncomingRequest, outparam: ResponseOutparam) {
        let hdrs = Fields::new();
        let resp = OutgoingResponse::new(hdrs);
        resp.set_status_code(200).unwrap();

        ResponseOutparam::set(outparam, Ok(resp));
    }
}

bindings::export!(Component with_types_in bindings);
