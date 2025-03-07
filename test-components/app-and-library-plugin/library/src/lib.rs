mod bindings;

use self::bindings::exports::it::app_and_library_plugin_library::library_api::Guest;

pub struct Component;

impl Guest for Component {
    fn library_function() -> u64 {
        1
    }
}

bindings::export!(Component with_types_in bindings);
