use std::sync::{Arc, RwLock};

use async_trait::async_trait;

use crate::context::Context;
use crate::host::blobstore::types::{ContainerEntry, OutgoingValueEntry};
use crate::preview2::wasi::blobstore::container::{
    Container, ContainerMetadata, Error, Host, IncomingValue, ObjectMetadata, ObjectName,
    OutgoingValue, StreamObjectNames,
};

#[async_trait]
impl Host for Context {
    async fn drop_container(&mut self, container: Container) -> anyhow::Result<()> {
        self.table_mut().delete::<ContainerEntry>(container)?;
        Ok(())
    }

    async fn name(&mut self, container: Container) -> anyhow::Result<Result<String, Error>> {
        let name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        Ok(Ok(name))
    }

    async fn info(
        &mut self,
        container: Container,
    ) -> anyhow::Result<Result<ContainerMetadata, Error>> {
        let info = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| ContainerMetadata {
                name: container_entry.name.clone(),
                created_at: container_entry.created_at,
            })?;
        Ok(Ok(info))
    }

    async fn get_data(
        &mut self,
        container: Container,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<IncomingValue, Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .get_data(
                account_id.clone(),
                container_name.clone(),
                name.clone(),
                start,
                end,
            )
            .await;
        match result {
            Ok(get_data) => {
                let incoming_value = self.table_mut().push(Box::new(get_data))?;
                Ok(Ok(incoming_value))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn write_data(
        &mut self,
        container: Container,
        name: ObjectName,
        data: OutgoingValue,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let data = self
            .table()
            .get::<OutgoingValueEntry>(data)
            .map(|outgoing_value_entry| outgoing_value_entry.body.read().unwrap().clone())?;
        let result = self
            .blob_store_service()
            .write_data(
                account_id.clone(),
                container_name.clone(),
                name.clone(),
                data.clone(),
            )
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn drop_stream_object_names(&mut self, names: StreamObjectNames) -> anyhow::Result<()> {
        self.table_mut().delete::<StreamObjectNamesEntry>(names)?;
        Ok(())
    }

    async fn read_stream_object_names(
        &mut self,
        this: StreamObjectNames,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool), Error>> {
        let names = self
            .table()
            .get::<StreamObjectNamesEntry>(this)
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
        this: StreamObjectNames,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool), Error>> {
        let names = self
            .table()
            .get::<StreamObjectNamesEntry>(this)
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

    async fn list_objects(
        &mut self,
        container: Container,
    ) -> anyhow::Result<Result<StreamObjectNames, Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .list_objects(account_id.clone(), container_name.clone())
            .await;
        match result {
            Ok(list_objects) => {
                let stream_object_names = self
                    .table_mut()
                    .push(Box::new(StreamObjectNamesEntry::new(list_objects)))?;
                Ok(Ok(stream_object_names))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_object(
        &mut self,
        container: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .delete_object(account_id.clone(), container_name.clone(), name.clone())
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_objects(
        &mut self,
        container: Container,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .delete_objects(account_id.clone(), container_name.clone(), names.clone())
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn has_object(
        &mut self,
        container: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool, Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .has_object(account_id.clone(), container_name.clone(), name.clone())
            .await;
        match result {
            Ok(has_object) => Ok(Ok(has_object)),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn object_info(
        &mut self,
        container: Container,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        let result = self
            .blob_store_service()
            .object_info(account_id.clone(), container_name.clone(), name.clone())
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

    async fn clear(&mut self, container: Container) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let container_name = self
            .table()
            .get::<ContainerEntry>(container)
            .map(|container_entry| container_entry.name.clone())?;
        self.blob_store_service()
            .clear(account_id.clone(), container_name.clone())
            .await?;
        Ok(Ok(()))
    }
}

struct StreamObjectNamesEntry {
    names: Arc<RwLock<Vec<String>>>,
}

impl StreamObjectNamesEntry {
    fn new(names: Vec<String>) -> Self {
        Self {
            names: Arc::new(RwLock::new(names)),
        }
    }
}
