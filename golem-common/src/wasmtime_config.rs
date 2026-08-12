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

use wasmtime::{Config, WasmBacktraceDetails};

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

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use wasmtime::Engine;
    use wasmtime::component::Component;

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
}
