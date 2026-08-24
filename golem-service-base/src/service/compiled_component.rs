// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use crate::metrics::storage::{
    record_compilation_cache_bytes_written, record_compilation_cache_get,
    record_compilation_cache_objects_written,
};
use crate::storage::blob::{BlobStorage, BlobStorageNamespace};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::environment::EnvironmentId;
use golem_common::wasmtime_config::wasmtime_artifact_fingerprint;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, info_span, warn};
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

    fn key(
        component_id: ComponentId,
        component_revision: ComponentRevision,
        artifact_fingerprint: &str,
    ) -> PathBuf {
        Path::new(&component_id.to_string())
            .join(component_revision.to_string())
            .join(format!("{artifact_fingerprint}.cwasm"))
    }

    fn legacy_key(component_id: ComponentId, component_revision: ComponentRevision) -> PathBuf {
        Path::new(&component_id.to_string()).join(format!("{component_revision}.cwasm"))
    }

    async fn get_at_key(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        engine: &Engine,
        artifact_fingerprint: &str,
        key_format: &'static str,
        key: &Path,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        let environment_id_string = environment_id.to_string();
        let bytes = match self
            .blob_storage
            .get_raw(
                "compiled_component",
                "get",
                BlobStorageNamespace::CompilationCache { environment_id },
                key,
            )
            .await
        {
            Ok(None) => {
                record_compilation_cache_get(
                    &environment_id_string,
                    artifact_fingerprint,
                    key_format,
                    "miss",
                );
                return Ok(None);
            }
            Ok(Some(bytes)) => bytes,
            Err(err) => {
                record_compilation_cache_get(
                    &environment_id_string,
                    artifact_fingerprint,
                    key_format,
                    "read_error",
                );
                return Err(WorkerExecutorError::component_download_failed(
                    component_id,
                    component_revision,
                    format!("Could not download compiled component: {err}"),
                ));
            }
        };

        let start = Instant::now();
        let span = info_span!(
            "Loading precompiled WASM component",
            artifact_fingerprint,
            key_format
        );
        let _enter = span.enter();

        let component = match unsafe { Component::deserialize(engine, &bytes) } {
            Ok(component) => component,
            Err(err) => {
                record_compilation_cache_get(
                    &environment_id_string,
                    artifact_fingerprint,
                    key_format,
                    "deserialize_error",
                );
                return Err(WorkerExecutorError::component_download_failed(
                    component_id,
                    component_revision,
                    format!("Could not deserialize compiled component: {err}"),
                ));
            }
        };

        record_compilation_cache_get(
            &environment_id_string,
            artifact_fingerprint,
            key_format,
            "hit",
        );
        debug!(
            component_id = %component_id,
            artifact_fingerprint,
            key_format,
            load_time_ms = start.elapsed().as_millis(),
            "Loaded precompiled component"
        );

        Ok(Some(component))
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
        let artifact_fingerprint = wasmtime_artifact_fingerprint(engine);
        if let Some(component) = self
            .get_at_key(
                environment_id,
                component_id,
                component_revision,
                engine,
                &artifact_fingerprint,
                "fingerprinted",
                &Self::key(component_id, component_revision, &artifact_fingerprint),
            )
            .await?
        {
            return Ok(Some(component));
        }

        let legacy_component = self
            .get_at_key(
                environment_id,
                component_id,
                component_revision,
                engine,
                &artifact_fingerprint,
                "legacy",
                &Self::legacy_key(component_id, component_revision),
            )
            .await?;

        if let Some(component) = &legacy_component
            && let Err(error) = self
                .put(environment_id, component_id, component_revision, component)
                .await
        {
            warn!(
                component_id = %component_id,
                component_revision = %component_revision,
                artifact_fingerprint,
                error = %error,
                "Failed to promote legacy compiled component"
            );
        }

        Ok(legacy_component)
    }

    async fn put(
        &self,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        component: &Component,
    ) -> Result<(), WorkerExecutorError> {
        let artifact_fingerprint = wasmtime_artifact_fingerprint(component.engine());
        let bytes = component
            .serialize()
            .expect("Could not serialize component");
        let byte_count = bytes.len() as u64;
        let result = self
            .blob_storage
            .put_raw(
                "compiled_component",
                "put",
                BlobStorageNamespace::CompilationCache { environment_id },
                &Self::key(component_id, component_revision, &artifact_fingerprint),
                &bytes,
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::component_download_failed(
                    component_id,
                    component_revision,
                    format!("Could not store compiled component: {err}"),
                )
            });
        if result.is_ok() {
            let env_str = environment_id.to_string();
            record_compilation_cache_bytes_written(&env_str, byte_count);
            record_compilation_cache_objects_written(&env_str, 1);
        }
        result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::blob::memory::InMemoryBlobStorage;
    use golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache;
    use test_r::test;
    use wasmtime::OptLevel;

    fn engine(opt_level: OptLevel) -> anyhow::Result<Engine> {
        let mut config = create_wasmtime_config_without_fs_cache();
        config.cranelift_opt_level(opt_level);
        Ok(Engine::new(&config)?)
    }

    fn namespace(environment_id: EnvironmentId) -> BlobStorageNamespace {
        BlobStorageNamespace::CompilationCache { environment_id }
    }

    #[test]
    async fn writes_fingerprinted_artifacts_without_writing_legacy_key() -> anyhow::Result<()> {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let service = DefaultCompiledComponentService::new(blob_storage.clone());
        let engine = engine(OptLevel::Speed)?;
        let component = Component::new(&engine, b"(component)")?;
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::INITIAL;
        let artifact_fingerprint = wasmtime_artifact_fingerprint(&engine);

        service
            .put(environment_id, component_id, component_revision, &component)
            .await?;

        assert!(
            blob_storage
                .get_raw(
                    "test",
                    "get",
                    namespace(environment_id),
                    &DefaultCompiledComponentService::key(
                        component_id,
                        component_revision,
                        &artifact_fingerprint,
                    ),
                )
                .await?
                .is_some()
        );
        assert!(
            blob_storage
                .get_raw(
                    "test",
                    "get",
                    namespace(environment_id),
                    &DefaultCompiledComponentService::legacy_key(component_id, component_revision,),
                )
                .await?
                .is_none()
        );

        Ok(())
    }

    #[test]
    async fn legacy_reads_are_promoted_to_fingerprinted_key() -> anyhow::Result<()> {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let service = DefaultCompiledComponentService::new(blob_storage.clone());
        let engine = engine(OptLevel::Speed)?;
        let component = Component::new(&engine, b"(component)")?;
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::INITIAL;
        let legacy_key =
            DefaultCompiledComponentService::legacy_key(component_id, component_revision);

        blob_storage
            .put_raw(
                "test",
                "put",
                namespace(environment_id),
                &legacy_key,
                &component.serialize()?,
            )
            .await?;

        assert!(
            service
                .get(environment_id, component_id, component_revision, &engine,)
                .await?
                .is_some()
        );

        let artifact_fingerprint = wasmtime_artifact_fingerprint(&engine);
        assert!(
            blob_storage
                .get_raw(
                    "test",
                    "get",
                    namespace(environment_id),
                    &DefaultCompiledComponentService::key(
                        component_id,
                        component_revision,
                        &artifact_fingerprint,
                    ),
                )
                .await?
                .is_some()
        );
        assert!(
            blob_storage
                .get_raw("test", "get", namespace(environment_id), &legacy_key,)
                .await?
                .is_some()
        );

        Ok(())
    }

    #[test]
    async fn incompatible_engine_artifacts_are_not_cross_read_and_can_coexist() -> anyhow::Result<()>
    {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let service = DefaultCompiledComponentService::new(blob_storage.clone());
        let unoptimized_engine = engine(OptLevel::None)?;
        let optimized_engine = engine(OptLevel::Speed)?;
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::INITIAL;

        let unoptimized_component = Component::new(&unoptimized_engine, b"(component)")?;
        service
            .put(
                environment_id,
                component_id,
                component_revision,
                &unoptimized_component,
            )
            .await?;

        assert!(
            service
                .get(
                    environment_id,
                    component_id,
                    component_revision,
                    &optimized_engine,
                )
                .await?
                .is_none()
        );

        let optimized_component = Component::new(&optimized_engine, b"(component)")?;
        service
            .put(
                environment_id,
                component_id,
                component_revision,
                &optimized_component,
            )
            .await?;

        let unoptimized_fingerprint = wasmtime_artifact_fingerprint(&unoptimized_engine);
        let optimized_fingerprint = wasmtime_artifact_fingerprint(&optimized_engine);
        assert_ne!(unoptimized_fingerprint, optimized_fingerprint);

        for artifact_fingerprint in [unoptimized_fingerprint, optimized_fingerprint] {
            assert!(
                blob_storage
                    .get_raw(
                        "test",
                        "get",
                        namespace(environment_id),
                        &DefaultCompiledComponentService::key(
                            component_id,
                            component_revision,
                            &artifact_fingerprint,
                        ),
                    )
                    .await?
                    .is_some()
            );
        }

        Ok(())
    }

    #[test]
    async fn compilation_cache_remains_isolated_by_environment() -> anyhow::Result<()> {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let service = DefaultCompiledComponentService::new(blob_storage);
        let engine = engine(OptLevel::Speed)?;
        let component = Component::new(&engine, b"(component)")?;
        let source_environment_id = EnvironmentId::new();
        let other_environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::INITIAL;

        service
            .put(
                source_environment_id,
                component_id,
                component_revision,
                &component,
            )
            .await?;

        assert!(
            service
                .get(
                    other_environment_id,
                    component_id,
                    component_revision,
                    &engine,
                )
                .await?
                .is_none()
        );

        Ok(())
    }
}
