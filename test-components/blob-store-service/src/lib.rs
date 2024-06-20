mod bindings;

use crate::bindings::exports::golem::it::api::*;
use crate::bindings::wasi::blobstore::blobstore::*;
use crate::bindings::wasi::blobstore::container::*;
use crate::bindings::wasi::blobstore::types::*;

struct Component;

impl Guest for Component {
    fn create_container(container_name: String) {
        create_container(&container_name).unwrap();
    }

    fn container_exists(container_name: String) -> bool {
        container_exists(&container_name).unwrap()
    }
}

bindings::export!(Component with_types_in bindings);
