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

use super::golem_config::{
    CompiledComponentServiceConfig, ComponentCacheConfig, ComponentServiceConfig,
    ProjectServiceConfig,
};
use super::plugins::PluginsObservations;
use crate::error::GolemError;
use crate::metrics::component::record_compilation_time;
use async_trait::async_trait;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::component::ComponentOwner;
use golem_common::model::component_metadata::{DynamicLinkedInstance, LinearMemory};
use golem_common::model::plugin::PluginInstallation;
use golem_common::model::{
    AccountId, ComponentId, ComponentType, ComponentVersion, InitialComponentFile,
};
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::testing::LocalFileSystemComponentMetadata;
use golem_wasm_ast::analysis::AnalysedExport;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use uuid::Uuid;
use wasmtime::component::Component;
use wasmtime::Engine;

#[derive(Debug, Clone)]
pub struct ComponentMetadata {
    pub version: ComponentVersion,
    pub size: u64,
    pub memories: Vec<LinearMemory>,
    pub exports: Vec<AnalysedExport>,
    pub component_type: ComponentType,
    pub files: Vec<InitialComponentFile>,
    pub plugin_installations: Vec<PluginInstallation>,
    pub component_owner: ComponentOwner,
    pub dynamic_linking: HashMap<String, DynamicLinkedInstance>,
    pub env: HashMap<String, String>,
}

impl From<LocalFileSystemComponentMetadata> for ComponentMetadata {
    fn from(value: LocalFileSystemComponentMetadata) -> Self {
        Self {
            version: value.version,
            size: value.size,
            memories: value.memories,
            exports: value.exports,
            component_type: value.component_type,
            files: value.files,
            plugin_installations: vec![],
            component_owner: ComponentOwner {
                account_id: value.account_id,
                project_id: value.project_id,
            },
            dynamic_linking: value.dynamic_linking,
            env: value.env,
        }
    }
}

/// Service for downloading a specific Golem component from the Golem Component API
#[async_trait]
pub trait ComponentService: Send + Sync {
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

    /// Resolve a component given a user provided string. The syntax of the provided string is allowed to vary between implementations.
    /// Resolving component is the component in whoose context the resolution is being performed
    async fn resolve_component(
        &self,
        component_reference: String,
        resolving_component: ComponentOwner,
    ) -> Result<Option<ComponentId>, GolemError>;
}

