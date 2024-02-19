mod bindings;

use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {
    fn get_self_uri(function_name: String) -> String {
        get_self_uri(&function_name).value
    }
}
