#[allow(static_mut_refs)]
mod bindings;

use bindings::{
    exports::golem::api::oplog_processor,
    golem::api::oplog::{AgentInvocation, AgentInvocationStartedParameters, OplogEntry},
};
use uuid::Uuid;

use crate::bindings::exports::golem::component::api::*;
use std::{cell::RefCell, collections::HashMap};

/// This is one of any number of data types that our application
/// uses. Golem will take care to persist all application state,
/// whether that state is local to a function being executed or
/// global across the entire program.
struct State {
    invocations: Vec<String>,
    current_invocations: HashMap<String, AgentInvocationStartedParameters>,
}

thread_local! {
    /// This holds the state of our application.
    static STATE: RefCell<State> = RefCell::new(State {
        invocations: Vec::new(),
        current_invocations: HashMap::new()
    });
}

struct Component;

impl Guest for Component {
    fn get_invoked_functions() -> Vec<String> {
        STATE.with_borrow(|state| state.invocations.clone())
    }
}

impl oplog_processor::Guest for Component {
    fn process(
        account_info: oplog_processor::AccountInfo,
        _config: Vec<(String, String)>,
        component_id: oplog_processor::ComponentId,
        worker_id: oplog_processor::AgentId,
        _metadata: oplog_processor::AgentMetadata,
        _first_entry_index: oplog_processor::OplogIndex,
        entries: Vec<oplog_processor::OplogEntry>,
    ) -> Result<(), String> {
        STATE.with_borrow_mut(|state| {
            for entry in entries {
                if let OplogEntry::AgentInvocationStarted(params) = &entry {
                    state
                        .current_invocations
                        .insert(format!("{worker_id:?}"), params.clone());
                } else if let OplogEntry::AgentInvocationFinished(_params) = &entry {
                    if let Some(invocation) =
                        state.current_invocations.get(&format!("{worker_id:?}"))
                    {
                        let account_id = Uuid::from_u64_pair(
                            account_info.account_id.uuid.high_bits,
                            account_info.account_id.uuid.low_bits,
                        );

                        let component_id = Uuid::from_u64_pair(
                            component_id.uuid.high_bits,
                            component_id.uuid.low_bits,
                        );

                        let function_name = match &invocation.invocation {
                            AgentInvocation::AgentInitialization(_) => "agent-initialization".to_string(),
                            AgentInvocation::AgentMethodInvocation(method_params) => method_params.method_name.clone(),
                            AgentInvocation::SaveSnapshot => "save-snapshot".to_string(),
                            AgentInvocation::LoadSnapshot(_) => "load-snapshot".to_string(),
                            AgentInvocation::ProcessOplogEntries(_) => "process-oplog-entries".to_string(),
                            AgentInvocation::ManualUpdate(_) => "manual-update".to_string(),
                        };

                        state.invocations.push(format!(
                            "{}/{}/{}/{}",
                            account_id,
                            component_id,
                            worker_id.agent_id,
                            function_name
                        ));
                    } else {
                        println!(
                        "AgentInvocationFinished without corresponding AgentInvocationStarted"
                    )
                    }
                }
            }
        });

        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