pub fn configured(
    config: &ComponentServiceConfig,
    project_service_config: &ProjectServiceConfig,
    cache_config: &ComponentCacheConfig,
    compiled_config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage>,
    plugin_observations: Arc<dyn PluginsObservations>,
) -> Arc<dyn ComponentService> {
    let compiled_component_service =
        super::compiled_component::configured(compiled_config, blob_storage);
    match (config, project_service_config) {
        (ComponentServiceConfig::Grpc(config), ProjectServiceConfig::Grpc(project_config)) => {
            info!("Using component API at {}", config.url());
            Arc::new(self::grpc::ComponentServiceGrpc::new(
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
        (ComponentServiceConfig::Local(config), ProjectServiceConfig::Disabled(_)) => {
            info!("Using local component server at {:?}", config.root);
            Arc::new(self::filesystem::ComponentServiceLocalFileSystem::new(
                &config.root,
                cache_config.max_capacity,
                cache_config.time_to_idle,
                compiled_component_service,
            ))
        }
        _ => panic!("Unsupported cloud component and project service configuration"),
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ComponentKey {
    component_id: ComponentId,
    component_version: ComponentVersion,
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

mod filesystem {
    use super::create_component_cache;
    use super::record_compilation_time;
    use super::ComponentService;
    use super::{ComponentKey, ComponentMetadata};
    use crate::error::GolemError;
    use crate::services::compiled_component::CompiledComponentService;
    use async_lock::{RwLock, Semaphore};
    use async_trait::async_trait;
    use golem_common::cache::Cache;
    use golem_common::cache::SimpleCache;
    use golem_common::model::component::ComponentOwner;
    use golem_common::model::{AccountId, ComponentId, ComponentVersion};
    use golem_service_base::testing::LocalFileSystemComponentMetadata;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::task::spawn_blocking;
    use tracing::{debug, warn, Instrument};
    use wasmtime::component::Component;
    use wasmtime::Engine;

    pub struct ComponentServiceLocalFileSystem {
        root: PathBuf,
        component_cache: Cache<ComponentKey, (), Component, GolemError>,
        compiled_component_service: Arc<dyn CompiledComponentService>,
        index: RwLock<ComponentMetadataIndex>,
        updating_index: Semaphore,
    }

    impl ComponentServiceLocalFileSystem {
        pub fn new(
            root: &Path,
            max_capacity: usize,
            time_to_idle: Duration,
            compiled_component_service: Arc<dyn CompiledComponentService>,
        ) -> Self {
            if !root.exists() {
                std::fs::create_dir_all(root).expect("Failed to create local component store");
            }
            Self {
                root: root.to_path_buf(),
                component_cache: create_component_cache(max_capacity, time_to_idle),
                compiled_component_service,
                index: RwLock::new(ComponentMetadataIndex::new()),
                updating_index: Semaphore::new(1),
            }
        }

        async fn refresh_index(&self) -> Result<(), GolemError> {
            let permit = self.updating_index.acquire().await;

            let mut new_processed_files: Vec<String> = vec![];
            let mut new_metadata: Vec<LocalFileSystemComponentMetadata> = vec![];
            {
                let current = self.index.read().await;

                let mut reader = tokio::fs::read_dir(&self.root).await?;
                while let Some(entry) = reader.next_entry().await? {
                    if let Ok(file_name) = entry.file_name().into_string() {
                        if !current.processed_files.contains(&file_name)
                            && file_name.ends_with(".json")
                        {
                            new_processed_files.push(file_name.clone());

                            let file_content =
                                tokio::fs::read_to_string(self.root.join(file_name.clone()))
                                    .await
                                    .map_err(|e| GolemError::Unknown {
                                        details: format!(
                                            "Failed to read content from file {file_name}: {e}"
                                        ),
                                    })?;

                            let metadata = serde_json::from_str(&file_content).map_err(|e| {
                                GolemError::Unknown {
                                    details: format!("Failed to deserialize properties of component from {file_name}: {e}")
                                }
                            })?;

                            new_metadata.push(metadata);
                        };
                    };
                }
            }

            {
                let mut current = self.index.write().await;

                for file in new_processed_files {
                    current.processed_files.insert(file);
                }

                for metadata in new_metadata {
                    let component_id = metadata.component_id.clone();
                    let component_version = metadata.version;
                    let component_name = metadata.component_name.clone();

                    current
                        .latest_versions
                        .entry(component_id.clone())
                        .and_modify(|e| *e = (*e).max(component_version))
                        .or_insert(component_version);

                    current
                        .id_by_name
                        .entry(component_name)
                        .or_insert(component_id.clone());

                    let key = ComponentKey {
                        component_id,
                        component_version,
                    };

                    current.metadata.entry(key).or_insert(metadata);
                }
            }

            drop(permit);
            Ok(())
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
                                let component = spawn_blocking({
                                    let component_id = component_id.clone();
                                    move || {
                                        Component::from_binary(&engine, &bytes).map_err(|e| {
                                            GolemError::ComponentParseFailed {
                                                component_id: component_id.clone(),
                                                component_version,
                                                reason: format!("{e}"),
                                            }
                                        })
                                    }
                                })
                                .instrument(tracing::Span::current())
                                .await
                                .map_err(|join_err| GolemError::unknown(join_err.to_string()))??;
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
                                        warn!(
                                            "Failed to upload compiled component {:?}: {}",
                                            key, err
                                        );
                                        Ok(component)
                                    }
                                }
                            }
                        }
                    })
                })
                .await
        }

        async fn get_metadata_for_version(
            &self,
            component_id: &ComponentId,
            component_version: ComponentVersion,
        ) -> Result<ComponentMetadata, GolemError> {
            let key = ComponentKey {
                component_id: component_id.clone(),
                component_version,
            };
            let metadata = self.index.read().await.metadata.get(&key).cloned();

            let metadata = if let Some(metadata) = metadata {
                metadata
            } else {
                self.refresh_index().await?;
                let metadata = self.index.read().await.metadata.get(&key).cloned();
                metadata.ok_or(GolemError::unknown(format!(
                    "No such component found: {component_id}/{component_version}"
                )))?
            };

            Ok(metadata.into())
        }

        async fn get_latest_metadata(
            &self,
            component_id: &ComponentId,
        ) -> Result<ComponentMetadata, GolemError> {
            self.refresh_index().await?;

            let index = self.index.read().await;

            let latest_version = index.latest_versions.get(component_id);

            let metadata = match latest_version {
                Some(component_version) => {
                    let key = ComponentKey {
                        component_id: component_id.clone(),
                        component_version: *component_version,
                    };
                    let metadata = index.metadata.get(&key).cloned();
                    metadata.ok_or(GolemError::unknown(format!(
                        "No such component found: {component_id}/{component_version}"
                    )))?
                }
                None => Err(GolemError::unknown(
                    "Could not find any component with the given id",
                ))?,
            };

            Ok(metadata.into())
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
            let key = ComponentKey {
                component_id: component_id.clone(),
                component_version,
            };
            let metadata = self.index.read().await.metadata.get(&key).cloned();

            let metadata = if let Some(metadata) = metadata {
                metadata
            } else {
                self.refresh_index().await?;
                let metadata = self.index.read().await.metadata.get(&key).cloned();
                metadata.ok_or(GolemError::unknown(format!(
                    "No such component found: {component_id}/{component_version}"
                )))?
            };

            let wasm_path = self.root.join(metadata.wasm_filename.clone());

            let component = self
                .get_component_from_path(&wasm_path, engine, component_id, component_version)
                .await?;

            Ok((component, metadata.into()))
        }

        async fn get_metadata(
            &self,
            _account_id: &AccountId,
            component_id: &ComponentId,
            forced_version: Option<ComponentVersion>,
        ) -> Result<ComponentMetadata, GolemError> {
            match forced_version {
                Some(version) => self.get_metadata_for_version(component_id, version).await,
                None => self.get_latest_metadata(component_id).await,
            }
        }

        async fn resolve_component(
            &self,
            component_reference: String,
            _resolving_component: ComponentOwner,
        ) -> Result<Option<ComponentId>, GolemError> {
            Ok(self
                .index
                .read()
                .await
                .id_by_name
                .get(&component_reference)
                .cloned())
        }
    }

    struct ComponentMetadataIndex {
        processed_files: HashSet<String>,
        metadata: HashMap<ComponentKey, LocalFileSystemComponentMetadata>,
        latest_versions: HashMap<ComponentId, u64>,
        id_by_name: HashMap<String, ComponentId>,
    }

    impl ComponentMetadataIndex {
        fn new() -> Self {
            Self {
                processed_files: HashSet::new(),
                metadata: HashMap::new(),
                latest_versions: HashMap::new(),
                id_by_name: HashMap::new(),
            }
        }
    }
}

