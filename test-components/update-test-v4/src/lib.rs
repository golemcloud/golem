mod bindings;

use crate::bindings::exports::golem::api::{load_snapshot};

struct Component;

impl load_snapshot::Guest for Component {
    fn load(_bytes: Vec<u8>) -> Result<(), String> {
        Err("Invalid snapshot - simulating failure".to_string())
    }
}