mod bindings;

use golem_rust::bindings::golem::api::host::set_oplog_persistence_level;
use golem_rust::bindings::wasi::clocks::wall_clock::now;
use golem_rust::PersistenceLevel;
use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn run(value: u64, not_durable: bool, commit: bool) -> u64 {
        if not_durable {
            set_oplog_persistence_level(PersistenceLevel::PersistNothing);
        }

        let mut sum: u64 = 0;

        for _i in 0..value {
            let nanos = now().nanoseconds;
            sum = sum.checked_add(nanos as u64).unwrap_or(sum);
        }

        if commit {
            golem_rust::oplog_commit(1);
        }

        sum
    }
}

bindings::export!(Component with_types_in bindings);
