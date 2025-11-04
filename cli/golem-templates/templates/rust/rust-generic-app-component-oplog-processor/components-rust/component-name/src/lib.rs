#[allow(static_mut_refs)]
#[allow(unused_imports)]
mod bindings;

use golem_rust::oplog_processor;
use golem_rust::oplog_processor::exports::golem::api::oplog_processor::{
    AccountInfo, AgentId, AgentMetadata, ComponentId, OplogEntry, OplogIndex,
};
use std::collections::HashMap;

// Import for using common lib (also see Cargo.toml for adding the dependency):
// use common_lib::example_common_function;

struct Component;

impl oplog_processor::exports::golem::api::oplog_processor::Guest for Component {
    fn process(
        account_info: AccountInfo,
        config: Vec<(String, String)>,
        component_id: ComponentId,
        agent_id: AgentId,
        metadata: AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    ) -> Result<(), String> {
        Ok(())
    }
}

oplog_processor::export_oplog_processor!(Component with_types_in oplog_processor);
