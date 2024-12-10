mod bindings;

use crate::bindings::exports::test::caller::api::*;

struct Component;

impl Guest for Component {
    fn run() {}
}

bindings::export!(Component with_types_in bindings);
