use golem_rust::{agent_definition, agent_implementation};
use golem_rust::bindings::wasi::blobstore::blobstore;

#[agent_definition]
pub trait BlobStore {
    fn new(name: String) -> Self;
    fn create_container(&self, container_name: String);
    fn container_exists(&self, container_name: String) -> bool;
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
}
