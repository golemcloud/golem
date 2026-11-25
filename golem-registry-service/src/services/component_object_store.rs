// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use super::run_cpu_bound_work;
use crate::repo::component::{ComponentRepo, ComponentUpload};
use anyhow::{Error, anyhow};
use futures::StreamExt;
use futures::stream::BoxStream;
use golem_common::model::account::AccountId;
use golem_common::model::diff::Hash;
use golem_service_base::repo::SqlDateTime;
use golem_service_base::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace,
};
use golem_service_base::stream::LoggedByteStream;
use sha2::{Digest, Sha256};
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, debug_span, error, warn};
use tracing_futures::Instrument;
use uuid::Uuid;

const COMPONENT_FILES_LABEL: &str = "component_files";
const STAGE_TIMEOUT: Duration = Duration::from_secs(600);
// Give an in-flight upload time to finish writing and attach before collection.
const STAGE_LEASE: chrono::TimeDelta = chrono::TimeDelta::hours(1);

#[derive(Debug)]
pub struct ComponentObjectStore {
    blob_storage: Arc<dyn BlobStorage>,
}

impl ComponentObjectStore {
    pub fn new(blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self { blob_storage }
    }

    pub async fn get(&self, account_id: AccountId, object_key: &str) -> Result<Vec<u8>, Error> {
        self.logged(
            "Getting component",
            account_id,
            object_key,
            self.get_internal(account_id, object_key).await,
        )
    }

    async fn get_internal(
        &self,
        account_id: AccountId,
        object_key: &str,
    ) -> Result<Vec<u8>, Error> {
        let result = self
            .blob_storage
            .with(COMPONENT_FILES_LABEL, "get")
            .get_raw(
                BlobStorageNamespace::ComponentUploads { account_id },
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .ok_or(anyhow!("Did not find component for key: {object_key}"))?
            .to_vec();

        Ok(result)
    }

    /// The content of the component object at `object_key`, or `None` when the store has no
    /// object at the key.
    pub async fn get_stream(
        &self,
        account_id: AccountId,
        object_key: &str,
    ) -> Result<Option<BoxStream<'static, Result<Vec<u8>, Error>>>, Error> {
        let span =
            debug_span!("Getting component stream", account_id=%account_id, key = object_key);
        Ok(self
            .get_stream_internal(account_id, object_key)
            .await?
            .map(|inner_stream| {
                let logging_stream = LoggedByteStream::new(inner_stream);
                let instrumented_stream = logging_stream.instrument(span);
                Box::pin(instrumented_stream) as BoxStream<'static, Result<Vec<u8>, Error>>
            }))
    }

    async fn get_stream_internal(
        &self,
        account_id: AccountId,
        object_key: &str,
    ) -> Result<Option<BoxStream<'static, Result<Vec<u8>, Error>>>, Error> {
        let result = self
            .blob_storage
            .with(COMPONENT_FILES_LABEL, "get_stream")
            .get_stream(
                BlobStorageNamespace::ComponentUploads { account_id },
                &PathBuf::from(object_key),
            )
            .await
            .map_err(|e| anyhow!(e))?
            .map(|stream| {
                Box::pin(stream.map(|rb| rb.map(|b| b.to_vec()).map_err(|e| anyhow!(e))))
                    as BoxStream<'static, Result<Vec<u8>, Error>>
            });

        Ok(result)
    }

    pub async fn stage(
        &self,
        repo: &dyn ComponentRepo,
        account_id: AccountId,
        data: Arc<[u8]>,
    ) -> Result<(Hash, ComponentUpload), Error> {
        let data_clone = data.clone();
        let (hash, digest) = run_cpu_bound_work(move || {
            (
                blake3::hash(&data_clone).into(),
                format!("{:x}", Sha256::digest(&data_clone)),
            )
        })
        .await;
        let object_key = format!("{digest}-{}", Uuid::new_v4());

        // Every request stages all bytes before the database decides which copy to retain.
        // The unique key also fences a retry from deletion of an older copy of the digest.
        let expires = SqlDateTime::from(chrono::Utc::now() + STAGE_LEASE);
        repo.register_upload(account_id.0, &object_key, expires)
            .await?;
        tokio::time::timeout(
            STAGE_TIMEOUT,
            self.blob_storage
                .with(COMPONENT_FILES_LABEL, "stage")
                .put_raw(
                    BlobStorageNamespace::ComponentUploads { account_id },
                    &PathBuf::from(&object_key),
                    data.as_ref(),
                ),
        )
        .await??;
        Ok((
            hash,
            ComponentUpload {
                object_key,
                digest,
                storage: self.blob_storage.clone(),
                bytes: data,
            },
        ))
    }

    pub async fn run_upload_cleanup(&self, repo: &dyn ComponentRepo) {
        let mut after = String::new();
        loop {
            match repo.collect_uploads(SqlDateTime::now(), &after).await {
                Ok((next, candidates)) => {
                    after = next;
                    for (account_id, key) in candidates {
                        match self
                            .blob_storage
                            .with(COMPONENT_FILES_LABEL, "collect")
                            .delete(
                                BlobStorageNamespace::ComponentUploads {
                                    account_id: AccountId(account_id),
                                },
                                &PathBuf::from(&key),
                            )
                            .await
                        {
                            Ok(()) => {
                                if let Err(error) = repo.confirm_upload_deleted(&key).await {
                                    warn!(account_id = %account_id, key, %error, "Could not confirm component upload deletion");
                                }
                            }
                            Err(error) => {
                                warn!(account_id = %account_id, key, %error, "Could not delete component upload")
                            }
                        }
                    }
                }
                Err(error) => {
                    warn!(%error, "Could not find unreferenced component uploads");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }
            if after.is_empty() {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }

    pub async fn abandon_upload(&self, repo: &dyn ComponentRepo, key: &str) {
        if let Err(error) = repo.abandon_upload(key).await {
            warn!(key, %error, "Could not expire failed component upload");
        }
    }

    fn logged<R>(
        &self,
        message: &'static str,
        account_id: AccountId,
        key: &str,
        result: Result<R, Error>,
    ) -> Result<R, Error> {
        match &result {
            Ok(_) => debug!(account_id=%account_id, key = key, "{message}"),
            Err(error) => {
                error!(account_id=%account_id, key = key, error = error.to_string(), "{message}")
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use test_r::test;

    /// A component object that the store no longer holds gives no stream, so the component
    /// service can report the missing binary as not found instead of an internal error.
    #[test]
    async fn a_deleted_component_object_gives_no_stream() -> anyhow::Result<()> {
        let blob_storage: Arc<dyn BlobStorage> = Arc::new(InMemoryBlobStorage::new());
        let store = ComponentObjectStore::new(blob_storage.clone());
        let account_id = AccountId::new();
        let key = "component";
        let namespace = BlobStorageNamespace::ComponentUploads { account_id };
        blob_storage
            .with(COMPONENT_FILES_LABEL, "put")
            .put_raw(namespace.clone(), &PathBuf::from(key), b"component")
            .await?;

        let stored = match store.get_stream(account_id, key).await? {
            Some(stream) => Some(
                stream
                    .collect::<Vec<_>>()
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?
                    .concat(),
            ),
            None => None,
        };
        blob_storage
            .with(COMPONENT_FILES_LABEL, "delete")
            .delete(namespace, &PathBuf::from(key))
            .await?;
        let deleted = store.get_stream(account_id, key).await?;

        assert_eq!(stored, Some(b"component".to_vec()));
        assert!(deleted.is_none());
        Ok(())
    }
}
