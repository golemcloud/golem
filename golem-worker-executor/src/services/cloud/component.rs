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

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cloud::CloudGolemTypes;
use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use crate::metrics::component::record_compilation_time;
use crate::services::compiled_component;
use crate::services::compiled_component::CompiledComponentService;
use crate::services::component::{ComponentMetadata, ComponentService};
use crate::services::golem_config::{
    CompiledComponentServiceConfig, ComponentCacheConfig, ComponentServiceConfig,
    ProjectServiceConfig,
};
use crate::services::plugins::PluginsObservations;
use async_trait::async_trait;
use cloud_api_grpc::proto::golem::cloud::project::v1::cloud_project_service_client::CloudProjectServiceClient;
use cloud_common::model::CloudComponentOwner;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    download_component_response, get_component_metadata_response, ComponentError,
    DownloadComponentRequest, GetComponentsRequest, GetLatestComponentRequest,
    GetVersionedComponentRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::{AccountId, ComponentId, ComponentVersion};
use golem_common::model::{ProjectId, RetryConfig};
use golem_common::retries::with_retries;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_ast::analysis::AnalysedExport;
use http::Uri;
use prost::Message;
use tokio::task::spawn_blocking;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{debug, info, warn};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

pub fn configured(
    config: &ComponentServiceConfig,
    project_service_config: &ProjectServiceConfig,
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    plugin_observations: Arc<dyn PluginsObservations + Send + Sync>,
) -> Arc<dyn ComponentService<CloudGolemTypes> + Send + Sync> {
    let compiled_component_service = compiled_component::configured(compiled_config, blob_storage);
    match (config, project_service_config) {
        (ComponentServiceConfig::Grpc(config), ProjectServiceConfig::Grpc(project_config)) => {
            info!("Using component API at {}", config.url());
            Arc::new(ComponentServiceCloudGrpc::new(
                config.uri(),
                project_config.uri(),
                config
                    .access_token
                    .parse::<Uuid>()
                    .expect("Access token must be an UUID"),
                cache_config.max_capacity,
                cache_config.max_metadata_capacity,
                cache_config.max_resolved_component_capacity,
                cache_config.max_resolved_project_capacity,
                cache_config.time_to_idle,
                config.retries.clone(),
                config.connect_timeout,
                compiled_component_service,
                config.max_component_size,
                plugin_observations,
            ))
        }
        _ => panic!("Unsupported cloud component and project service configuration. Currently only gRPC is supported for both")
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: ComponentVersion,
}

pub struct ComponentServiceCloudGrpc {
    component_cache: Cache<ComponentKey, (), Component, GolemError>,
    component_metadata_cache:
        Cache<ComponentKey, (), ComponentMetadata<CloudGolemTypes>, GolemError>,
    resolved_component_cache: Cache<(ProjectId, String), (), Option<ComponentId>, GolemError>,
    access_token: Uuid,
    retry_config: RetryConfig,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    component_client: GrpcClient<ComponentServiceClient<Channel>>,
    project_client: GrpcClient<CloudProjectServiceClient<Channel>>,
    plugin_observations: Arc<dyn PluginsObservations>,
    resolved_project_cache: Cache<(AccountId, String), (), Option<ProjectId>, GolemError>,
}

impl ComponentServiceCloudGrpc {
    pub fn new(
        component_endpoint: Uri,
        project_endpoint: Uri,
        access_token: Uuid,
        max_component_capacity: usize,
        max_metadata_capacity: usize,
        max_resolved_component_capacity: usize,
        max_resolved_project_capacity: usize,
        time_to_idle: Duration,
        retry_config: RetryConfig,
        connect_timeout: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        max_component_size: usize,
        plugin_observations: Arc<dyn PluginsObservations>,
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
            project_client: GrpcClient::new(
                "project_service",
                move |channel| {
                    CloudProjectServiceClient::new(channel)
                        .max_decoding_message_size(max_component_size)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                project_endpoint,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    connect_timeout,
                },
            ),
            resolved_project_cache: create_resolved_project_cache(
                max_resolved_project_capacity,
                time_to_idle,
            ),
            plugin_observations,
        }
    }

    fn resolve_project_remotely(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> impl Future<Output = Result<Option<ProjectId>, GolemError>> + 'static {
        use cloud_api_grpc::proto::golem::cloud::project::v1::{
            get_projects_response, GetProjectsRequest, ProjectError,
        };
        use cloud_api_grpc::proto::golem::cloud::project::Project as GrpcProject;

        let client = self.project_client.clone();
        let retry_config = self.retry_config.clone();
        let access_token = self.access_token;

        fn get_account(project: &GrpcProject) -> AccountId {
            project
                .data
                .clone()
                .expect("did not receive account data")
                .owner_account_id
                .expect("failed to receive project owner_account_id")
                .into()
        }

        let account_id = account_id.clone();
        let project_name = project_name.to_string();

        async move {
            with_retries(
                "component",
                "resolve_project_remotely",
                Some(format!("{account_id}/{project_name}").to_string()),
                &retry_config,
                &(
                    client,
                    account_id.clone(),
                    project_name.to_string(),
                    access_token,
                ),
                |(client, account_id, project_name, access_token)| {
                    Box::pin(async move {
                        let response = client
                            .call("lookup_project_by_name", move |client| {
                                let request = authorised_grpc_request(
                                    GetProjectsRequest {
                                        project_name: Some(project_name.to_string()),
                                    },
                                    access_token,
                                );
                                Box::pin(client.get_projects(request))
                            })
                            .await?
                            .into_inner();

                        match response
                            .result
                            .expect("Didn't receive expected field result")
                        {
                            get_projects_response::Result::Success(payload) => {
                                let project_id = payload
                                    .data
                                    .into_iter()
                                    // TODO: Push account filter to the server
                                    .find(|p| get_account(p) == *account_id)
                                    .map(|c| c.id.expect("didn't receive expected project_id"));
                                Ok(project_id
                                    .map(|c| c.try_into().expect("failed to convert project_id")))
                            }
                            get_projects_response::Result::Error(err) => {
                                Err(GrpcError::Domain(err))?
                            }
                        }
                    })
                },
                is_grpc_retriable::<ProjectError>,
            )
            .await
            .map_err(|err| GolemError::unknown(format!("Failed to get project: {err}")))
        }
    }

    fn resolve_component_remotely(
        &self,
        project_id: &ProjectId,
        component_name: &str,
    ) -> impl Future<Output = Result<Option<ComponentId>, GolemError>> + 'static {
        use golem_api_grpc::proto::golem::component::v1::{
            get_components_response, ComponentError,
        };

        let client = self.component_client.clone();
        let retry_config = self.retry_config.clone();
        let access_token = self.access_token;

        let project_id = project_id.clone();
        let component_name = component_name.to_string();

        async move {
            with_retries(
                "component",
                "resolve_component_remotely",
                Some(format!("{project_id}/{component_name}").to_string()),
                &retry_config,
                &(client, project_id, component_name, access_token),
                |(client, project_id, component_name, access_token)| {
                    Box::pin(async move {
                        let response = client
                            .call("lookup_component_by_name", move |client| {
                                let request = authorised_grpc_request(
                                    GetComponentsRequest {
                                        project_id: Some(project_id.clone().into()),
                                        component_name: Some(component_name.clone()),
                                    },
                                    access_token,
                                );
                                Box::pin(client.get_components(request))
                            })
                            .await?
                            .into_inner();

                        match response
                            .result
                            .expect("Didn't receive expected field result")
                        {
                            get_components_response::Result::Success(payload) => {
                                let component_id = payload.components.first().map(|c| {
                                    c.versioned_component_id
                                        .expect("didn't receive expected versioned component id")
                                        .component_id
                                        .expect("didn't receive expected component id")
                                });
                                Ok(component_id.map(|c| {
                                    ComponentId::try_from(c)
                                        .expect("failed to convert component id")
                                }))
                            }
                            get_components_response::Result::Error(err) => {
                                Err(GrpcError::Domain(err))?
                            }
                        }
                    })
                },
                is_grpc_retriable::<ComponentError>,
            )
            .await
            .map_err(|err| GolemError::unknown(format!("Failed to get component: {err}")))
        }
    }
}

