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

    // Wasmtime 50 needs this budget for the Bash tool's bounded recursion. Keep the
    // default 1.5 MiB of additional fiber stack available to host functions.
    config.max_wasm_stack(1024 * 1024);
    config.async_stack_size(2560 * 1024);
    config.wasm_multi_value(true);
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_error_context(true);
    config.wasm_gc(false);
    config.wasm_function_references(false);
    config.wasm_exceptions(false);
    config.wasm_wide_arithmetic(false);
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

    /// Golem leaves the WebAssembly GC proposal disabled, so Wasmtime never allocates a GC
    /// heap and `MemoryKind::GcHeap` never reaches a resource limiter.
    ///
    /// `DurableResourceLimiter` in golem-worker-executor relies on that: it admits GC heap
    /// growth without taking a linear-memory admission grant, because the collector's capacity
    /// is not memory the guest declared and Golem's per-agent memory accounting is keyed to
    /// guest linear memory. Enabling GC would make this test fail, which is the point -- that
    /// accounting decision has to be revisited before agents can allocate GC objects.
    #[test]
    fn gc_proposal_is_disabled() {
        let engine = Engine::new(&create_wasmtime_config_without_fs_cache()).unwrap();
        // A `struct` type definition is only valid with the GC proposal enabled.
        let error = Module::new(&engine, "(module (type (struct (field i32))))")
            .expect_err("the GC proposal is expected to be disabled");
        assert!(
            format!("{error:?}").contains("gc"),
            "expected a GC-proposal rejection, got: {error:?}"
        );
    }

    #[test]
    fn additional_proposals_remain_disabled() -> anyhow::Result<()> {
        let engine = Engine::new(&create_wasmtime_config_without_fs_cache())?;
        let mut enabled_config = create_wasmtime_config_without_fs_cache();
        enabled_config
            .wasm_function_references(true)
            .wasm_exceptions(true)
            .wasm_wide_arithmetic(true);
        let enabled_engine = Engine::new(&enabled_config)?;

        for module in [
            "(module (type $f (func)) (func (param (ref null $f))))",
            "(module (tag (param i32)))",
            "(module (func (param i64 i64) (result i64 i64) local.get 0 local.get 1 i64.mul_wide_u))",
        ] {
            Module::new(&enabled_engine, module)?;
            assert!(Module::new(&engine, module).is_err(), "accepted {module}");
        }

        Ok(())
    }
}
