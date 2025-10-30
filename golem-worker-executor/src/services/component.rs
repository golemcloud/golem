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

use super::golem_config::{ComponentCacheConfig, ComponentServiceConfig};
use crate::metrics::component::record_compilation_time;
use async_trait::async_trait;
use futures::TryStreamExt;
use golem_common::cache::SimpleCache;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::component::{ComponentDto, ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::RetryConfig;
use golem_common::retries::with_retries;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::service::compiled_component::CompiledComponentService;
use golem_service_base::service::compiled_component::CompiledComponentServiceConfig;
use golem_service_base::storage::blob::BlobStorage;
use http::Uri;
use prost::Message;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::task::spawn_blocking;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::info;
use tracing::{debug, warn};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;
use golem_service_base::clients::registry::{GrpcRegistryService, RegistryService};

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService: Send + Sync {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError>;

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        forced_version: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError>;

    /// Resolve a component given a user-provided string. The syntax of the provided string is allowed to vary between implementations.
    /// Resolving component is the component in whose context the resolution is being performed
    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_environment: EnvironmentId,
        resolving_application: ApplicationId,
        resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError>;

    /// Returns all the component metadata the implementation has cached.
    /// This is useful for some mock/local implementations.
    async fn all_cached_metadata(&self) -> Vec<ComponentDto>;
}

pub fn configured(
    config: &ComponentServiceConfig,
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage>,
) -> Arc<dyn ComponentService> {
    let compiled_component_service =
        golem_service_base::service::compiled_component::configured(compiled_config, blob_storage);

    info!("Using component API at {}", config.url());
    Arc::new(ComponentServiceDefault::new(
        config.uri(),
        config
            .access_token
            .parse::<Uuid>()
            .expect("Access token must be an UUID"),
        cache_config.max_capacity,
        cache_config.max_metadata_capacity,
        cache_config.max_resolved_component_capacity,
        cache_config.time_to_idle,
        config.retries.clone(),
        config.connect_timeout,
        compiled_component_service,
        config.max_component_size,
    ))
}

pub struct ComponentServiceDefault {
    component_cache: Cache<ComponentKey, (), Component, WorkerExecutorError>,
    component_metadata_cache: Cache<ComponentKey, (), ComponentDto, WorkerExecutorError>,
    resolved_component_cache:
        Cache<(EnvironmentId, String), (), Option<ComponentId>, WorkerExecutorError>,
    access_token: Uuid,
    retry_config: RetryConfig,
    compiled_component_service: Arc<dyn CompiledComponentService>,
    component_client: GrpcRegistryService,
}

impl ComponentServiceDefault {
    pub fn new(
        component_endpoint: Uri,
        access_token: Uuid,
        max_component_capacity: usize,
        max_metadata_capacity: usize,
        max_resolved_component_capacity: usize,
        time_to_idle: Duration,
        retry_config: RetryConfig,
        connect_timeout: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService>,
        max_component_size: usize,
    ) -> Self {
        Self {
            component_cache: create_component_cache(max_component_capacity, time_to_idle),
            component_metadata_cache: create_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            resolved_component_cache: create_resolved_component_cache(
                max_resolved_component_capacity,
                time_to_idle,
            ),
            access_token,
            retry_config: retry_config.clone(),
            compiled_component_service,
            component_client: GrpcClient::new(
                "component_service",
                move |channel| {
                    ComponentServiceClient::new(channel)
                        .max_decoding_message_size(max_component_size)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                component_endpoint,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    connect_timeout,
                },
            ),
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceDefault {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: ComponentRevision,
    ) -> Result<(Component, ComponentDto), WorkerExecutorError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id_clone = component_id.clone();
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let metadata = self
            .get_metadata(component_id, Some(component_version))
            .await?;
        let environment_id_clone = metadata.environment_id.clone();

        let component = self
            .component_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_component_service
                        .get(
                            &environment_id_clone,
                            &component_id_clone,
                            component_version,
                            &engine,
                        )
                        .await;

                    let component = match result {
                        Ok(component) => component,
                        Err(err) => {
                            warn!("Failed to download compiled component {:?}: {}", key, err);
                            None
                        }
                    };

                    match component {
                        Some(component) => Ok(component),
                        None => {
                            let bytes = self.component_client.download_component(&component_id_clone, component_version, &AuthCtx::System).await.map_err(|e|
                                WorkerExecutorError::ComponentDownloadFailed {
                                    component_id: component_id_clone.clone(),
                                    component_version,
                                    reason: format!("{e}"),
                                }
                            )?;

                            let start = Instant::now();
                            let component_id_clone2 = component_id_clone.clone();
                            let component = spawn_blocking(move || {
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    WorkerExecutorError::ComponentParseFailed {
                                        component_id: component_id_clone2,
                                        component_version,
                                        reason: format!("{e}"),
                                    }
                                })
                            })
                            .await
                            .map_err(|join_err| {
                                WorkerExecutorError::unknown(join_err.to_string())
                            })??;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                component_id_clone,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_component_service
                                .put(
                                    &environment_id_clone,
                                    &component_id_clone,
                                    component_version,
                                    &component,
                                )
                                .await;

                            match result {
                                Ok(_) => Ok(component),
                                Err(err) => {
                                    warn!("Failed to upload compiled component {:?}: {}", key, err);
                                    Ok(component)
                                }
                            }
                        }
                    }
                })
            })
            .await?;

        Ok((component, metadata))
    }

    async fn get_metadata(
        &self,
        component_id: &ComponentId,
        forced_version: Option<ComponentRevision>,
    ) -> Result<ComponentDto, WorkerExecutorError> {
        match forced_version {
            Some(version) => {
                let client = self.component_client.clone();
                let access_token = self.access_token;
                let retry_config = self.retry_config.clone();
                let component_id = component_id.clone();
                self.component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id: component_id.clone(),
                            component_version: version,
                        },
                        || {
                            Box::pin(async move {
                                let metadata = client.get_component_metadata(&component_id, version, &AuthCtx::System).await.map_err(|e| WorkerExecutorError::runtime(format!("Failed getting component metadata: {e}")))?;
                                Ok(metadata)
                            })
                        },
                    )
                    .await
            }
            None => {
                let metadata = self.component_client.get_latest_component_metadata(&component_id, &AuthCtx::System).await.map_err(|e| WorkerExecutorError::runtime(format!("Failed getting component metadata: {e}")))?;

                let metadata = self
                    .component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id: component_id.clone(),
                            component_version: metadata.revision,
                        },
                        || Box::pin(async move { Ok(metadata) }),
                    )
                    .await?;

                Ok(metadata)
            }
        }
    }

    async fn resolve_component(
        &self,
        component_slug: String,
        resolving_environment: EnvironmentId,
        resolving_application: ApplicationId,
        resolving_account: AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        let component = self.component_client.resolve_component(
            &resolving_account,
            &resolving_application,
            &resolving_environment,
            &component_slug,
            &AuthCtx::System
        )
        .await
        .map_err(|e| WorkerExecutorError::runtime(format!("Resolving component failed: {e}")))?;

        Ok(component.map(|c| c.id))
    }

    async fn all_cached_metadata(&self) -> Vec<ComponentDto> {
        self.component_metadata_cache
            .iter()
            .await
            .into_iter()
            .map(|(_, v)| v)
            .collect()
    }
}

