mod bindings;

use self::bindings::exports::it::app_and_library_plugin_app::app_api::Guest;
use self::bindings::it::app_and_library_plugin_library::library_api::library_function;

struct Component;

impl Guest for Component {

    fn app_function() -> u64 {
        library_function() + 1
    }
}

bindings::export!(Component with_types_in bindings);
