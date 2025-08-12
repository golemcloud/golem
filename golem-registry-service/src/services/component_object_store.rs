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

use anyhow::{Error, anyhow};
use futures::StreamExt;
use futures::stream::BoxStream;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use golem_service_base::stream::LoggedByteStream;
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, debug_span, error};
use tracing_futures::Instrument;

const COMPONENT_FILES_LABEL: &str = "component_files";

#[derive(Debug)]
pub struct ComponentObjectStore {
    blob_storage: Arc<dyn BlobStorage>,
}

impl ComponentObjectStore {
    pub fn new(blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self { blob_storage }
    }

    pub async fn get(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<Vec<u8>, Error> {
        self.logged(
            "Getting component",
            environment_id,
            object_key,
            self.get_internal(environment_id, object_key).await,
        )
    }

    async fn get_internal(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<Vec<u8>, Error> {
        let result = self
            .blob_storage
            .get_raw(
                COMPONENT_FILES_LABEL,
                "get",
                BlobStorageNamespace::Components {
                    environment_id: environment_id.clone(),
                },
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or(anyhow!("Did not find component for key: {object_key}"))?
            .to_vec();

        Ok(result)
    }

    pub async fn get_stream(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, Error>>, Error> {
        let span = debug_span!("Getting component stream", environment_id=%environment_id, key = object_key);
        let inner_stream = self.get_stream_internal(environment_id, object_key).await?;
        let logging_stream = LoggedByteStream::new(inner_stream);
        let instrumented_stream = logging_stream.instrument(span);
        Ok(Box::pin(instrumented_stream))
    }

    async fn get_stream_internal(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<BoxStream<'static, Result<Vec<u8>, Error>>, Error> {
        let result = self
            .blob_storage
            .get_stream(
                COMPONENT_FILES_LABEL,
                "get_stream",
                BlobStorageNamespace::Components {
                    environment_id: environment_id.clone(),
                },
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or(anyhow!("Did not find component for key: {object_key}"))?
            .map(|rb| rb.map(|b| b.to_vec()).map_err(|e| anyhow!(e)));

        Ok(Box::pin(result))
    }

    pub async fn put(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        self.logged(
            "Putting object",
            environment_id,
            object_key,
            self.put_internal(environment_id, object_key, data).await,
        )
    }

    async fn put_internal(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        self.blob_storage
            .put_raw(
                COMPONENT_FILES_LABEL,
                "put",
                BlobStorageNamespace::Components {
                    environment_id: environment_id.clone(),
                },
                &PathBuf::from(object_key),
                &data,
            )
            .await
            .map_err(|e| anyhow!(e))
    }

    pub async fn delete(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<(), Error> {
        self.logged(
            "Deleting object",
            environment_id,
            object_key,
            self.delete_internal(environment_id, object_key).await,
        )
    }

    async fn delete_internal(
        &self,
        environment_id: &EnvironmentId,
        object_key: &str,
    ) -> Result<(), Error> {
        self.blob_storage
            .delete(
                COMPONENT_FILES_LABEL,
                "delete",
                BlobStorageNamespace::Components {
                    environment_id: environment_id.clone(),
                },
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))
    }

    fn logged<R>(
        &self,
        message: &'static str,
        environment_id: &EnvironmentId,
        key: &str,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        match &result {
            Ok(_) => debug!(environment_id=%environment_id, key = key, "{message}"),
            Err(error) => {
                error!(environment_id=%environment_id, key = key, error = error.to_string(), "{message}")
            }
        }
        result
    }
}
