mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        input
    }
}
