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

use std::hash::{Hash, Hasher};
use wasmtime::{Config, Engine, WasmBacktraceDetails};

#[derive(Default)]
struct ArtifactFingerprintHasher(blake3::Hasher);

impl Hasher for ArtifactFingerprintHasher {
    fn finish(&self) -> u64 {
        let hash = self.0.clone().finalize();
        u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap())
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

/// Creates the Wasmtime configuration shared by every Golem component engine.
///
/// A component compiled by one Golem service can be deserialized by another,
/// so compilation-affecting settings must not be configured independently at
/// individual call sites.
pub fn create_wasmtime_config(enable_fs_cache: bool) -> Config {
    let mut config = Config::default();

    config.wasm_multi_value(true);
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_error_context(true);
    // Required to run native Kotlin/Wasm (WasmGC) agent components; all three are mandatory.
    // Additive and deterministic (no oplog-replay impact).
    config.wasm_gc(true);
    config.wasm_function_references(true);
    config.wasm_exceptions(true);
    config.wasm_threads(false);
    config.shared_memory(false);
    config.epoch_interruption(true);
    config.consume_fuel(true);
    config.concurrency_support(true);
    config.wasm_backtrace_details(WasmBacktraceDetails::Enable);

    if enable_fs_cache {
        config.cache(Some(
            wasmtime::Cache::new(wasmtime::CacheConfig::new()).expect("Failed to initialize cache"),
        ));
    }

    config
}

pub fn create_wasmtime_config_with_fs_cache() -> Config {
    create_wasmtime_config(true)
}

pub fn create_wasmtime_config_without_fs_cache() -> Config {
    create_wasmtime_config(false)
}

/// Returns a stable fingerprint for Wasmtime engines that can share precompiled artifacts.
pub fn wasmtime_artifact_fingerprint(engine: &Engine) -> String {
    let mut hasher = ArtifactFingerprintHasher::default();
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.0.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use wasmtime::component::Component;
    use wasmtime::{Engine, Module, OptLevel};

    #[test]
    fn precompiled_components_are_compatible_across_engines() -> anyhow::Result<()> {
        let compilation_engine = Engine::new(&create_wasmtime_config_with_fs_cache())?;
        let precompiled_component = compilation_engine.precompile_component(b"(component)")?;
        let executor_engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;

        // SAFETY: these bytes were produced by Wasmtime in this process and are
        // deserialized without being modified.
        unsafe { Component::deserialize(&executor_engine, precompiled_component) }?;

        Ok(())
    }

    /// A component that uses every artifact-affecting feature Kotlin/Wasm needs: a GC struct type,
    /// a typed function reference called through `call_ref`, and an exception tag that is thrown and
    /// caught. Each one is required rather than incidental: `wasm-tools validate` rejects this
    /// component with any of `gc`, `exceptions` or `function-references` turned off.
    const KOTLIN_FEATURE_COMPONENT: &str = r#"
        (component
          (core module $m
            (type $point (struct (field i32)))
            (type $thunk (func (result i32)))
            (tag $failure)
            (func $answer (type $thunk) (i32.const 42))
            (elem declare func $answer)
            (func (export "run") (result i32)
              (drop (struct.new $point (i32.const 7)))
              (block $caught
                (try_table (catch_all $caught)
                  (throw $failure)
                )
              )
              (call_ref $thunk (ref.func $answer))
            )
          )
          (core instance (instantiate $m))
        )
    "#;

    /// The compilation service precompiles and the worker executor deserializes, so the two engines
    /// must agree on every artifact-affecting setting. An empty component cannot show that: it uses
    /// none of the features a real guest does, and the serialized artifact records them. This
    /// compiles a component that needs all three Kotlin/Wasm features through the same engine pair.
    ///
    /// A failure here is not a crash in production but a silent cost: the executor rejects the
    /// cached artifact and compiles the component again, which is a severe cold-start regression.
    #[test]
    fn precompiled_components_with_gc_and_exceptions_are_compatible_across_engines()
    -> anyhow::Result<()> {
        let compilation_engine = Engine::new(&create_wasmtime_config_with_fs_cache())?;
        let precompiled_component =
            compilation_engine.precompile_component(KOTLIN_FEATURE_COMPONENT.as_bytes())?;
        let executor_engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;

        // SAFETY: these bytes were produced by Wasmtime in this process and are
        // deserialized without being modified.
        unsafe { Component::deserialize(&executor_engine, precompiled_component) }?;

        Ok(())
    }

    #[test]
    fn artifact_fingerprint_ignores_filesystem_cache_configuration() -> anyhow::Result<()> {
        let with_cache = Engine::new(&create_wasmtime_config_with_fs_cache())?;
        let without_cache = Engine::new(&create_wasmtime_config_without_fs_cache())?;

        assert_eq!(
            wasmtime_artifact_fingerprint(&with_cache),
            wasmtime_artifact_fingerprint(&without_cache)
        );

        Ok(())
    }

    #[test]
    fn artifact_fingerprint_changes_with_compilation_configuration() -> anyhow::Result<()> {
        let mut unoptimized_config = create_wasmtime_config_without_fs_cache();
        unoptimized_config.cranelift_opt_level(OptLevel::None);
        let unoptimized = Engine::new(&unoptimized_config)?;

        let mut optimized_config = create_wasmtime_config_without_fs_cache();
        optimized_config.cranelift_opt_level(OptLevel::Speed);
        let optimized = Engine::new(&optimized_config)?;

        assert_ne!(
            wasmtime_artifact_fingerprint(&unoptimized),
            wasmtime_artifact_fingerprint(&optimized)
        );

        Ok(())
    }

    /// Golem enables the WebAssembly GC proposal, which Kotlin/Wasm guests require.
    ///
    /// Wasmtime grows its internal GC heaps through the same resource-limiter callbacks as
    /// linear memory, tagged `MemoryKind::GcHeap`. `DurableResourceLimiter` in
    /// golem-worker-executor admits that growth without taking an admission grant, because
    /// Golem's per-agent memory accounting is keyed to guest linear memory. A guest whose
    /// objects live in the GC heap therefore grows outside that accounting, and reconciling the
    /// two is open work.
    #[test]
    fn gc_proposal_is_enabled() {
        let engine = Engine::new(&create_wasmtime_config_without_fs_cache()).unwrap();
        // A `struct` type definition is only valid with the GC proposal enabled.
        Module::new(&engine, "(module (type (struct (field i32))))")
            .expect("the GC proposal is expected to be enabled");
    }
}
