mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn bid() -> BidResult {
        BidResult::Success
    }
}

bindings::export!(Component with_types_in bindings);
