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

use crate::services::golem_config::CompiledComponentServiceConfig;
use crate::Engine;
use async_trait::async_trait;
use golem_common::model::{ComponentId, ProjectId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::Instant;
use tracing::debug;
use wasmtime::component::Component;

/// Service for storing compiled native binaries of WebAssembly components
#[async_trait]
pub trait CompiledComponentService: Send + Sync {
    async fn get(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        component_version: u64,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError>;
    async fn put(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        component_version: u64,
        component: &Component,
    ) -> Result<(), WorkerExecutorError>;
}

pub struct DefaultCompiledComponentService {
    blob_storage: Arc<dyn BlobStorage + Send + Sync>,
}

impl DefaultCompiledComponentService {
    pub fn new(blob_storage: Arc<dyn BlobStorage + Send + Sync>) -> Self {
        Self { blob_storage }
    }

    fn key(component_id: &ComponentId, component_version: u64) -> PathBuf {
        Path::new(&component_id.to_string()).join(format!("{component_version}.cwasm"))
    }
}

#[async_trait]
impl CompiledComponentService for DefaultCompiledComponentService {
    async fn get(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        component_version: u64,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        match self
            .blob_storage
            .get_raw(
                "compiled_component",
                "get",
                BlobStorageNamespace::CompilationCache {
                    project_id: project_id.clone(),
                },
                &Self::key(component_id, component_version),
            )
            .await
        {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => {
                let start = Instant::now();
                let component = unsafe {
                    Component::deserialize(engine, &bytes).map_err(|err| {
                        WorkerExecutorError::component_download_failed(
                            component_id.clone(),
                            component_version,
                            format!("Could not deserialize compiled component: {err}"),
                        )
                    })?
                };
                let end = Instant::now();

                let load_time = end.duration_since(start);
                debug!(
                    "Loaded precompiled image for {} in {}ms",
                    component_id,
                    load_time.as_millis(),
                );

                Ok(Some(component))
            }
            Err(err) => Err(WorkerExecutorError::component_download_failed(
                component_id.clone(),
                component_version,
                format!("Could not download compiled component: {err}"),
            )),
        }
    }

    async fn put(
        &self,
        project_id: &ProjectId,
        component_id: &ComponentId,
        component_version: u64,
        component: &Component,
    ) -> Result<(), WorkerExecutorError> {
        let bytes = component
            .serialize()
            .expect("Could not serialize component");
        self.blob_storage
            .put_raw(
                "compiled_component",
                "put",
                BlobStorageNamespace::CompilationCache {
                    project_id: project_id.clone(),
                },
                &Self::key(component_id, component_version),
                &bytes,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::component_download_failed(
                    component_id.clone(),
                    component_version,
                    format!("Could not store compiled component: {err}"),
                )
            })
    }
}

pub fn configured(
    config: &CompiledComponentServiceConfig,
    blob_storage: Arc<dyn BlobStorage>,
) -> Arc<dyn CompiledComponentService> {
    match config {
        CompiledComponentServiceConfig::Enabled(_) => {
            Arc::new(DefaultCompiledComponentService::new(blob_storage))
        }
        CompiledComponentServiceConfig::Disabled(_) => {
            Arc::new(CompiledComponentServiceDisabled::new())
        }
    }
}

pub struct CompiledComponentServiceDisabled {}

impl Default for CompiledComponentServiceDisabled {
    fn default() -> Self {
        Self::new()
    }
}

impl CompiledComponentServiceDisabled {
    pub fn new() -> Self {
        CompiledComponentServiceDisabled {}
    }
}

#[async_trait]
impl CompiledComponentService for CompiledComponentServiceDisabled {
    async fn get(
        &self,
        _project_id: &ProjectId,
        _component_id: &ComponentId,
        _component_version: u64,
        _engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        Ok(None)
    }

    async fn put(
        &self,
        _project_id: &ProjectId,
        _component_id: &ComponentId,
        _component_version: u64,
        _component: &Component,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }
}
