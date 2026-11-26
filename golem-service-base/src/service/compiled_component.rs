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
    record_compilation_cache_bytes_written, record_compilation_cache_objects_written,
};
use crate::storage::blob::{
    BlobStorage, BlobStorageLabelledApi, BlobStorageNamespace, ExistsResult, join_blob_key,
};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_common::wasmtime_config::wasmtime_artifact_fingerprint;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, info_span};
use wasmtime::Engine;
use wasmtime::component::Component;

/// Service for storing compiled native binaries of WebAssembly components
#[async_trait]
pub trait CompiledComponentService: Send + Sync {
    async fn exists(&self, wasm_hash: Hash, engine: &Engine) -> anyhow::Result<bool>;
    async fn get(
        &self,
        wasm_hash: Hash,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError>;

    /// `wasm_hash` must be the verified hash of the Wasm bytes used to compile `component`.
    async fn put(
        &self,
        environment_id: EnvironmentId,
        wasm_hash: Hash,
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

    pub fn key(wasm_hash: Hash, artifact_fingerprint: &str) -> PathBuf {
        PathBuf::from(join_blob_key(
            &wasm_hash.to_string(),
            &format!("{artifact_fingerprint}.cwasm"),
        ))
    }
}

#[async_trait]
impl CompiledComponentService for DefaultCompiledComponentService {
    async fn exists(&self, wasm_hash: Hash, engine: &Engine) -> anyhow::Result<bool> {
        Ok(self
            .blob_storage
            .with("compiled_component", "exists")
            .exists(
                BlobStorageNamespace::CompilationCache,
                &Self::key(wasm_hash, &wasmtime_artifact_fingerprint(engine)),
            )
            .await?
            == ExistsResult::File)
    }

    async fn get(
        &self,
        wasm_hash: Hash,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        let artifact_fingerprint = wasmtime_artifact_fingerprint(engine);
        match self
            .blob_storage
            .with("compiled_component", "get")
            .get_raw(
                BlobStorageNamespace::CompilationCache,
                &Self::key(wasm_hash, &artifact_fingerprint),
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
        wasm_hash: Hash,
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
            .with("compiled_component", "put")
            .put_raw(
                BlobStorageNamespace::CompilationCache,
                &Self::key(wasm_hash, &artifact_fingerprint),
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
    async fn exists(&self, _: Hash, _: &Engine) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn get(
        &self,
        _wasm_hash: Hash,
        _component_id: ComponentId,
        _component_revision: ComponentRevision,
        _engine: &Engine,
    ) -> Result<Option<Component>, WorkerExecutorError> {
        Ok(None)
    }

    async fn put(
        &self,
        _environment_id: EnvironmentId,
        _wasm_hash: Hash,
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

    #[test]
    async fn compiled_artifact_is_shared_across_environments_but_not_across_wasm()
    -> anyhow::Result<()> {
        let service = DefaultCompiledComponentService::new(Arc::new(InMemoryBlobStorage::new()));
        let engine = engine(OptLevel::Speed)?;
        let original = b"(component)";
        let changed = b"(component (core module (memory 1)))";
        let original_hash = Hash::new(blake3::hash(original));
        let changed_hash = Hash::new(blake3::hash(changed));
        let first = EnvironmentId::new();
        let first_component = ComponentId::new();
        let second_component = ComponentId::new();
        let initial = ComponentRevision::INITIAL;
        let updated = ComponentRevision::new(1).unwrap();
        let original_artifact = Component::new(&engine, original)?;
        service
            .put(
                first,
                original_hash,
                first_component,
                initial,
                &original_artifact,
            )
            .await?;
        assert!(
            service
                .get(original_hash, second_component, initial, &engine)
                .await?
                .is_some()
        );
        assert!(
            service
                .get(changed_hash, first_component, updated, &engine)
                .await?
                .is_none()
        );
        let changed_artifact = Component::new(&engine, changed)?;
        service
            .put(
                first,
                changed_hash,
                first_component,
                updated,
                &changed_artifact,
            )
            .await?;
        assert!(
            service
                .get(changed_hash, first_component, updated, &engine)
                .await?
                .is_some()
        );
        assert!(
            service
                .get(original_hash, second_component, initial, &engine)
                .await?
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn compiled_component_key_uses_contract_separator() {
        let wasm_hash = Hash::empty();
        let expected = format!("{wasm_hash}/fingerprint.cwasm");
        assert_eq!(
            DefaultCompiledComponentService::key(wasm_hash, "fingerprint").to_str(),
            Some(expected.as_str())
        );
    }

    #[test]
    async fn incompatible_engine_artifacts_are_not_cross_read_and_can_coexist() -> anyhow::Result<()>
    {
        let blob_storage = Arc::new(InMemoryBlobStorage::new());
        let service = DefaultCompiledComponentService::new(blob_storage);
        let unoptimized_engine = engine(OptLevel::None)?;
        let optimized_engine = engine(OptLevel::Speed)?;
        let environment_id = EnvironmentId::new();
        let component_id = ComponentId::new();
        let component_revision = ComponentRevision::INITIAL;
        let wasm_hash = Hash::new(blake3::hash(b"(component)"));

        assert_ne!(
            wasmtime_artifact_fingerprint(&unoptimized_engine),
            wasmtime_artifact_fingerprint(&optimized_engine)
        );

        let unoptimized_component = Component::new(&unoptimized_engine, b"(component)")?;
        service
            .put(
                environment_id,
                wasm_hash,
                component_id,
                component_revision,
                &unoptimized_component,
            )
            .await?;

        assert!(
            service
                .get(
                    wasm_hash,
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
                wasm_hash,
                component_id,
                component_revision,
                &optimized_component,
            )
            .await?;

        for engine in [&unoptimized_engine, &optimized_engine] {
            assert!(
                service
                    .get(wasm_hash, component_id, component_revision, engine,)
                    .await?
                    .is_some()
            );
        }

        Ok(())
    }
}
