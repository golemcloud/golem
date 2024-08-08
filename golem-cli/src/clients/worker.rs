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

use crate::model::{
    GolemError, IdempotencyKey, WorkerMetadata, WorkerName, WorkerUpdateMode,
    WorkersMetadataResponse,
};
use async_trait::async_trait;
use golem_client::model::{InvokeParameters, InvokeResult, ScanCursor, WorkerFilter, WorkerId};
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};

#[async_trait]
pub trait WorkerClient {
    async fn new_worker(
        &self,
        name: WorkerName,
        component_urn: ComponentUrn,
        args: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<WorkerId, GolemError>;

    async fn invoke_and_await(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<InvokeResult, GolemError>;

    async fn invoke(
        &self,
        worker_urn: WorkerUrn,
        function: String,
        parameters: InvokeParameters,
        idempotency_key: Option<IdempotencyKey>,
    ) -> Result<(), GolemError>;

    async fn interrupt(&self, worker_urn: WorkerUrn) -> Result<(), GolemError>;
    async fn simulated_crash(&self, worker_urn: WorkerUrn) -> Result<(), GolemError>;
    async fn delete(&self, worker_urn: WorkerUrn) -> Result<(), GolemError>;
    async fn get_metadata(&self, worker_urn: WorkerUrn) -> Result<WorkerMetadata, GolemError>;
    async fn find_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<WorkerFilter>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<WorkersMetadataResponse, GolemError>;
    async fn list_metadata(
        &self,
        component_urn: ComponentUrn,
        filter: Option<Vec<String>>,
        cursor: Option<ScanCursor>,
        count: Option<u64>,
        precise: Option<bool>,
    ) -> Result<WorkersMetadataResponse, GolemError>;
    async fn connect(&self, worker_urn: WorkerUrn) -> Result<(), GolemError>;

    async fn update(
        &self,
        worker_urn: WorkerUrn,
        mode: WorkerUpdateMode,
        target_version: u64,
    ) -> Result<(), GolemError>;
}
