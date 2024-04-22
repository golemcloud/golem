// Copyright 2024 Golem Cloud
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

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::config::{ComponentStoreLocalConfig, ComponentStoreS3Config};
use crate::stream::ByteStream;
use async_trait::async_trait;
use aws_config::BehaviorVersion;
use futures::Stream;
use tracing::{debug, info};

#[async_trait]
pub trait ComponentObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, anyhow::Error>;

    async fn get_stream(&self, object_key: &str) -> ByteStream;

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), anyhow::Error>;
}

pub struct AwsByteStream(aws_sdk_s3::primitives::ByteStream);

impl Stream for AwsByteStream {
    type Item = Result<Vec<u8>, anyhow::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0)
            .poll_next(cx)
            .map_ok(|b| b.to_vec())
            .map_err(|e| e.into())
    }
}

impl From<aws_sdk_s3::primitives::ByteStream> for ByteStream {
    fn from(stream: aws_sdk_s3::primitives::ByteStream) -> Self {
        Self::new(AwsByteStream(stream))
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
        let sdk_config = aws_config::load_defaults(BehaviorVersion::v2023_11_09()).await;
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
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, anyhow::Error> {
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

    async fn get_stream(&self, object_key: &str) -> ByteStream {
        let key = self.get_key(object_key);

        info!("Getting object: {}/{}", self.bucket_name, key);

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
        }
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), anyhow::Error> {
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
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, anyhow::Error> {
        let dir_path = self.get_dir_path();

        debug!("Getting object: {}/{}", dir_path.display(), object_key);

        let file_path = dir_path.join(object_key);

        if file_path.exists() {
            fs::read(file_path).map_err(|e| e.into())
        } else {
            Err(anyhow::Error::msg("Object not found"))
        }
    }

    async fn get_stream(&self, object_key: &str) -> ByteStream {
        let dir_path = self.get_dir_path();

        debug!("Getting object: {}/{}", dir_path.display(), object_key);

        let file_path = dir_path.join(object_key);

        match aws_sdk_s3::primitives::ByteStream::from_path(file_path).await {
            Ok(stream) => stream.into(),
            Err(error) => ByteStream::error(error),
        }
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), anyhow::Error> {
        let dir_path = self.get_dir_path();

        debug!("Putting object: {}/{}", dir_path.display(), object_key);

        if !dir_path.exists() {
            fs::create_dir_all(dir_path.clone())?;
        }

        let file_path = dir_path.join(object_key);

        fs::write(file_path, data).map_err(|e| e.into())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::ComponentStoreLocalConfig;
    use crate::service::component_object_store::{ComponentObjectStore, FsComponentObjectStore};
    use futures::TryStreamExt;

    #[tokio::test]
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
