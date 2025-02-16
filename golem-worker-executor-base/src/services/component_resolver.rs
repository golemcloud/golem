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

use std::future::Future;

use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{ComponentError, GetComponentRequest, GetComponentsRequest};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::GrpcClient;
use golem_common::model::component::DefaultComponentOwner;
use golem_common::model::{ComponentId, RetryConfig};
use golem_common::retries::with_retries;
use tonic::transport::Channel;
use uuid::Uuid;
use async_trait::async_trait;
use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use std::time::Duration;


/// Used to resolve a ComponentId from a user-supplied string.
#[async_trait]
pub trait ComponentResolver: Send + Sync {
    /// Resolve a component given a user provided string. The syntax of the provided string is allowed to vary between implementations.
    /// Resolving component is the component in whoose context the resolution is being performed


    /// TODO: It would be better to require WorkerCtx::ComponentOwner here as the resolving context instead of leaving it to the
    /// implemenetation to resolve it internally from the component id (as we need to fetch the component metadata to construct the worker anyway).
    /// Problem is that requires changin the grpc interface of the component service and everything that follows from there.
    async fn resolve_component(&self, component_reference: String, resolving_component: ComponentId) -> Result<Option<ComponentId>, GolemError>;
}
