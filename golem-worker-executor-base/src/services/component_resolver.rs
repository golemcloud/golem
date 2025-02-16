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
    ///
    /// TODO: It would be better to require WorkerCtx::ComponentOwner here as the resolving context instead of leaving it to the
    /// implemenetation to resolve it internally from the component id (as we need to fetch the component metadata to construct the worker anyway).
    /// Problem is that requires changin the grpc interface of the component service and everything that follows from there.
    async fn resolve_component(&self, component_reference: String, resolving_component: ComponentId) -> Result<Option<ComponentId>, GolemError>;
}
// pub struct DefaultComponentResolver {
//     cache: Cache<String, (), Option<ComponentId>, GolemError>,
//     client: GrpcClient<ComponentServiceClient<Channel>>,
//     retry_config: RetryConfig,
//     access_token: Uuid
// }

// impl DefaultComponentResolver {

//     pub fn new(
//         client: GrpcClient<ComponentServiceClient<Channel>>,
//         access_token: Uuid,
//         max_capacity: usize,
//         time_to_idle: Duration,
//         retry_config: RetryConfig,
//     ) -> Self {
//         let cache = Cache::new(
//             Some(max_capacity),
//             FullCacheEvictionMode::LeastRecentlyUsed(1),
//             BackgroundEvictionMode::OlderThan {
//                 ttl: time_to_idle,
//                 period: Duration::from_secs(60),
//             },
//             "component",
//         );

//         Self {
//             client,
//             cache,
//             access_token,
//             retry_config
//         }
//     }

//     fn resolve_component_remotely(&self, component_reference: String) -> impl Future<Output = Result<Option<ComponentId>, GolemError>> + 'static {
//         use golem_api_grpc::proto::golem::component::v1::{get_components_response, ComponentError};

//         let client = self.client.clone();
//         let retry_config = self.retry_config.clone();
//         let access_token = self.access_token.clone();

//         async move {
//             with_retries(
//                 "component",
//                 "get_by_name",
//                 Some(component_reference.clone()),
//                 &retry_config,
//                 &(client, component_reference.clone(), access_token),
//                 |(client, component_reference, access_token)| Box::pin(async move {
//                     let response = client.call("lookup_component_by_name", move |client| {
//                         let request = authorised_grpc_request(
//                             GetComponentsRequest {
//                                 project_id: None,
//                                 component_name: Some(component_reference.clone())
//                             },
//                             access_token
//                         );
//                         Box::pin(client.get_components(request))
//                     })
//                     .await?
//                     .into_inner();

//                     match response.result.expect("Didn't receive expected field result") {
//                         get_components_response::Result::Success(payload) => {
//                             let component_id = payload.components.first().map(|c| c.versioned_component_id.expect("didn't receive expected versioned component id").component_id.expect("didn't receive expected component id"));
//                             Ok(component_id.map(|c| ComponentId::try_from(c).expect("failed to convert component id")))
//                         },
//                         get_components_response::Result::Error(err) => Err(GrpcError::Domain(err))?
//                     }
//                 }),
//                 is_grpc_retriable::<ComponentError>
//             ).await.map_err(|err| GolemError::unknown(format!("Failed to get component: {err}")))
//         }
//     }

// }

// /// Only supports resolving components based on the component name.
// #[async_trait]
// impl ComponentResolver for DefaultComponentResolver {
//     async fn resolve_component(&self, component_reference: String, _resolving_component: ComponentId) -> Result<Option<ComponentId>, GolemError> {
//         if component_reference.contains("/") {
//             Err(GolemError::invalid_request("\"/\" are not allowed in component references"))?;
//         };

//         self.cache.get_or_insert_simple(
//             &component_reference.clone(),
//             || Box::pin(self.resolve_component_remotely(component_reference))
//         ).await
//     }
// }
