use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::context::Context;
use crate::host::blobstore::types::ContainerEntry;
use crate::preview2::wasi::blobstore::blobstore::{
    Container, ContainerName, Error, Host, ObjectId,
};

pub mod container;
pub mod types;

#[async_trait]
impl Host for Context {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .create_container(account_id.clone(), name.clone())
            .await;

        match result {
            Ok(created_at) => {
                let container = self
                    .table_mut()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<Container>, Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .get_container(account_id.clone(), name.clone())
            .await;

        match result {
            Ok(Some(created_at)) => {
                let container = self
                    .table_mut()
                    .push(ContainerEntry::new(name, created_at))?;
                Ok(Ok(container))
            }
            Ok(None) => Ok(Err("Container not found".to_string())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn delete_container(&mut self, name: ContainerName) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .delete_container(account_id.clone(), name.clone())
            .await;

        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .container_exists(account_id.clone(), name.clone())
            .await;
        match result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .copy_object(
                account_id.clone(),
                src.container.clone(),
                src.object.clone(),
                dest.container.clone(),
                dest.object.clone(),
            )
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }

    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let result = self
            .blob_store_service()
            .move_object(
                account_id.clone(),
                src.container.clone(),
                src.object.clone(),
                dest.container.clone(),
                dest.object.clone(),
            )
            .await;
        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(format!("{:?}", e))),
        }
    }
}
