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

//! The blob store of a never-live owner. It refuses every write and lets the reads through.

use super::*;

pub(crate) struct RefusingBlobStoreService {
    pub(crate) inner: Arc<dyn BlobStoreService>,
}

#[async_trait]
impl BlobStoreService for RefusingBlobStoreService {
    async fn clear(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::clear").to_string(),
        ))
    }

    async fn container_exists(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.inner
            .container_exists(environment_id, container_name)
            .await
    }

    async fn copy_object(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _source_container_name: String,
        _source_object_name: String,
        _destination_container_name: String,
        _destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::copy_object").to_string(),
        ))
    }

    async fn create_container(
        &self,
        _environment_id: EnvironmentId,
        _container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::create_container").to_string(),
        ))
    }

    async fn delete_container(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _container_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::delete_container").to_string(),
        ))
    }

    async fn delete_object(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _container_name: String,
        _object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::delete_object").to_string(),
        ))
    }

    async fn delete_objects(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _container_name: &str,
        _object_names: &[String],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::delete_objects").to_string(),
        ))
    }

    async fn get_container(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Option<u64>, BlobStoreError> {
        self.inner
            .get_container(environment_id, container_name)
            .await
    }

    async fn get_data(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
        start: u64,
        end: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        self.inner
            .get_data(environment_id, container_name, object_name, start, end)
            .await
    }

    async fn has_object(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<bool, BlobStoreError> {
        self.inner
            .has_object(environment_id, container_name, object_name)
            .await
    }

    async fn list_objects(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
    ) -> Result<Vec<String>, BlobStoreError> {
        self.inner
            .list_objects(environment_id, container_name)
            .await
    }

    async fn move_object(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _source_container_name: String,
        _source_object_name: String,
        _destination_container_name: String,
        _destination_object_name: String,
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::move_object").to_string(),
        ))
    }

    async fn object_info(
        &self,
        environment_id: EnvironmentId,
        container_name: String,
        object_name: String,
    ) -> Result<ObjectMetadata, BlobStoreError> {
        self.inner
            .object_info(environment_id, container_name, object_name)
            .await
    }

    async fn write_data(
        &self,
        _resource_limits: Arc<AtomicResourceEntry>,
        _environment_id: EnvironmentId,
        _container_name: &str,
        _object_name: &str,
        _data: &[u8],
    ) -> Result<BlobStoreMutation, BlobStoreError> {
        Err(BlobStoreError::PermissionDenied(
            crate::durable_host::never_live::refusal("blobstore::write_data").to_string(),
        ))
    }
}
