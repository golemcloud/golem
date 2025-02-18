mod bindings;

use crate::bindings::exports::test::caller::api::*;
use crate::bindings::golem::rpc::types::Uri;

struct Component;

impl Guest for Component {
    fn run() {
        let _api = crate::bindings::test::main_client::api_client::Iface1::new(&Uri { value: "TODO".to_string() });
    }
}

bindings::export!(Component with_types_in bindings);
