// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{anyhow, Error};
use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use golem_service_base::stream::LoggedByteStream;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, debug_span, error};
use tracing_futures::Instrument;

const COMPONENT_FILES_LABEL: &str = "component_files";

#[async_trait]
pub trait ComponentObjectStore: Debug + Send + Sync {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error>;

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, Error>>, Error>;

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error>;

    async fn delete(&self, object_key: &str) -> Result<(), Error>;
}

#[derive(Debug)]
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
    ) -> Result<BoxStream<'static, Result<Vec<u8>, Error>>, Error> {
        let span = debug_span!("Getting component stream", key = object_key);
        let inner_stream = self.store.get_stream(object_key).await?;
        let logging_stream = LoggedByteStream::new(inner_stream);
        let instrumented_stream = logging_stream.instrument(span);
        Ok(Box::pin(instrumented_stream))
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

#[derive(Debug)]
pub struct BlobStorageComponentObjectStore {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl BlobStorageComponentObjectStore {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }
}

#[async_trait]
impl ComponentObjectStore for BlobStorageComponentObjectStore {
    async fn get(&self, object_key: &str) -> Result<Vec<u8>, Error> {
        let result = self
            .blob_storage
            .get_raw(
                COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::Components,
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or(anyhow!("Did not find component for key: {object_key}"))?
            .to_vec();

        Ok(result)
    }

    async fn get_stream(
        &self,
        object_key: &str,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, Error>>, Error> {
        let result = self
            .blob_storage
            .get_stream(
                COMPONENT_FILES_LABEL,
                "get_stream",
                BlobStorageNamespace::Components,
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or(anyhow!("Did not find component for key: {object_key}"))?
            .map(|rb| rb.map(|b| b.to_vec()).map_err(|e| anyhow!(e)));

        Ok(Box::pin(result))
    }

    async fn put(&self, object_key: &str, data: Vec<u8>) -> Result<(), Error> {
        self.blob_storage
            .put_raw(
                COMPONENT_FILES_LABEL,
                "put",
                BlobStorageNamespace::Components,
                &PathBuf::from(object_key),
                &data,
            )
            .await
            .map_err(|e| anyhow!(e))
    }

    async fn delete(&self, object_key: &str) -> Result<(), Error> {
        self.blob_storage
            .delete(
                COMPONENT_FILES_LABEL,
                "delete",
                BlobStorageNamespace::Components,
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))
    }
}
