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

use crate::error::worker_executor::WorkerExecutorError;
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, info_span};
use wasmtime::Engine;
use wasmtime::component::Component;

/// Service for storing compiled native binaries of WebAssembly components
#[async_trait]
pub trait CompiledComponentService: Send + Sync {
    async fn get(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError>;
    async fn put(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component: &Component,
    ) -> Result<(), WorkerExecutorError>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum CompiledComponentServiceConfig {
    Enabled(CompiledComponentServiceEnabledConfig),
    Disabled(CompiledComponentServiceDisabledConfig),
}

impl CompiledComponentServiceConfig {
    pub fn enabled() -> Self {
        Self::Enabled(CompiledComponentServiceEnabledConfig {})
    }

    pub fn disabled() -> Self {
        Self::Disabled(CompiledComponentServiceDisabledConfig {})
    }
}

impl SafeDisplay for CompiledComponentServiceConfig {
    fn to_safe_string(&self) -> String {
        match self {
            CompiledComponentServiceConfig::Enabled(_) => "enabled".to_string(),
            CompiledComponentServiceConfig::Disabled(_) => "disabled".to_string(),
        }
    }
}

impl Default for CompiledComponentServiceConfig {
    fn default() -> Self {
        Self::enabled()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledComponentServiceEnabledConfig {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledComponentServiceDisabledConfig {}

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

pub struct DefaultCompiledComponentService {
    blob_storage: Arc<dyn BlobStorage>,
}

impl DefaultCompiledComponentService {
    pub fn new(blob_storage: Arc<dyn BlobStorage>) -> Self {
        Self { blob_storage }
    }

    fn key(component_id: ComponentId, component_revision: ComponentRevision) -> PathBuf {
        Path::new(&component_id.to_string()).join(format!("{component_revision}.cwasm"))
    }
}

#[async_trait]
impl CompiledComponentService for DefaultCompiledComponentService {
    async fn get(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        match self
            .blob_storage
            .get_raw(
                "compiled_component",
                "get",
                BlobStorageNamespace::CompilationCache { environment_id },
                &Self::key(component_id, component_revision),
            )
            .await
        {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => {
                let start = Instant::now();
                let component = {
                    let span = info_span!("Loading precompiled WASM component");
                    let _enter = span.enter();

                    let component = unsafe {
                        Component::deserialize(engine, &bytes).map_err(|err| {
                            WorkerExecutorError::component_download_failed(
                                component_id,
                                component_revision,
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
                    component
                };

                Ok(Some(component))
            }
            Err(err) => Err(WorkerExecutorError::component_download_failed(
                component_id,
                component_revision,
                format!("Could not download compiled component: {err}"),
            )),
        }
    }

    async fn put(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component: &Component,
    ) -> Result<(), WorkerExecutorError> {
        let bytes = component
            .serialize()
            .expect("Could not serialize component");
        self.blob_storage
            .put_raw(
                "compiled_component",
                "put",
                BlobStorageNamespace::CompilationCache { environment_id },
                &Self::key(component_id, component_revision),
                &bytes,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::component_download_failed(
                    component_id,
                    component_revision,
                    format!("Could not store compiled component: {err}"),
                )
            })
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
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        Ok(None)
    }

    async fn put(
        &self,
        _environment_id: EnvironmentId,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _component: &Component,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }
}
