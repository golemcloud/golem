use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::context::Context;
use crate::host::keyvalue::error::ErrorEntry;
use crate::host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::preview2::wasi::keyvalue::readwrite::{
    Bucket, Error, Host, IncomingValue, Key, OutgoingValue,
};

#[async_trait]
impl Host for Context {
    async fn get(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<Resource<IncomingValue>, Resource<Error>>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(&bucket)?.name.clone();
        let result = self
            .key_value_service()
            .get(account_id.clone(), bucket.clone(), key.clone())
            .await;
        match result {
            Ok(Some(value)) => {
                let incoming_value = self.table_mut().push(IncomingValueEntry::new(value))?;
                Ok(Ok(incoming_value))
            }
            Ok(None) => {
                let error = self
                    .table_mut()
                    .push(ErrorEntry::new("Key not found".to_string()))?;
                Ok(Err(error))
            }
            Err(e) => {
                let error = self.table_mut().push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
        outgoing_value: Resource<OutgoingValue>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(&bucket)?.name.clone();
        let outgoing_value = self
            .table()
            .get::<OutgoingValueEntry>(&outgoing_value)?
            .body
            .read()
            .unwrap()
            .clone();
        let result = self
            .key_value_service()
            .set(
                account_id.clone(),
                bucket.clone(),
                key.clone(),
                outgoing_value.clone(),
            )
            .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.table_mut().push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(&bucket)?.name.clone();
        let result = self
            .key_value_service()
            .delete(account_id.clone(), bucket.clone(), key.clone())
            .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self.table_mut().push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<Bucket>,
        key: Key,
    ) -> anyhow::Result<Result<bool, Resource<Error>>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(&bucket)?.name.clone();
        let result = self
            .key_value_service()
            .exists(account_id.clone(), bucket.clone(), key.clone())
            .await;
        match result {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                let error = self.table_mut().push(ErrorEntry::new(format!("{:?}", e)))?;
                Ok(Err(error))
            }
        }
    }
}
