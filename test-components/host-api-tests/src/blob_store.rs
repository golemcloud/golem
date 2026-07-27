use golem_rust::bindings::wasi::blobstore::blobstore;
use golem_rust::bindings::wasi::blobstore::types::OutgoingValue;
use golem_rust::{agent_definition, agent_implementation, wit_stream};

/// Writes `data` into a fresh outgoing value using the WASI P3 stream-based
/// `outgoing-value-write-body` interface: the guest creates a `stream<u8>`,
/// hands the readable end to the host, then writes the bytes into the writable
/// end. The host consumes the stream into the outgoing value's body buffer.
async fn write_body(data: Vec<u8>) -> OutgoingValue {
    let outgoing_value = OutgoingValue::new_outgoing_value();
    let (mut writer, reader) = wit_stream::new::<u8>();
    outgoing_value.outgoing_value_write_body(reader).unwrap();
    let remaining = writer.write_all(data).await;
    assert!(remaining.is_empty(), "host did not consume the entire body");
    drop(writer);
    outgoing_value
}

#[agent_definition]
pub trait BlobStore {
    fn new(name: String) -> Self;
    fn create_container(&self, container_name: String);
    fn container_exists(&self, container_name: String) -> bool;
    async fn write_data(&self, container_name: String, object_name: String, data: Vec<u8>);
    fn get_data(&self, container_name: String, object_name: String) -> Vec<u8>;
    async fn write_object(&self, container_name: String, object_name: String, data: Vec<u8>);
    fn delete_object(&self, container_name: String, object_name: String);
    fn delete_objects(&self, container_name: String, object_names: Vec<String>);
    fn delete_container(&self, container_name: String);
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

    async fn write_data(&self, container_name: String, object_name: String, data: Vec<u8>) {
        let container = blobstore::get_container(&container_name).unwrap();
        let outgoing_value = write_body(data).await;
        container.write_data(&object_name, &outgoing_value).unwrap();
    }

    fn get_data(&self, container_name: String, object_name: String) -> Vec<u8> {
        let container = blobstore::get_container(&container_name).unwrap();
        let info = container.object_info(&object_name).unwrap();
        let incoming_value = container.get_data(&object_name, 0, info.size).unwrap();
        incoming_value.incoming_value_consume_sync().unwrap()
    }

    async fn write_object(&self, container_name: String, object_name: String, data: Vec<u8>) {
        let container = blobstore::get_container(&container_name).unwrap();
        let outgoing_value = write_body(data).await;
        container.write_data(&object_name, &outgoing_value).unwrap();
    }

    fn delete_object(&self, container_name: String, object_name: String) {
        let container = blobstore::get_container(&container_name).unwrap();
        container.delete_object(&object_name).unwrap();
    }

    fn delete_objects(&self, container_name: String, object_names: Vec<String>) {
        let container = blobstore::get_container(&container_name).unwrap();
        container.delete_objects(&object_names).unwrap();
    }

    fn delete_container(&self, container_name: String) {
        blobstore::delete_container(&container_name).unwrap();
    }
}
