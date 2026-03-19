use golem_rust::{agent_definition, agent_implementation};
use golem_rust::bindings::wasi::blobstore::blobstore;
use golem_rust::bindings::wasi::blobstore::types::OutgoingValue;

#[agent_definition]
pub trait BlobStore {
    fn new(name: String) -> Self;
    fn create_container(&self, container_name: String);
    fn container_exists(&self, container_name: String) -> bool;
    fn write_data(&self, container_name: String, object_name: String, data: Vec<u8>);
    fn get_data(&self, container_name: String, object_name: String) -> Vec<u8>;
}

pub struct BlobStoreImpl {
    _name: String,
}

#[agent_implementation]
impl BlobStore for BlobStoreImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn create_container(&self, container_name: String) {
        blobstore::create_container(&container_name).unwrap();
    }

    fn container_exists(&self, container_name: String) -> bool {
        blobstore::container_exists(&container_name).unwrap()
    }

    fn write_data(&self, container_name: String, object_name: String, data: Vec<u8>) {
        let container = blobstore::get_container(&container_name).unwrap();
        let outgoing_value = OutgoingValue::new_outgoing_value();
        let body = outgoing_value.outgoing_value_write_body().unwrap();
        body.blocking_write_and_flush(&data).unwrap();
        drop(body);
        container.write_data(&object_name, &outgoing_value).unwrap();
    }

    fn get_data(&self, container_name: String, object_name: String) -> Vec<u8> {
        let container = blobstore::get_container(&container_name).unwrap();
        let info = container.object_info(&object_name).unwrap();
        let incoming_value = container.get_data(&object_name, 0, info.size).unwrap();
        incoming_value.incoming_value_consume_sync().unwrap()
    }
}
