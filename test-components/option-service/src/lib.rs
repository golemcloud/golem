mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn echo(input: Option<String>) -> Option<String> {
        input
    }

    fn todo(input: Task) -> String {
        input.name
    }
}

bindings::export!(Component with_types_in bindings);
