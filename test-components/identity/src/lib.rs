mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        crate::bindings::golem::api::identity::get_token().unwrap()
    }
}

bindings::export!(Component with_types_in bindings);
