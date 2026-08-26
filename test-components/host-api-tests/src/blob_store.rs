use golem_rust::bindings::wasi::blobstore::blobstore;
use golem_rust::bindings::wasi::blobstore::types::{ObjectId, OutgoingValue};
use golem_rust::{
    PromiseId, agent_definition, agent_implementation, await_promise, create_promise, wit_stream,
};

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

async fn try_write_body(data: Vec<u8>) -> Result<OutgoingValue, String> {
    let outgoing_value = OutgoingValue::new_outgoing_value();
    let (mut writer, reader) = wit_stream::new::<u8>();
    outgoing_value
        .outgoing_value_write_body(reader)
        .map_err(|error| format!("{error:?}"))?;
    let remaining = writer.write_all(data).await;
    if !remaining.is_empty() {
        return Err("host did not consume the entire body".to_string());
    }
    drop(writer);
    Ok(outgoing_value)
}

#[agent_definition]
pub trait BlobStore {
    fn new(name: String) -> Self;
    fn create_release_promise(&self) -> PromiseId;
    fn create_container(&self, container_name: String);
    fn container_exists(&self, container_name: String) -> bool;
    async fn write_data(&self, container_name: String, object_name: String, data: Vec<u8>);
    fn get_data(&self, container_name: String, object_name: String) -> Vec<u8>;
    async fn write_object(&self, container_name: String, object_name: String, data: Vec<u8>);
    fn delete_object(&self, container_name: String, object_name: String);
    fn delete_objects(&self, container_name: String, object_names: Vec<String>);
    fn delete_container(&self, container_name: String);
    fn container_exists_result(&self, container_name: String) -> Result<bool, String>;
    fn get_data_result(
        &self,
        container_name: String,
        object_name: String,
    ) -> Result<Vec<u8>, String>;
    async fn write_data_result(
        &self,
        container_name: String,
        object_name: String,
        data: Vec<u8>,
    ) -> Result<(), String>;
    async fn consume_data_after_promise(
        &self,
        container_name: String,
        object_name: String,
        release: PromiseId,
    ) -> Result<Vec<u8>, String>;
    fn delete_object_result(
        &self,
        container_name: String,
        object_name: String,
    ) -> Result<(), String>;
    fn blobstore_probe(
        &self,
        operation: String,
        container_name: String,
        object_name: String,
        destination_container: String,
        destination_object: String,
    ) -> Result<(), String>;
    async fn container_probe(
        &self,
        operation: String,
        container_name: String,
        object_name: String,
        object_names: Vec<String>,
        data: Vec<u8>,
    ) -> Result<(), String>;
}

pub struct BlobStoreImpl {
    _name: String,
}

#[agent_implementation]
impl BlobStore for BlobStoreImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn create_release_promise(&self) -> PromiseId {
        create_promise()
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

    fn container_exists_result(&self, container_name: String) -> Result<bool, String> {
        blobstore::container_exists(&container_name).map_err(|error| format!("{error:?}"))
    }

    fn get_data_result(
        &self,
        container_name: String,
        object_name: String,
    ) -> Result<Vec<u8>, String> {
        let container =
            blobstore::get_container(&container_name).map_err(|error| format!("{error:?}"))?;
        let info = container
            .object_info(&object_name)
            .map_err(|error| format!("{error:?}"))?;
        let value = container
            .get_data(&object_name, 0, info.size)
            .map_err(|error| format!("{error:?}"))?;
        value
            .incoming_value_consume_sync()
            .map_err(|error| format!("{error:?}"))
    }

    async fn write_data_result(
        &self,
        container_name: String,
        object_name: String,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let container =
            blobstore::get_container(&container_name).map_err(|error| format!("{error:?}"))?;
        let outgoing_value = try_write_body(data).await?;
        container
            .write_data(&object_name, &outgoing_value)
            .map_err(|error| format!("{error:?}"))
    }

    async fn consume_data_after_promise(
        &self,
        container_name: String,
        object_name: String,
        release: PromiseId,
    ) -> Result<Vec<u8>, String> {
        let container =
            blobstore::get_container(&container_name).map_err(|error| format!("{error:?}"))?;
        let info = container
            .object_info(&object_name)
            .map_err(|error| format!("{error:?}"))?;
        let value = container
            .get_data(&object_name, 0, info.size)
            .map_err(|error| format!("{error:?}"))?;
        await_promise(&release).await;
        let stream = value
            .incoming_value_consume_async()
            .map_err(|error| format!("{error:?}"))?;
        Ok(stream.collect().await)
    }

    fn delete_object_result(
        &self,
        container_name: String,
        object_name: String,
    ) -> Result<(), String> {
        let container =
            blobstore::get_container(&container_name).map_err(|error| format!("{error:?}"))?;
        container
            .delete_object(&object_name)
            .map_err(|error| format!("{error:?}"))
    }

    fn blobstore_probe(
        &self,
        operation: String,
        container_name: String,
        object_name: String,
        destination_container: String,
        destination_object: String,
    ) -> Result<(), String> {
        match operation.as_str() {
            "create-container" => blobstore::create_container(&container_name).map(|_| ()),
            "get-container" => blobstore::get_container(&container_name).map(|_| ()),
            "delete-container" => blobstore::delete_container(&container_name),
            "container-exists" => blobstore::container_exists(&container_name).map(|_| ()),
            "copy-object" | "move-object" => {
                let source = ObjectId {
                    container: container_name,
                    object: object_name,
                };
                let destination = ObjectId {
                    container: destination_container,
                    object: destination_object,
                };
                if operation == "copy-object" {
                    blobstore::copy_object(&source, &destination)
                } else {
                    blobstore::move_object(&source, &destination)
                }
            }
            _ => return Err(format!("unknown blobstore operation: {operation}")),
        }
        .map_err(|error| format!("{error:?}"))
    }

    async fn container_probe(
        &self,
        operation: String,
        container_name: String,
        object_name: String,
        object_names: Vec<String>,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let container = match operation.as_str() {
            "info" | "get-data" | "has-object" | "object-info" | "list-objects" => {
                blobstore::create_container(&container_name)
            }
            _ => blobstore::get_container(&container_name),
        }
        .map_err(|error| format!("{error:?}"))?;
        match operation.as_str() {
            "info" => container.info().map(|_| ()),
            "get-data" => container.get_data(&object_name, 0, 0).map(|_| ()),
            "write-data" => {
                let outgoing_value = try_write_body(data).await?;
                container.write_data(&object_name, &outgoing_value)
            }
            "delete-object" => container.delete_object(&object_name),
            "delete-objects" => container.delete_objects(&object_names),
            "has-object" => container.has_object(&object_name).map(|_| ()),
            "object-info" => container.object_info(&object_name).map(|_| ()),
            "clear" => container.clear(),
            "list-objects" => {
                let stream = container
                    .list_objects()
                    .map_err(|error| format!("{error:?}"))?;
                stream.collect().await;
                return Ok(());
            }
            _ => return Err(format!("unknown container operation: {operation}")),
        }
        .map_err(|error| format!("{error:?}"))
    }
}
