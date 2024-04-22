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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::TryStreamExt;
use golem_api_grpc::proto::golem::component::component_service_client::ComponentServiceClient;
use golem_api_grpc::proto::golem::component::ComponentError;
use golem_api_grpc::proto::golem::component::{
    download_component_response, get_component_metadata_response, DownloadComponentRequest,
    GetLatestComponentRequest,
};
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::config::RetryConfig;
use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
use golem_common::model::ComponentId;
use golem_common::retries::with_retries;
use http::Uri;
use prost::Message;
use tracing::{debug, info, warn};
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError, UriBackConversion};
use crate::metrics::component::record_compilation_time;
use crate::services::compiled_component;
use crate::services::compiled_component::CompiledComponentService;
use crate::services::golem_config::{
    CompiledComponentServiceConfig, ComponentCacheConfig, ComponentServiceConfig,
};

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Component, GolemError>;

    async fn get_latest(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
    ) -> Result<(u64, Component), GolemError>;

    async fn get_latest_version(&self, component_id: &ComponentId) -> Result<u64, GolemError>;
}

pub async fn configured(
    config: &ComponentServiceConfig,
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
) -> Arc<dyn ComponentService + Send + Sync> {
    let compiled_component_service = compiled_component::configured(compiled_config).await;
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
                cache_config.time_to_idle,
                config.retries.clone(),
                compiled_component_service,
                config.max_component_size,
            ))
        }
        ComponentServiceConfig::Local(config) => Arc::new(ComponentServiceLocalFileSystem::new(
            &config.root,
            cache_config.max_capacity,
            cache_config.time_to_idle,
            compiled_component_service,
        )),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: u64,
}

pub struct ComponentServiceGrpc {
    endpoint: Uri,
    component_cache: Cache<ComponentKey, (), Component, GolemError>,
    access_token: Uuid,
    retry_config: RetryConfig,
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    max_component_size: usize,
}

impl ComponentServiceGrpc {
    pub fn new(
        endpoint: Uri,
        access_token: Uuid,
        max_capacity: usize,
        time_to_idle: Duration,
        retry_config: RetryConfig,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        max_component_size: usize,
    ) -> Self {
        Self {
            endpoint,
            component_cache: create_component_cache(max_capacity, time_to_idle),
            access_token,
            retry_config,
            compiled_component_service,
            max_component_size,
        }
    }
}

#[async_trait]
impl ComponentService for ComponentServiceGrpc {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Component, GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id = component_id.clone();
        let engine = engine.clone();
        let endpoint = self.endpoint.clone();
        let access_token = self.access_token;
        let retry_config = self.retry_config.clone();
        let max_component_size = self.max_component_size;
        let compiled_component_service = self.compiled_component_service.clone();
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
                            let bytes = download_via_grpc(
                                &endpoint,
                                &access_token,
                                &retry_config,
                                &component_id,
                                component_version,
                                max_component_size,
                            )
                            .await?;

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

    async fn get_latest(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
    ) -> Result<(u64, Component), GolemError> {
        let latest_version = get_latest_version_via_grpc(
            &self.endpoint,
            &self.access_token,
            &self.retry_config,
            component_id,
        )
        .await?;
        let component = self.get(engine, component_id, latest_version).await?;
        Ok((latest_version, component))
    }

    async fn get_latest_version(&self, component_id: &ComponentId) -> Result<u64, GolemError> {
        get_latest_version_via_grpc(
            &self.endpoint,
            &self.access_token,
            &self.retry_config,
            component_id,
        )
        .await
    }
}

