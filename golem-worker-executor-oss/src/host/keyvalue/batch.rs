use async_trait::async_trait;
use wasmtime_wasi::preview2::TableError;

use crate::context::Context;
use crate::host::keyvalue::error::ErrorEntry;
use crate::host::keyvalue::types::{BucketEntry, IncomingValueEntry, OutgoingValueEntry};
use crate::preview2::wasi::keyvalue::batch::{
    Bucket, Error, Host, IncomingValue, Key, Keys, OutgoingValue,
};

#[async_trait]
impl Host for Context {
    async fn get_many(
        &mut self,
        bucket: Bucket,
        keys: Keys,
    ) -> anyhow::Result<Result<Vec<IncomingValue>, Error>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(bucket)?.name.clone();
        let result = self
            .key_value_service()
            .get_many(account_id.clone(), bucket.clone(), keys.clone())
            .await
            .map(|result| result.into_iter().collect::<Option<Vec<_>>>());
        match result {
            Ok(Some(values)) => {
                let incoming_values = values
                    .into_iter()
                    .map(|value| {
                        self.table_mut()
                            .push(Box::new(IncomingValueEntry::new(value)))
                    })
                    .collect::<Result<Vec<u32>, _>>()?;
                Ok(Ok(incoming_values))
            }
            Ok(None) => {
                let error = self
                    .table_mut()
                    .push(Box::new(ErrorEntry::new("Key not found".to_string())))?;
                Ok(Err(error))
            }
            Err(e) => {
                let error = self
                    .table_mut()
                    .push(Box::new(ErrorEntry::new(format!("{:?}", e))))?;
                Ok(Err(error))
            }
        }
    }

    async fn get_keys(&mut self, bucket: Bucket) -> anyhow::Result<Keys> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(bucket)?.name.clone();
        let keys = self
            .key_value_service()
            .get_keys(account_id.clone(), bucket.clone())
            .await?;
        Ok(keys)
    }

    async fn set_many(
        &mut self,
        bucket: Bucket,
        key_values: Vec<(Key, OutgoingValue)>,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(bucket)?.name.clone();
        let key_values = key_values
            .into_iter()
            .map(|(key, outgoing_value)| {
                let outgoing_value = self
                    .table()
                    .get::<OutgoingValueEntry>(outgoing_value)?
                    .body
                    .read()
                    .unwrap()
                    .clone();
                Ok((key, outgoing_value))
            })
            .collect::<Result<Vec<(String, Vec<u8>)>, TableError>>()?;
        let result = self
            .key_value_service()
            .set_many(account_id.clone(), bucket.clone(), key_values.clone())
            .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .table_mut()
                    .push(Box::new(ErrorEntry::new(format!("{:?}", e))))?;
                Ok(Err(error))
            }
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Bucket,
        keys: Keys,
    ) -> anyhow::Result<Result<(), Error>> {
        let account_id = self.account_id().clone();
        let bucket = self.table().get::<BucketEntry>(bucket)?.name.clone();
        let result = self
            .key_value_service()
            .delete_many(account_id.clone(), bucket.clone(), keys.clone())
            .await;
        match result {
            Ok(()) => Ok(Ok(())),
            Err(e) => {
                let error = self
                    .table_mut()
                    .push(Box::new(ErrorEntry::new(format!("{:?}", e))))?;
                Ok(Err(error))
            }
        }
    }
}
