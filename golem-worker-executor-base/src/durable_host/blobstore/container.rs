use async_trait::async_trait;
use golem_common::model::WrappedFunctionType;
use wasmtime::component::Resource;
use wasmtime_wasi::preview2::WasiView;

use crate::durable_host::blobstore::types::{
    ContainerEntry, IncomingValueEntry, OutgoingValueEntry, StreamObjectNamesEntry,
};
use crate::durable_host::serialized::SerializableError;
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::preview2::wasi::blobstore::container::{
    Container, ContainerMetadata, Error, Host, HostContainer, HostStreamObjectNames, IncomingValue,
    ObjectMetadata, ObjectName, OutgoingValue, StreamObjectNames,
};
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostContainer for DurableWorkerCtx<Ctx> {
    async fn name(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<String, Error>> {
        record_host_function_call("blobstore::container::container", "name");
        let name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        Ok(Ok(name))
    }

    async fn info(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<ContainerMetadata, Error>> {
        record_host_function_call("blobstore::container::container", "info");
        let info = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| ContainerMetadata {
                name: container_entry.name.clone(),
                created_at: container_entry.created_at,
            })?;
        Ok(Ok(info))
    }

    async fn get_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Resource<IncomingValue>, Error>> {
        record_host_function_call("blobstore::container::container", "get_data");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<Ctx, Vec<u8>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::container::get_data",
            |ctx| {
                ctx.private_state.blob_store_service.get_data(
                    account_id.clone(),
                    container_name.clone(),
                    name.clone(),
                    start,
                    end,
                )
            },
        )
        .await;
        match result {
            Ok(get_data) => {
                let incoming_value = self
                    .as_wasi_view()
                    .table_mut()
                    .push(IncomingValueEntry::new(get_data))?;
                Ok(Ok(incoming_value))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn write_data(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
        data: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::container::container", "write_data");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let data = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&data)
            .map(|outgoing_value_entry| outgoing_value_entry.body.read().unwrap().clone())?;
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::container::write_data",
            |ctx| {
                ctx.private_state.blob_store_service.write_data(
                    account_id.clone(),
                    container_name.clone(),
                    name.clone(),
                    data.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn list_objects(
        &mut self,
        container: Resource<Container>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNames>, Error>> {
        record_host_function_call("blobstore::container::container", "list_objects");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<Ctx, Vec<String>, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::container::list_objects",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .list_objects(account_id.clone(), container_name.clone())
            },
        )
        .await;
        match result {
            Ok(list_objects) => {
                let stream_object_names = self
                    .as_wasi_view()
                    .table_mut()
                    .push(StreamObjectNamesEntry::new(list_objects))?;
                Ok(Ok(stream_object_names))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::container::container", "delete_object");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::container::delete_object",
            |ctx| {
                ctx.private_state.blob_store_service.delete_object(
                    account_id.clone(),
                    container_name.clone(),
                    name.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_objects(
        &mut self,
        container: Resource<Container>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::container::container", "delete_objects");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::container::delete_objects",
            |ctx| {
                ctx.private_state.blob_store_service.delete_objects(
                    account_id.clone(),
                    container_name.clone(),
                    names.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn has_object(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool, Error>> {
        record_host_function_call("blobstore::container::container", "has_object");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<Ctx, bool, SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::container::has_object",
            |ctx| {
                ctx.private_state.blob_store_service.has_object(
                    account_id.clone(),
                    container_name.clone(),
                    name.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(has_object) => Ok(Ok(has_object)),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn object_info(
        &mut self,
        container: Resource<Container>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, Error>> {
        record_host_function_call("blobstore::container::container", "object_info");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = Durability::<
            Ctx,
            crate::services::blob_store::ObjectMetadata,
            SerializableError,
        >::wrap(
            self,
            WrappedFunctionType::ReadRemote,
            "golem blobstore::container::object_info",
            |ctx| {
                ctx.private_state.blob_store_service.object_info(
                    account_id.clone(),
                    container_name.clone(),
                    name.clone(),
                )
            },
        )
        .await;
        match result {
            Ok(object_info) => {
                let object_info = ObjectMetadata {
                    name: object_info.name,
                    container: object_info.container,
                    created_at: object_info.created_at,
                    size: object_info.size,
                };
                Ok(Ok(object_info))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn clear(&mut self, container: Resource<Container>) -> anyhow::Result<Result<(), Error>> {
        record_host_function_call("blobstore::container::container", "clear");
        let account_id = self.private_state.account_id.clone();
        let container_name = self
            .as_wasi_view()
            .table()
            .get::<ContainerEntry>(&container)
            .map(|container_entry| container_entry.name.clone())?;
        Durability::<Ctx, (), SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteRemote,
            "golem blobstore::container::clear",
            |ctx| {
                ctx.private_state
                    .blob_store_service
                    .clear(account_id.clone(), container_name.clone())
            },
        )
        .await?;
        Ok(Ok(()))
    }

    fn drop(&mut self, container: Resource<Container>) -> anyhow::Result<()> {
        record_host_function_call("blobstore::container::container", "drop");
        self.as_wasi_view()
            .table_mut()
            .delete::<ContainerEntry>(container)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostStreamObjectNames for DurableWorkerCtx<Ctx> {
    async fn read_stream_object_names(
        &mut self,
        self_: Resource<StreamObjectNames>,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool), Error>> {
        record_host_function_call(
            "blobstore::container::stream_object_names",
            "read_stream_object_names",
        );
        let names = self
            .as_wasi_view()
            .table()
            .get::<StreamObjectNamesEntry>(&self_)
            .map(|stream_object_names_entry| stream_object_names_entry.names.clone())?;
        let mut names = names.write().unwrap();
        let mut result = Vec::new();
        for _ in 0..len {
            if let Some(name) = names.pop() {
                result.push(name);
            } else {
                return Ok(Ok((result, true)));
            }
        }
        Ok(Ok((result, false)))
    }

    async fn skip_stream_object_names(
        &mut self,
        self_: Resource<StreamObjectNames>,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool), Error>> {
        record_host_function_call(
            "blobstore::container::stream_object_names",
            "skip_stream_object_names",
        );
        let names = self
            .as_wasi_view()
            .table()
            .get(&self_)
            .map(|stream_object_names_entry| stream_object_names_entry.names.clone())?;
        let mut names = names.write().unwrap();
        let mut result = 0;
        for _ in 0..num {
            if names.pop().is_some() {
                result += 1;
            } else {
                return Ok(Ok((result, true)));
            }
        }
        Ok(Ok((result, false)))
    }

    fn drop(&mut self, rep: Resource<StreamObjectNames>) -> anyhow::Result<()> {
        record_host_function_call("blobstore::container::stream_object_names", "drop");
        self.as_wasi_view().table_mut().delete(rep)?;
        Ok(())
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}
