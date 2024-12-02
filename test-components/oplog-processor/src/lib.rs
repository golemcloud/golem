mod bindings;

use bindings::{
    exports::golem::api::oplog_processor::{self, GuestProcessor},
    golem::api::oplog::{ExportedFunctionInvokedParameters, OplogEntry},
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
}

thread_local! {
    /// This holds the state of our application.
    static STATE: RefCell<State> = RefCell::new(State {
        invocations: Vec::new(),
    });
}

struct Component;

impl Guest for Component {
    fn get_invoked_functions() -> Vec<String> {
        STATE.with_borrow(|state| state.invocations.clone())
    }
}

impl oplog_processor::Guest for Component {
    type Processor = OplogProcessor;
}

struct OplogProcessor {
    account_id: oplog_processor::AccountId,
    component_id: oplog_processor::ComponentId,
    _config: HashMap<String, String>,
    current_invocation: RefCell<Option<ExportedFunctionInvokedParameters>>,
}

impl GuestProcessor for OplogProcessor {
    fn new(
        account_info: oplog_processor::AccountInfo,
        component_id: oplog_processor::ComponentId,
        config: Vec<(String, String)>,
    ) -> Self {
        Self {
            account_id: account_info.account_id,
            component_id,
            _config: config.into_iter().collect(),
            current_invocation: RefCell::new(None),
        }
    }

    fn process(
        &self,
        worker_id: oplog_processor::WorkerId,
        _metadata: oplog_processor::WorkerMetadata,
        _first_entry_index: oplog_processor::OplogIndex,
        entries: Vec<oplog_processor::OplogEntry>,
    ) -> Result<(), String> {
        for entry in entries {
            if let OplogEntry::ExportedFunctionInvoked(params) = &entry {
                *self.current_invocation.borrow_mut() = Some(params.clone());
            } else if let OplogEntry::ExportedFunctionCompleted(_params) = &entry {
                if let Some(invocation) = self.current_invocation.borrow_mut().take() {
                    let component_id = Uuid::from_u64_pair(
                        self.component_id.uuid.high_bits,
                        self.component_id.uuid.low_bits,
                    );

                    STATE.with_borrow_mut(|state| {
                        state.invocations.push(format!(
                            "{}/{:?}{}/{}",
                            self.account_id.value,
                            component_id,
                            worker_id.worker_name,
                            invocation.function_name
                        ));
                    });
                } else {
                    println!(
                        "ExportedFunctionCompleted without corresponding ExportedFunctionInvoked"
                    )
                }
            }
        }

        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
