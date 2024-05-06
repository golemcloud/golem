mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        common::echo(input)
    }

    fn calculate(input: u64) -> u64 {
        common::calculate_sum(10000, input).0
    }
}
