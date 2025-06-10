#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem_it::ifs_update_exports::golem_it_ifs_update_api::*;
use crate::bindings::exports::golem::api::{load_snapshot, save_snapshot};
// Import for using common lib (also see Cargo.toml for adding the dependency):
// use common_lib::example_common_function;
use std::cell::RefCell;

/// This is one of any number of data types that our application
/// uses. Golem will take care to persist all application state,
/// whether that state is local to a function being executed or
/// global across the entire program.
struct State {
    content: String
}

thread_local! {
    /// This holds the state of our application.
    static STATE: RefCell<State> = RefCell::new(State {
        content: "".to_string(),
    });
}

struct Component;

impl Guest for Component {
    fn load_file() {
        STATE.set(State { content: std::fs::read_to_string("/foo.txt").unwrap() });
    }
    fn get_file_content() -> String {
        STATE.with_borrow(|s| s.content.clone())
    }
}

impl save_snapshot::Guest for Component {
    // intentionally incorrect implementation
    fn save() -> Vec<u8> {
        Vec::new()
    }
}

impl load_snapshot::Guest for Component {
    // intentionally incorrect implementation
    fn load(_bytes: Vec<u8>) -> Result<(), String> {
        STATE.set(State { content: "restored".to_string() });
        Ok(())
    }
}

bindings::export!(Component with_types_in bindings);