mod grpc {
    use super::{create_component_cache, ComponentKey, ComponentMetadata, ComponentService};
    use crate::error::GolemError;
    use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
    use crate::metrics::component::record_compilation_time;
    use crate::services::compiled_component::CompiledComponentService;
    use crate::services::plugins::PluginsObservations;
    use async_trait::async_trait;
    use futures_util::TryStreamExt;
    use golem_api_grpc::proto::golem::component::v1::component_service_client::ComponentServiceClient;
    use golem_api_grpc::proto::golem::component::v1::{
        download_component_response, get_component_metadata_response, ComponentError,
        DownloadComponentRequest, GetComponentsRequest, GetLatestComponentRequest,
        GetVersionedComponentRequest,
    };
    use golem_api_grpc::proto::golem::project::v1::cloud_project_service_client::CloudProjectServiceClient;
    use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
    use golem_common::client::{GrpcClient, GrpcClientConfig};
    use golem_common::metrics::external_calls::record_external_call_response_size_bytes;
    use golem_common::model::component::ComponentOwner;
    use golem_common::model::{AccountId, ComponentId, ComponentVersion};
    use golem_common::model::{ProjectId, RetryConfig};
    use golem_common::retries::with_retries;
    use golem_wasm_ast::analysis::AnalysedExport;
    use http::Uri;
    use prost::Message;
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::task::spawn_blocking;
    use tonic::codec::CompressionEncoding;
    use tonic::transport::Channel;
    use tracing::{debug, warn};
    use uuid::Uuid;
    use wasmtime::component::Component;
    use wasmtime::Engine;

    pub struct ComponentServiceGrpc {
        component_cache: Cache<ComponentKey, (), Component, GolemError>,
        component_metadata_cache: Cache<ComponentKey, (), ComponentMetadata, GolemError>,
        resolved_component_cache: Cache<(ProjectId, String), (), Option<ComponentId>, GolemError>,
        access_token: Uuid,
        retry_config: RetryConfig,
        compiled_component_service: Arc<dyn CompiledComponentService + Send + Sync>,
        component_client: GrpcClient<ComponentServiceClient<Channel>>,
        project_client: GrpcClient<CloudProjectServiceClient<Channel>>,
        plugin_observations: Arc<dyn PluginsObservations>,
        resolved_project_cache: Cache<(AccountId, String), (), Option<ProjectId>, GolemError>,
    }

    impl ComponentServiceGrpc {
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
            use golem_api_grpc::proto::golem::project::v1::{
                get_projects_response, GetProjectsRequest, ProjectError,
            };
            use golem_api_grpc::proto::golem::project::Project as GrpcProject;

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
                                    Ok(project_id.map(|c| {
                                        c.try_into().expect("failed to convert project_id")
                                    }))
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
                                            .expect(
                                                "didn't receive expected versioned component id",
                                            )
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
                                            reason: format!("{e}"),
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
                                        warn!(
                                            "Failed to upload compiled component {:?}: {}",
                                            key, err
                                        );
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
            resolving_component: ComponentOwner,
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
                            Some(download_component_response::Result::SuccessChunk(chunk)) => {
                                Ok(chunk)
                            }
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
                        component_owner: ComponentOwner {
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
            reason: format!("{error}"),
        }
    }

    fn grpc_get_latest_version_error(
        error: GrpcError<ComponentError>,
        component_id: &ComponentId,
    ) -> GolemError {
        GolemError::GetLatestVersionOfComponentFailed {
            component_id: component_id.clone(),
            reason: format!("{error}"),
        }
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
}