// async fn download_via_grpc(
//     client: &GrpcClient<ComponentServiceClient<Channel>>,
//     access_token: &Uuid,
//     retry_config: &RetryConfig,
//     component_id: &ComponentId,
//     component_version: ComponentRevision,
// ) -> Result<Vec<u8>, WorkerExecutorError> {
//     with_retries(
//         "components",
//         "download",
//         Some(component_id.to_string()),
//         retry_config,
//         &(
//             client.clone(),
//             component_id.clone(),
//             access_token.to_owned(),
//         ),
//         |(client, component_id, access_token)| {
//             Box::pin(async move {
//                 let response = client
//                     .call("download_component", move |client| {
//                         let request = authorised_grpc_request(
//                             DownloadComponentRequest {
//                                 component_id: Some(component_id.clone().into()),
//                                 version: Some(component_version.0),
//                                 auth_ctx: Some(AuthCtx::System.into()),
//                             },
//                             access_token,
//                         );
//                         Box::pin(client.download_component(request))
//                     })
//                     .await?
//                     .into_inner();

//                 let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
//                 let bytes = chunks
//                     .into_iter()
//                     .map(|chunk| match chunk.result {
//                         None => Err("Empty response".to_string().into()),
//                         Some(download_component_response::Result::SuccessChunk(chunk)) => Ok(chunk),
//                         Some(download_component_response::Result::Error(error)) => {
//                             Err(GrpcError::Domain(error))
//                         }
//                     })
//                     .collect::<Result<Vec<Vec<u8>>, GrpcError<ComponentError>>>()?;

//                 let bytes: Vec<u8> = bytes.into_iter().flatten().collect();

//                 record_external_call_response_size_bytes("components", "download", bytes.len());

//                 Ok(bytes)
//             })
//         },
//         is_grpc_retriable::<ComponentError>,
//     )
//     .await
//     .map_err(|error| grpc_component_download_error(error, component_id, component_version))
// }

