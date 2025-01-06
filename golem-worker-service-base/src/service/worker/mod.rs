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

pub use connect_proxy::*;
pub mod default;
pub use routing_logic::*;
pub use worker_stream::*;

mod connect_proxy;
mod error;
mod routing_logic;
mod worker_stream;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use golem_common::{model::{TargetWorkerId, ComponentFilePath}, SafeDisplay};
use golem_service_base::model::{GolemError, VersionedComponentId};
use std::fmt::{Display, Formatter};
use std::pin::Pin;
use crate::service::worker::default::{WorkerResult, WorkerRequestMetadata};

#[derive(Debug)]
pub enum WorkerServiceError {
    Internal(String),
    InvalidRequest(String),
    WorkerNotFound(String),
    InternalCallError(CallWorkerExecutorError),
    TypeChecker(String),
    FileNotFound(ComponentFilePath),
    BadFileType(ComponentFilePath),
    VersionedComponentIdNotFound(VersionedComponentId),
}

impl Display for WorkerServiceError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_safe_string())
    }
}

impl SafeDisplay for WorkerServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            WorkerServiceError::Internal(msg) => format!("Internal error: {}", msg),
            WorkerServiceError::InvalidRequest(msg) => format!("Invalid request: {}", msg),
            WorkerServiceError::WorkerNotFound(msg) => format!("Worker not found: {}", msg),
            WorkerServiceError::InternalCallError(err) => format!("Internal call error: {}", err),
            WorkerServiceError::TypeChecker(msg) => format!("Type checker error: {}", msg),
            WorkerServiceError::FileNotFound(path) => format!("File not found: {}", path),
            WorkerServiceError::BadFileType(path) => format!("Bad file type: {}", path),
            WorkerServiceError::VersionedComponentIdNotFound(id) => format!("Versioned component ID not found: {}", id),
        }
    }
}

impl From<String> for WorkerServiceError {
    fn from(err: String) -> Self {
        WorkerServiceError::Internal(err)
    }
}

impl From<GolemError> for WorkerServiceError {
    fn from(err: GolemError) -> Self {
        WorkerServiceError::Internal(err.to_string())
    }
}

impl From<CallWorkerExecutorError> for WorkerServiceError {
    fn from(err: CallWorkerExecutorError) -> Self {
        WorkerServiceError::InternalCallError(err)
    }
}

#[async_trait]
pub trait WorkerService {
    async fn get_swagger_ui_contents(
        &self,
        worker_id: &TargetWorkerId,
        mount_path: String,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>;

    async fn get_file_contents(
        &self,
        worker_id: &TargetWorkerId,
        path: ComponentFilePath,
        metadata: WorkerRequestMetadata,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>;
}
