#[allow(static_mut_refs)]
#[allow(unused_imports)]
mod bindings;

use golem_rust::oplog_processor;
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::{AccountInfo, ComponentId, OplogEntry, OplogIndex, WorkerId, WorkerMetadata};
use std::collections::HashMap;

// Import for using common lib (also see Cargo.toml for adding the dependency):
// use common_lib::example_common_function;

struct Component;

struct OplogProcessor {
    account_info: AccountInfo,
    component_id: ComponentId,
    config: HashMap<String, String>
}

impl oplog_processor::exports::golem::api::oplog_processor::GuestProcessor for OplogProcessor {
    fn new(account_info: AccountInfo, component_id: ComponentId, config: Vec<(String, String)>) -> Self {
        Self {
            account_info,
            component_id,
            config: config.into_iter().collect(),
        }
    }

    fn process(&self, worker_id: WorkerId, metadata: WorkerMetadata, first_entry_index: OplogIndex, entries: Vec<OplogEntry>) -> Result<(), String> {
        Ok(())
    }
}

impl oplog_processor::exports::golem::api::oplog_processor::Guest for Component {
    type Processor = OplogProcessor;
}

oplog_processor::export_oplog_processor!(Component with_types_in oplog_processor);