#[async_trait]
impl ComponentService<CloudGolemTypes> for ComponentServiceCloudGrpc {
    async fn get(
        &self,
        engine: &Engine,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata<CloudGolemTypes>), GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let client_clone = self.component_client.clone();
        let component_id_clone = component_id.clone();
        let engine = engine.clone();
        let access_token = self.access_token;
        let retry_config_clone = self.retry_config.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let component = self
            .component_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_component_service
                        .get(&component_id_clone, component_version, &engine)
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
                            let bytes = download_via_grpc(
                                &client_clone,
                                &access_token,
                                &retry_config_clone,
                                &component_id_clone,
                                component_version,
                            )
                            .await?;

                            let start = Instant::now();
                            let component_id_clone2 = component_id_clone.clone();
                            let component = spawn_blocking(move || {
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    GolemError::ComponentParseFailed {
                                        component_id: component_id_clone2,
                                        component_version,
                                        reason: format!("{}", e),
                                    }
                                })
                            })
                            .await
                            .map_err(|join_err| GolemError::unknown(join_err.to_string()))??;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                component_id_clone,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_component_service
                                .put(&component_id_clone, component_version, &component)
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
        let metadata = self
            .get_metadata(account_id, component_id, Some(component_version))
            .await?;

        Ok((component, metadata))
    }

    async fn get_metadata(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<ComponentMetadata<CloudGolemTypes>, GolemError> {
        match forced_version {
            Some(version) => {
                let client = self.component_client.clone();
                let access_token = self.access_token;
                let retry_config = self.retry_config.clone();
                let component_id = component_id.clone();
                let plugin_observations = self.plugin_observations.clone();
                let account_id = account_id.clone();
                self.component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id: component_id.clone(),
                            component_version: version,
                        },
                        || {
                            Box::pin(async move {
                                let metadata = get_metadata_via_grpc(
                                    &client,
                                    &access_token,
                                    &retry_config,
                                    &component_id,
                                    forced_version,
                                )
                                .await?;
                                for installation in &metadata.plugin_installations {
                                    plugin_observations
                                        .observe_plugin_installation(
                                            &account_id,
                                            &component_id,
                                            metadata.version,
                                            installation,
                                        )
                                        .await?;
                                }
                                Ok(metadata)
                            })
                        },
                    )
                    .await
            }
            None => {
                let metadata = get_metadata_via_grpc(
                    &self.component_client,
                    &self.access_token,
                    &self.retry_config,
                    component_id,
                    None,
                )
                .await?;

                let metadata = self
                    .component_metadata_cache
                    .get_or_insert_simple(
                        &ComponentKey {
                            component_id: component_id.clone(),
                            component_version: metadata.version,
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
        component_reference: String,
        resolving_component: CloudComponentOwner,
    ) -> Result<Option<ComponentId>, GolemError> {
        let component_slug = ComponentSlug::parse(&component_reference).map_err(|e| {
            GolemError::invalid_request(format!("Invalid component reference: {e}"))
        })?;

        let account_id = component_slug
            .account_id
            .clone()
            .unwrap_or(resolving_component.account_id);

        let project_id = if let Some(project_name) = component_slug.project_name {
            self.resolved_project_cache
                .get_or_insert_simple(&(account_id.clone(), project_name.clone()), || {
                    Box::pin(self.resolve_project_remotely(&account_id, &project_name))
                })
                .await?
                .ok_or(GolemError::invalid_request(format!(
                    "Failed to resolve project: {project_name}"
                )))?
        } else {
            resolving_component.project_id
        };

        let component_name = component_slug.component_name;

        self.resolved_component_cache
            .get_or_insert_simple(&(project_id.clone(), component_name.clone()), || {
                Box::pin(self.resolve_component_remotely(&project_id, &component_name))
            })
            .await
    }
}

async fn download_via_grpc(
    client: &GrpcClient<ComponentServiceClient<Channel>>,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
    component_version: ComponentVersion,
) -> Result<Vec<u8>, GolemError> {
    with_retries(
        "components",
        "download",
        Some(component_id.to_string()),
        retry_config,
        &(
            client.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(client, component_id, access_token)| {
            Box::pin(async move {
                let response = client
                    .call("download_component", move |client| {
                        let request = authorised_grpc_request(
                            DownloadComponentRequest {
                                component_id: Some(component_id.clone().into()),
                                version: Some(component_version),
                            },
                            access_token,
                        );
                        Box::pin(client.download_component(request))
                    })
                    .await?
                    .into_inner();

                let chunks = response.into_stream().try_collect::<Vec<_>>().await?;
                let bytes = chunks
                    .into_iter()
                    .map(|chunk| match chunk.result {
                        None => Err("Empty response".to_string().into()),
                        Some(download_component_response::Result::SuccessChunk(chunk)) => Ok(chunk),
                        Some(download_component_response::Result::Error(error)) => {
                            Err(GrpcError::Domain(error))
                        }
                    })
                    .collect::<Result<Vec<Vec<u8>>, GrpcError<ComponentError>>>()?;

                let bytes: Vec<u8> = bytes.into_iter().flatten().collect();

                record_external_call_response_size_bytes("components", "download", bytes.len());

                Ok(bytes)
            })
        },
        is_grpc_retriable::<ComponentError>,
    )
    .await
    .map_err(|error| grpc_component_download_error(error, component_id, component_version))
}

async fn get_metadata_via_grpc(
    client: &GrpcClient<ComponentServiceClient<Channel>>,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
    component_version: Option<ComponentVersion>,
) -> Result<ComponentMetadata<CloudGolemTypes>, GolemError> {
    let desc = format!("Getting component metadata of {component_id}");
    debug!("{}", &desc);
    with_retries(
        "components",
        "get_metadata",
        Some(component_id.to_string()),
        retry_config,
        &(
            client.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(client, component_id, access_token)| {
            Box::pin(async move {
                let response = match component_version {
                    Some(component_version) => client
                        .call("get_component_metadata", move |client| {
                            let request = authorised_grpc_request(
                                GetVersionedComponentRequest {
                                    component_id: Some(component_id.clone().into()),
                                    version: component_version,
                                },
                                access_token,
                            );
                            Box::pin(client.get_component_metadata(request))
                        })
                        .await?
                        .into_inner(),
                    None => client
                        .call("get_latest_component_metadata", move |client| {
                            let request = authorised_grpc_request(
                                GetLatestComponentRequest {
                                    component_id: Some(component_id.clone().into()),
                                },
                                access_token,
                            );
                            Box::pin(client.get_latest_component_metadata(request))
                        })
                        .await?
                        .into_inner(),
                };
                let len = response.encoded_len();
                let component = match response.result {
                    None => Err("Empty response".to_string().into()),
                    Some(get_component_metadata_response::Result::Success(response)) => {
                        Ok(response.component.ok_or(GrpcError::Unexpected(
                            "No component information in response".to_string(),
                        ))?)
                    }
                    Some(get_component_metadata_response::Result::Error(error)) => {
                        Err(GrpcError::Domain(error))
                    }
                }?;

                let result = ComponentMetadata::<CloudGolemTypes> {
                    version: component
                        .versioned_component_id
                        .as_ref()
                        .map(|id| id.version)
                        .ok_or(GrpcError::Unexpected(
                            "Undefined component version".to_string(),
                        ))?,
                    size: component.component_size,
                    component_type: component.component_type().into(),
                    memories: component
                        .metadata
                        .as_ref()
                        .map(|metadata| metadata.memories.iter().map(|m| (*m).into()).collect())
                        .unwrap_or_default(),
                    exports: component
                        .metadata
                        .as_ref()
                        .map(|metadata| {
                            let export = &metadata.exports;
                            let vec: Vec<Result<AnalysedExport, String>> = export
                                .iter()
                                .cloned()
                                .map(AnalysedExport::try_from)
                                .collect();
                            vec.into_iter().collect()
                        })
                        .unwrap_or_else(|| Ok(Vec::new()))
                        .map_err(|err| {
                            GrpcError::Unexpected(format!("Failed to get the exports: {err}"))
                        })?,
                    files: component
                        .files
                        .into_iter()
                        .map(|file| file.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            GrpcError::Unexpected(format!("Failed to get the files: {err}"))
                        })?,
                    plugin_installations: component
                        .installed_plugins
                        .into_iter()
                        .map(|plugin| plugin.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|err| {
                            GrpcError::Unexpected(format!(
                                "Failed to get the plugin installations: {err}"
                            ))
                        })?,
                    dynamic_linking: HashMap::from_iter(
                        component
                            .metadata
                            .map(|metadata| {
                                metadata
                                    .dynamic_linking
                                    .into_iter()
                                    .map(|(k, v)| v.try_into().map(|v| (k.clone(), v)))
                                    .collect::<Result<Vec<_>, String>>()
                            })
                            .unwrap_or_else(|| Ok(Vec::new()))
                            .map_err(|err| {
                                GrpcError::Unexpected(format!(
                                    "Failed to get the dynamic linking information: {err}"
                                ))
                            })?,
                    ),
                    component_owner: CloudComponentOwner {
                        account_id: component
                            .account_id
                            .ok_or(GrpcError::Unexpected(
                                "Missing account_id for component".to_string(),
                            ))?
                            .into(),
                        project_id: ProjectId(
                            component
                                .project_id
                                .and_then(|p| p.value)
                                .ok_or(GrpcError::Unexpected(
                                    "Missing project_id for component".to_string(),
                                ))?
                                .into(),
                        ),
                    },
                    env: component.env,
                };

                record_external_call_response_size_bytes("components", "get_metadata", len);

                Ok(result)
            })
        },
        is_grpc_retriable::<ComponentError>,
    )
    .await
    .map_err(|error| grpc_get_latest_version_error(error, component_id))
}

fn grpc_component_download_error(
    error: GrpcError<ComponentError>,
    component_id: &ComponentId,
    component_version: ComponentVersion,
) -> GolemError {
    GolemError::ComponentDownloadFailed {
        component_id: component_id.clone(),
        component_version,
        reason: format!("{}", error),
    }
}

fn grpc_get_latest_version_error(
    error: GrpcError<ComponentError>,
    component_id: &ComponentId,
) -> GolemError {
    GolemError::GetLatestVersionOfComponentFailed {
        component_id: component_id.clone(),
        reason: format!("{}", error),
    }
}

fn create_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), Component, GolemError> {
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

fn create_component_metadata_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<ComponentKey, (), ComponentMetadata<CloudGolemTypes>, GolemError> {
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

fn create_resolved_project_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<(AccountId, String), (), Option<ProjectId>, GolemError> {
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "resolved_project",
    )
}

fn create_resolved_component_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<(ProjectId, String), (), Option<ComponentId>, GolemError> {
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

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct ComponentSlug {
    account_id: Option<AccountId>,
    project_name: Option<String>,
    component_name: String,
}

impl ComponentSlug {
    pub fn parse(str: &str) -> Result<Self, String> {
        // TODO: We probably want more validations here.
        if str.is_empty() {
            Err("Empty component references are not allowed")?;
        };

        if str.contains(" ") {
            Err("No spaces allowed in component reference")?;
        };

        let mut parts = str.split("/").collect::<Vec<_>>();

        if parts.is_empty() || parts.len() > 3 {
            Err("Unexpected number of \"/\"-delimited parts in component reference")?
        };

        parts.reverse();

        Ok(ComponentSlug {
            account_id: parts.get(2).map(|s| AccountId {
                value: s.to_string(),
            }),
            project_name: parts.get(1).map(|s| s.to_string()),
            component_name: parts[0].to_string(), // safe due to the check above
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentSlug;
    use golem_common::model::AccountId;
    use test_r::test;

    #[test]
    fn parse_component() {
        let res = ComponentSlug::parse("foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_id: None,
                project_name: None,
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_project_component() {
        let res = ComponentSlug::parse("bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_id: None,
                project_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_account_project_component() {
        let res = ComponentSlug::parse("baz/bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_id: Some(AccountId {
                    value: "baz".to_string()
                }),
                project_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn reject_longer() {
        let res = ComponentSlug::parse("foo/baz/bar/foobar");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty() {
        let res = ComponentSlug::parse("");
        assert!(res.is_err())
    }

    #[test]
    fn reject_spaces() {
        let res = ComponentSlug::parse("baz/bar baz/foobar");
        assert!(res.is_err())
    }
}