async fn download_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
    component_version: u64,
    max_component_size: usize,
) -> Result<Vec<u8>, GolemError> {
    let desc = format!("Downloading component {component_id}");
    debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, component_id, access_token)| {
            Box::pin(async move {
                let mut client = ComponentServiceClient::connect(endpoint.as_http_02())
                    .await?
                    .max_decoding_message_size(max_component_size);

                let request = authorised_grpc_request(
                    DownloadComponentRequest {
                        component_id: Some(component_id.clone().into()),
                        version: Some(component_version),
                    },
                    access_token,
                );

                let response = client.download_component(request).await?.into_inner();

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

async fn get_latest_version_via_grpc(
    endpoint: &Uri,
    access_token: &Uuid,
    retry_config: &RetryConfig,
    component_id: &ComponentId,
) -> Result<u64, GolemError> {
    let desc = format!("Getting latest version of {component_id}");
    debug!("{}", &desc);
    with_retries(
        &desc,
        "components",
        "download",
        retry_config,
        &(
            endpoint.clone(),
            component_id.clone(),
            access_token.to_owned(),
        ),
        |(endpoint, component_id, access_token)| {
            Box::pin(async move {
                let mut client = ComponentServiceClient::connect(endpoint.as_http_02()).await?;

                let request = authorised_grpc_request(
                    GetLatestComponentRequest {
                        component_id: Some(component_id.clone().into()),
                    },
                    access_token,
                );

                let response = client
                    .get_latest_component_metadata(request)
                    .await?
                    .into_inner();

                let len = response.encoded_len();
                let version = match response.result {
                    None => Err("Empty response".to_string().into()),
                    Some(get_component_metadata_response::Result::Success(response)) => {
                        let version = response
                            .component
                            .and_then(|component| component.versioned_component_id)
                            .map(|id| id.version)
                            .ok_or(GrpcError::Unexpected(
                                "Undefined component version".to_string(),
                            ))?;
                        Ok(version)
                    }
                    Some(get_component_metadata_response::Result::Error(error)) => {
                        Err(GrpcError::Domain(error))
                    }
                }?;

                record_external_call_response_size_bytes("components", "get_latest_version", len);

                Ok(version)
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
    component_version: u64,
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
    compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
}

impl ComponentServiceLocalFileSystem {
    pub fn new(
        root: &Path,
        max_capacity: usize,
        time_to_idle: Duration,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
    ) -> Self {
        if !root.exists() {
            std::fs::create_dir_all(root).expect("Failed to create local component store");
        }
        Self {
            root: root.to_path_buf(),
            component_cache: create_component_cache(max_capacity, time_to_idle),
            compiled_component_service,
        }
    }

    async fn get_from_path(
        &self,
        path: &Path,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Component, GolemError> {
        let key = ComponentKey {
            component_id: component_id.clone(),
            component_version,
        };
        let component_id = component_id.clone();
        let engine = engine.clone();
        let compiled_component_service = self.compiled_component_service.clone();
        let path = path.to_path_buf();
        debug!("Loading component from {:?}", path);
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
}

#[async_trait]
impl ComponentService for ComponentServiceLocalFileSystem {
    async fn get(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
        component_version: u64,
    ) -> Result<Component, GolemError> {
        let path = self
            .root
            .join(format!("{}-{}.wasm", component_id, component_version));

        self.get_from_path(&path, engine, component_id, component_version)
            .await
    }

    async fn get_latest(
        &self,
        engine: &Engine,
        component_id: &ComponentId,
    ) -> Result<(u64, Component), GolemError> {
        let prefix = format!("{}-", component_id);
        let mut reader = tokio::fs::read_dir(&self.root).await?;
        let mut matching_files = Vec::new();
        while let Some(entry) = reader.next_entry().await? {
            if let Ok(file_name) = entry.file_name().into_string() {
                if file_name.starts_with(&prefix) && file_name.ends_with(".wasm") {
                    matching_files.push((
                        entry.path(),
                        file_name[prefix.len()..file_name.len() - 5].to_string(),
                    ));
                }
            }
        }

        let latest = matching_files
            .into_iter()
            .filter_map(|(path, s)| s.parse::<u64>().map(|version| (path, version)).ok())
            .max_by_key(|(_, version)| *version);

        match latest {
            Some((path, version)) => {
                let component = self
                    .get_from_path(&path, engine, component_id, version)
                    .await?;
                Ok((version, component))
            }
            None => Err(GolemError::GetLatestVersionOfComponentFailed {
                component_id: component_id.clone(),
                reason: "Could not find any component with the given id".to_string(),
            }),
        }
    }

    async fn get_latest_version(&self, component_id: &ComponentId) -> Result<u64, GolemError> {
        let prefix = format!("{}-", component_id);
        let mut reader = tokio::fs::read_dir(&self.root).await?;
        let mut matching_files = Vec::new();
        while let Some(entry) = reader.next_entry().await? {
            if let Ok(file_name) = entry.file_name().into_string() {
                if file_name.starts_with(&prefix) && file_name.ends_with(".wasm") {
                    matching_files.push((
                        entry.path(),
                        file_name[prefix.len()..file_name.len() - 5].to_string(),
                    ));
                }
            }
        }

        let latest = matching_files
            .into_iter()
            .filter_map(|(_path, s)| s.parse::<u64>().ok())
            .max();

        match latest {
            Some(version) => Ok(version),
            None => Err(GolemError::GetLatestVersionOfComponentFailed {
                component_id: component_id.clone(),
                reason: "Could not find any component with the given id".to_string(),
            }),
        }
    }
}

#[cfg(any(feature = "mocks", test))]
pub struct ComponentServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for ComponentServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl ComponentServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl ComponentService for ComponentServiceMock {
    async fn get(
        &self,
        _engine: &Engine,
        _component_id: &ComponentId,
        _component_version: u64,
    ) -> Result<Component, GolemError> {
        unimplemented!()
    }

    async fn get_latest(
        &self,
        _engine: &Engine,
        _component_id: &ComponentId,
    ) -> Result<(u64, Component), GolemError> {
        unimplemented!()
    }

    async fn get_latest_version(&self, _component_id: &ComponentId) -> Result<u64, GolemError> {
        unimplemented!()
    }
}
