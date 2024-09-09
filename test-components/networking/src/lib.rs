mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::sockets::instance_network::*;
use crate::bindings::wasi::sockets::ip_name_lookup::*;

struct Component;

impl Guest for Component {
    fn get() -> Vec<String> {
        let network = instance_network();
        let resolve_stream = resolve_addresses(&network, "golem.cloud").expect("resolve_addresses");
        let pollable = resolve_stream.subscribe();
        pollable.block();

        let mut result = Vec::new();
        loop {
            let next = resolve_stream.resolve_next_address().expect("resolve_next_address");
            if let Some(next) = next {
                result.push(format!("{:?}", next));
            } else {
                break;
            }
        }

        result
    }
}

bindings::export!(Component with_types_in bindings);
