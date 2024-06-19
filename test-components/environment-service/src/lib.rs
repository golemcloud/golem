mod bindings;

use crate::bindings::exports::golem::it::api::Guest;

use std::env::{args, vars};

struct Component;

impl Guest for Component {
    fn get_environment() -> Result<Vec<(String, String)>, String> {
        Ok(vars().collect::<Vec<_>>())
    }

    fn get_arguments() -> Result<Vec<String>, String> {
        Ok(args().collect::<Vec<_>>())
    }
}

bindings::export!(Component with_types_in bindings);
