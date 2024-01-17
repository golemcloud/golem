cargo_component_bindings::generate!();

use crate::bindings::exports::golem::it::api::Guest;

use std::fs::{read_to_string, remove_file, write};

struct Component;

impl Guest for Component {
    fn read_file(path: String) -> Result<String, String> {
        read_to_string(path).map_err(|e| e.to_string())
    }

    fn write_file(path: String, contents: String) -> Result<(), String> {
        write(path, contents).map_err(|e| e.to_string())
    }

    fn delete_file(path: String) -> Result<(), String> {
        remove_file(path).map_err(|e| e.to_string())
    }
}
