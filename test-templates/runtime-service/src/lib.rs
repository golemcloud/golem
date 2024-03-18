mod bindings;

use crate::bindings::exports::golem::it::api::Guest;
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {
    fn get_self_uri(function_name: String) -> String {
        get_self_uri(&function_name).value
    }

    fn jump() -> u64 {
        let mut state = 0;

        println!("started: {state}"); // 'started 1'
        state += 1;

        let state1 = get_oplog_index();

        state += 1;
        println!("second: {state}"); // 'second 2'

        set_oplog_index(state1); // we resume from state 1 so we emit 'second 2' again but not 'started 1'

        state += 1;
        println!("third: {state}"); // 'third 3'

        let state2 = get_oplog_index();

        state += 1;
        println!("fourth: {state}"); // 'fourth 4'

        set_oplog_index(state2); // we resume from state 2, so emit 'fourth 4' again but not the rest

        state += 1;
        println!("fifth: {state}"); // 'fifth 5'

        // Expected final output:
        // started 1
        // second 2
        // second 2
        // third 3
        // fourth 4
        // fourth 4
        // fifth 5

        state // final value is 5
    }
}
