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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use crate::metrics::component::record_compilation_time;
use crate::services::compiled_component;
use crate::services::compiled_component::CompiledComponentService;
use crate::services::golem_config::{
    CompiledComponentServiceConfig, ComponentCacheConfig, ComponentServiceConfig,
};
use crate::services::plugins::PluginsObservations;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::v1::{
    download_component_response, get_component_metadata_response, ComponentError,
    DownloadComponentRequest, GetLatestComponentRequest, GetVersionedComponentRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::component_metadata::LinearMemory;
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::RetryConfig;
use golem_common::model::{
    AccountId, ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_common::retries::with_retries;
use golem_service_base::storage::blob::BlobStorage;
use golem_wasm_ast::analysis::AnalysedExport;
use http::Uri;
use prost::Message;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::{debug, info, warn};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentMetadata {
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugin_installations: Vec<PluginInstallation>,
}

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService {
    async fn get(
        &self,
        engine: &Engine,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata), GolemError>;

    async fn get_metadata(
        &self,
        account_id: &AccountId,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<ComponentMetadata, GolemError>;
}

pub async fn configured(
    config: &ComponentServiceConfig,
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
    plugin_observations: Arc<dyn PluginsObservations + Send + Sync>,
) -> Arc<dyn ComponentService + Send + Sync> {
    let compiled_component_service = compiled_component::configured(compiled_config, blob_storage);
    match config {
        ComponentServiceConfig::Grpc(config) => {
            info!("Using component API at {}", config.url());
            Arc::new(ComponentServiceGrpc::new(
                config.uri(),
                config
                    .access_token
                    .parse::<Uuid>()
                    .expect("Access token must be an UUID"),
                cache_config.max_capacity,
                cache_config.max_metadata_capacity,
                cache_config.time_to_idle,
                config.retries.clone(),
                compiled_component_service,
                config.max_component_size,
                plugin_observations,
            ))
        }
        ComponentServiceConfig::Local(config) => Arc::new(ComponentServiceLocalFileSystem::new(
            &config.root,
            cache_config.max_capacity,
            cache_config.max_metadata_capacity,
            cache_config.time_to_idle,
            compiled_component_service,
        )),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: ComponentVersion,
}

pub struct ComponentServiceGrpc {
    component_cache: Cache<ComponentKey, (), Component, GolemError>,
    component_metadata_cache: Cache<ComponentKey, (), ComponentMetadata, GolemError>,
    access_token: Uuid,
    retry_config: RetryConfig,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    client: GrpcClient<ComponentServiceClient<Channel>>,
    plugin_observations: Arc<dyn PluginsObservations + Send + Sync>,
}

impl ComponentServiceGrpc {
    pub fn new(
        endpoint: Uri,
        access_token: Uuid,
        max_capacity: usize,
        max_metadata_capacity: usize,
        time_to_idle: Duration,
        retry_config: RetryConfig,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        max_component_size: usize,
        plugin_observations: Arc<dyn PluginsObservations + Send + Sync>,
    ) -> Self {
        Self {
            component_cache: create_component_cache(max_capacity, time_to_idle),
            component_metadata_cache: create_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            access_token,
            retry_config: retry_config.clone(),
            compiled_component_service,
            client: GrpcClient::new(
                "component_service",
                move |channel| {
                    ComponentServiceClient::new(channel)
                        .max_decoding_message_size(max_component_size)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                endpoint,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    ..Default::default() // TODO
                },
            ),
            plugin_observations,
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceGrpc {
    async fn get(
        &self,
        engine: &Engine,
        account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata), GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let client_clone = self.client.clone();
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
    ) -> Result<ComponentMetadata, GolemError> {
        match forced_version {
            Some(version) => {
                let client = self.client.clone();
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
                    &self.client,
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
) -> Result<ComponentMetadata, GolemError> {
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

                let result = ComponentMetadata {
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
                        .map(|metadata| {
                            let export = metadata.exports;
                            let vec: Vec<Result<AnalysedExport, String>> =
                                export.into_iter().map(AnalysedExport::try_from).collect();
                            vec.into_iter().collect()
                        })
                        .unwrap_or_else(|| Ok(Vec::new()))
                        .map_err(|_| {
                            GrpcError::Unexpected("Failed to get the exports".to_string())
                        })?,
                    files: component
                        .files
                        .into_iter()
                        .map(|file| file.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| {
                            GrpcError::Unexpected("Failed to get the files".to_string())
                        })?,
                    plugin_installations: component
                        .installed_plugins
                        .into_iter()
                        .map(|plugin| plugin.try_into())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| {
                            GrpcError::Unexpected(
                                "Failed to get the plugin installations".to_string(),
                            )
                        })?,
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
) -> Cache<ComponentKey, (), ComponentMetadata, GolemError> {
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

impl From<std::io::Error> for GolemError {
    fn from(value: std::io::Error) -> Self {
        GolemError::Unknown {
            details: format!("{}", value),
        }
    }
}

pub struct ComponentServiceLocalFileSystem {
    root: PathBuf,
    component_cache: Cache<ComponentKey, (), Component, GolemError>,
    component_metadata_cache: Cache<ComponentKey, (), ComponentMetadata, GolemError>,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
}

impl ComponentServiceLocalFileSystem {
    pub fn new(
        root: &Path,
        max_capacity: usize,
        max_metadata_capacity: usize,
        time_to_idle: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    ) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local component store");
        }
        Self {
            root: root.to_path_buf(),
            component_cache: create_component_cache(max_capacity, time_to_idle),
            component_metadata_cache: create_component_metadata_cache(
                max_metadata_capacity,
                time_to_idle,
            ),
            compiled_component_service,
        }
    }

    async fn find_component_files(
        &self,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<(ComponentVersion, PathBuf, PathBuf), GolemError> {
        let prefix: String = format!("{}-", component_id);
        let mut reader = tokio::fs::read_dir(&self.root).await?;
        let mut matching_files = Vec::new();
        while let Some(entry) = reader.next_entry().await? {
            if let Ok(file_name) = entry.file_name().into_string() {
                if file_name.starts_with(&prefix) && file_name.ends_with(".wasm") {
                    // strip wasm extension
                    let base_path = file_name[..file_name.len() - 5].to_string();

                    if let Some(version) = Self::extract_version(&base_path) {
                        matching_files.push((
                            version,
                            entry.path(),
                            self.root.join(PathBuf::from(format!("{base_path}.json"))),
                        ));
                    };
                };
            };
        }

        match forced_version {
            Some(forced_version) => matching_files
                .into_iter()
                .find(|(version, _, _)| *version == forced_version)
                .ok_or(GolemError::GetLatestVersionOfComponentFailed {
                    component_id: component_id.clone(),
                    reason: "Could not find any component with the given id and version"
                        .to_string(),
                }),
            None => matching_files
                .into_iter()
                .max_by_key(|(version, _, _)| *version)
                .ok_or(GolemError::GetLatestVersionOfComponentFailed {
                    component_id: component_id.clone(),
                    reason: "Could not find any component with the given id".to_string(),
                }),
        }
    }

    async fn get_component_from_path(
        &self,
        wasm_path: &Path,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<Component, GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id = component_id.clone();
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let path = wasm_path.to_path_buf();

        self.component_cache
            .get_or_insert_simple(&key.clone(), || {
                Box::pin(async move {
                    let result = compiled_component_service
                        .get(&component_id, component_version, &engine)
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
                            let bytes = tokio::fs::read(path).await?;

                            let start = Instant::now();
                            let component =
                                Component::from_binary(&engine, &bytes).map_err(|e| {
                                    GolemError::ComponentParseFailed {
                                        component_id: component_id.clone(),
                                        component_version,
                                        reason: format!("{}", e),
                                    }
                                })?;
                            let end = Instant::now();

                            let compilation_time = end.duration_since(start);
                            record_compilation_time(compilation_time);
                            debug!(
                                "Compiled {} in {}ms",
                                component_id,
                                compilation_time.as_millis(),
                            );

                            let result = compiled_component_service
                                .put(&component_id, component_version, &component)
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
            .await
    }

    async fn get_metadata_from_path(
        &self,
        props_path: &Path,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<ComponentMetadata, GolemError> {
        let component_id = component_id.clone();
        let props_path = PathBuf::from(props_path);

        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };

        self.component_metadata_cache
            .get_or_insert_simple(&key, || {
                Box::pin(async move {
                    let data = tokio::fs::read_to_string(props_path.clone()).await.map_err(|e| {
                        GolemError::GetLatestVersionOfComponentFailed {
                            component_id: component_id.clone(),
                            reason: format!("Failed to read properties of component {component_id}/{component_version} from {props_path:?}: {}", e),
                        }
                    })?;

                    serde_json::from_str(&data).map_err(|e| {
                        GolemError::GetLatestVersionOfComponentFailed {
                            component_id: component_id.clone(),
                            reason: format!("Failed to deserialize properties of component {component_id}/{component_version}: {}", e),
                        }
                    })
                })
            })
            .await
    }

    fn extract_version(file_name: &str) -> Option<ComponentVersion> {
        let version_part = file_name.split('-').last()?;
        version_part.parse::<u64>().ok()
    }
}

#[async_trait]
impl ComponentService for ComponentServiceLocalFileSystem {
    async fn get(
        &self,
        engine: &Engine,
        _account_id: &AccountId,
        component_id: &ComponentId,
        component_version: ComponentVersion,
    ) -> Result<(Component, ComponentMetadata), GolemError> {
        let (version, wasm_path, props_path) = self
            .find_component_files(component_id, Some(component_version))
            .await?;

        let component = self
            .get_component_from_path(&wasm_path, engine, component_id, version)
            .await?;
        let metadata = self
            .get_metadata_from_path(&props_path, component_id, version)
            .await?;
        Ok((component, metadata))
    }

    async fn get_metadata(
        &self,
        _account_id: &AccountId,
        component_id: &ComponentId,
        forced_version: Option<ComponentVersion>,
    ) -> Result<ComponentMetadata, GolemError> {
        let (version, _, props_path) = self
            .find_component_files(component_id, forced_version)
            .await?;
        self.get_metadata_from_path(&props_path, component_id, version)
            .await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentProperties {
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
}