// async fn get_metadata_via_grpc(
//     client: &GrpcClient<ComponentServiceClient<Channel>>,
//     access_token: &Uuid,
//     retry_config: &RetryConfig,
//     component_id: &ComponentId,
//     component_version: Option<ComponentRevision>,
// ) -> Result<ComponentDto, WorkerExecutorError> {
//     let desc = format!("Getting component metadata of {component_id}");
//     debug!("{}", &desc);
//     with_retries(
//         "components",
//         "get_metadata",
//         Some(component_id.to_string()),
//         retry_config,
//         &(
//             client.clone(),
//             component_id.clone(),
//             access_token.to_owned(),
//         ),
//         |(client, component_id, access_token)| {
//             Box::pin(async move {
//                 let response = match component_version {
//                     Some(component_version) => client
//                         .call("get_component_metadata", move |client| {
//                             let request = authorised_grpc_request(
//                                 GetVersionedComponentRequest {
//                                     component_id: Some(component_id.clone().into()),
//                                     version: component_version.0,
//                                     auth_ctx: Some(AuthCtx::System.into()),
//                                 },
//                                 access_token,
//                             );
//                             Box::pin(client.get_component_metadata(request))
//                         })
//                         .await?
//                         .into_inner(),
//                     None => client
//                         .call("get_latest_component_metadata", move |client| {
//                             let request = authorised_grpc_request(
//                                 GetLatestComponentRequest {
//                                     component_id: Some(component_id.clone().into()),
//                                     auth_ctx: Some(AuthCtx::System.into()),
//                                 },
//                                 access_token,
//                             );
//                             Box::pin(client.get_latest_component_metadata(request))
//                         })
//                         .await?
//                         .into_inner(),
//                 };
//                 let len = response.encoded_len();
//                 let component = match response.result {
//                     None => Err("Empty response".to_string().into()),
//                     Some(get_component_metadata_response::Result::Success(response)) => {
//                         Ok(response.component.ok_or(GrpcError::Unexpected(
//                             "No component information in response".to_string(),
//                         ))?)
//                     }
//                     Some(get_component_metadata_response::Result::Error(error)) => {
//                         Err(GrpcError::Domain(error))
//                     }
//                 }?;

//                 let result = component.try_into()?;
//                 record_external_call_response_size_bytes("components", "get_metadata", len);

//                 Ok(result)
//             })
//         },
//         is_grpc_retriable::<ComponentError>,
//     )
//     .await
//     .map_err(|error| grpc_get_latest_version_error(error, component_id))
// }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: ComponentRevision,
}

fn create_component_metadata_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), ComponentDto, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "component_metadata",
    )
}

fn create_resolved_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<(EnvironmentId, String), (), Option<ComponentId>, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "resolved_component",
    )
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), Component, WorkerExecutorError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "component",
    )
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct ComponentSlug {
    account_email: Option<String>,
    application_name: Option<String>,
    environment_name: Option<String>,
    component_name: String,
}

impl ComponentSlug {
    pub fn parse(str: &str) -> Result<Self, String> {
        // TODO: We probably want more validations here.
        if str.is_empty() {
            Err("Empty component references are not allowed")?;
        };

        let mut parts = str.split("/").collect::<Vec<_>>();

        if parts.is_empty() || parts.len() > 4 {
            Err("Unexpected number of \"/\"-delimited parts in component reference")?
        };

        if parts.iter().any(|p| p.is_empty()) {
            Err("Empty part in the component reference")?
        };

        parts.reverse();

        Ok(ComponentSlug {
            account_email: parts.get(3).map(|s| s.to_string()),
            application_name: parts.get(2).map(|s| s.to_string()),
            environment_name: parts.get(1).map(|s| s.to_string()),
            component_name: parts[0].to_string(), // safe due to the check above
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentSlug;
    use test_r::test;

    #[test]
    fn parse_component() {
        let res = ComponentSlug::parse("foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: None,
                environment_name: None,
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_environment_component() {
        let res = ComponentSlug::parse("bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: None,
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_application_environment_component() {
        let res = ComponentSlug::parse("foo/bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: Some("foo".to_string()),
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_account_application_environment_component() {
        let res = ComponentSlug::parse("foo@golem.cloud/foo/bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: Some("foo@golem.cloud".to_string()),
                application_name: Some("foo".to_string()),
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn reject_longer() {
        let res = ComponentSlug::parse("toolong/foo@golem.cloud/foo/bar/foobar");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty() {
        let res = ComponentSlug::parse("");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty_group_1() {
        let res = ComponentSlug::parse("foo/");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty_group_2() {
        let res = ComponentSlug::parse("/foo");
        assert!(res.is_err())
    }
}
