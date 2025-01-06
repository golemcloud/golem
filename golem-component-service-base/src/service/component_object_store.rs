// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::config::{ComponentStoreLocalConfig, ComponentStoreS3Config};
use anyhow::Error;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use futures::Stream;
use golem_service_base::stream::{ByteStream, LoggedByteStream};
use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use tracing::{debug, debug_span, error, info};
use tracing_futures::Instrument;

#[async_trait]
pub trait ComponentObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error>;

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>> + Send + Sync>>;

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error>;

    async fn delete(&self, object_key: &str) -> Result<(), Error>;
}

pub struct LoggedComponentObjectStore<Store> {
    store: Store,
}

impl<Store: ComponentObjectStore> LoggedComponentObjectStore<Store> {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    fn logged<R>(
        &self,
        message: &'static str,
        key: &str,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        match &result {
            Ok(_) => debug!(key = key, "{message}"),
            Err(error) => error!(key = key, error = error.to_string(), "{message}"),
        }
        result
    }
}

#[async_trait]
impl<Store: ComponentObjectStore + Sync> ComponentObjectStore
    for LoggedComponentObjectStore<Store>
{
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error> {
        self.logged(
            "Getting component",
            object_key,
            self.store.get(object_key).await,
        )
    }

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>> + Send + Sync>> {
        let span = debug_span!("Getting component stream", key = object_key);
        let inner_stream = self.store.get_stream(object_key).await;
        let logging_stream = LoggedByteStream::new(inner_stream);
        let instrumented_stream = logging_stream.instrument(span);
        Box::pin(instrumented_stream)
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        self.logged(
            "Putting object",
            object_key,
            self.store.put(object_key, data).await,
        )
    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        self.logged(
            "Deleting object",
            object_key,
            self.store.delete(object_key).await,
        )
    }
}

pub struct AwsS3ComponentObjectStore {
    client: aws_sdk_s3::Client,
    bucket_name: String,
    object_prefix: String,
}

impl AwsS3ComponentObjectStore {
    pub async fn new(config: &ComponentStoreS3Config) -> Self {
        info!(
            "S3 Component Object Store bucket: {}, prefix: {}",
            config.bucket_name, config.object_prefix
        );
        let sdk_config = aws_config::load_defaults(BehaviorVersion::v2024_03_28()).await;
        let client = aws_sdk_s3::Client::new(&sdk_config);
        Self {
            client,
            bucket_name: config.bucket_name.clone(),
            object_prefix: config.object_prefix.clone(),
        }
    }

    fn get_key(&self, object_key: &str) -> String {
        if self.object_prefix.is_empty() {
            object_key.to_string()
        } else {
            format!("{}/{}", self.object_prefix, object_key)
        }
    }
}

#[async_trait]
impl ComponentObjectStore for AwsS3ComponentObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error> {
        let key = self.get_key(object_key);

        info!("Getting object: {}/{}", self.bucket_name, key);

        let response = self
            .client
            .get_object()
            .bucket(&self.bucket_name)
            .key(key)
            .send()
            .await?;

        let data = response.body.collect().await?;
        Ok(data.to_vec())
    }

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>> + Send + Sync>> {
        let key = self.get_key(object_key);

        info!("Getting object: {}/{}", self.bucket_name, key);

        Box::pin(
            match self
                .client
                .get_object()
                .bucket(&self.bucket_name)
                .key(key)
                .send()
                .await
            {
                Ok(response) => response.body.into(),
                Err(error) => ByteStream::error(error),
            },
        )
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        let key = self.get_key(object_key);

        info!("Putting object: {}/{}", self.bucket_name, key);

        self.client
            .put_object()
            .bucket(&self.bucket_name)
            .key(key)
            .body(aws_sdk_s3::primitives::ByteStream::from(data))
            .send()
            .await?;

        Ok(())
    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        let key = self.get_key(object_key);

        info!("Deleting object: {}/{}", self.bucket_name, key);

        self.client
            .delete_object()
            .bucket(&self.bucket_name)
            .key(key)
            .send()
            .await?;

        Ok(())
    }
}

pub struct FsComponentObjectStore {
    root_path: String,
    object_prefix: String,
}

impl FsComponentObjectStore {
    pub fn new(config: &ComponentStoreLocalConfig) -> Result<Self, String> {
        let root_dir = std::path::PathBuf::from(config.root_path.as_str());
        if !root_dir.exists() {
            fs::create_dir_all(root_dir.clone()).map_err(|e| e.to_string())?;
        }
        info!(
            "FS Component Object Store root: {}, prefix: {}",
            root_dir.display(),
            config.object_prefix
        );

        Ok(Self {
            root_path: config.root_path.clone(),
            object_prefix: config.object_prefix.clone(),
        })
    }

    fn get_dir_path(&self) -> PathBuf {
        let root_path = std::path::PathBuf::from(self.root_path.as_str());
        if self.object_prefix.is_empty() {
            root_path
        } else {
            root_path.join(self.object_prefix.as_str())
        }
    }
}

#[async_trait]
impl ComponentObjectStore for FsComponentObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error> {
        let dir_path = self.get_dir_path();

        debug!("Getting object: {}/{}", dir_path.display(), object_key);

        let file_path = dir_path.join(object_key);

        if file_path.exists() {
            fs::read(file_path).map_err(|e| e.into())
        } else {
            Err(Error::msg("Object not found"))
        }
    }

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<u8>, Error>> + Send + Sync>> {
        let dir_path = self.get_dir_path();

        debug!("Getting object: {}/{}", dir_path.display(), object_key);

        let file_path = dir_path.join(object_key);

        Box::pin(
            match aws_sdk_s3::primitives::ByteStream::from_path(file_path).await {
                Ok(stream) => stream.into(),
                Err(error) => ByteStream::error(error),
            },
        )
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        let dir_path = self.get_dir_path();

        debug!("Putting object: {}/{}", dir_path.display(), object_key);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path.clone())?;
        }

        let file_path = dir_path.join(object_key);

        fs::write(file_path, data).map_err(|e| e.into())
    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        let dir_path = self.get_dir_path();

        debug!("Deleting object: {}/{}", dir_path.display(), object_key);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path.clone())?;
        }

        let file_path = dir_path.join(object_key);

        if file_path.exists() {
            fs::remove_file(file_path)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::config::ComponentStoreLocalConfig;
    use crate::service::component_object_store::{ComponentObjectStore, FsComponentObjectStore};
    use futures::TryStreamExt;

    #[test]
    pub async fn test_fs_object_store() {
        let config = ComponentStoreLocalConfig {
            root_path: "/tmp/cloud-service".to_string(),
            object_prefix: "prefix".to_string(),
        };

        let store = FsComponentObjectStore::new(&config).unwrap();

        let object_key = "test_object";

        let data = b"hello world".to_vec();

        store.put(object_key, data.clone()).await.unwrap();

        let get_data = store.get(object_key).await.unwrap();

        assert_eq!(get_data, data.clone());

        let stream = store.get_stream(object_key).await;
        let stream_data: Vec<Vec<u8>> = stream.try_collect::<Vec<_>>().await.unwrap();
        let stream_data: Vec<u8> = stream_data.into_iter().flatten().collect();
        assert_eq!(stream_data, data);

        let stream = store.get_stream("not_existing").await;
        let stream_data = stream.try_collect::<Vec<_>>().await;
        assert!(stream_data.is_err());
    }
}
