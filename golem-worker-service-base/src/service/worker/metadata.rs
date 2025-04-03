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

use golem_common::model::oplog::WorkerError;
use golem_service_base::model::ResourceLimits;

#[derive(Clone, Debug)]
pub struct WorkerRequestMetadata {
    pub limits: Option<ResourceLimits>,
}

#[async_trait::async_trait]
pub trait WorkerRequestMetadataResolver<Namespace>: Send + Sync + 'static {
    async fn resolve_metadata(&self, namespace: &Namespace) -> Result<WorkerRequestMetadata, WorkerError>;
}

pub struct EmptyWorkerRequestMetadataResolver;

#[async_trait::async_trait]
impl<Namespace> WorkerRequestMetadataResolver<Namespace> for EmptyWorkerRequestMetadataResolver {
    async fn resolve_metadata(&self, _namespace: &Namespace) -> Result<WorkerRequestMetadata, WorkerError> {
        Ok(WorkerRequestMetadata {
            limits: None
        })
    }
}
