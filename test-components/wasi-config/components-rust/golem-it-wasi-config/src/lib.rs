#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem_it::wasi_config_exports::golem_it_wasi_config_api::*;
use crate::bindings::wasi::config::store as wasi_config;

struct Component;

impl Guest for Component {
    fn get(key: String) -> Option<String> {
        wasi_config::get(&key).unwrap()
    }
    fn get_all() -> Vec<(String, String)> {
        wasi_config::get_all().unwrap()
    }
}

bindings::export!(Component with_types_in bindings);
