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

use assert2::assert;
use golem_common::model::agent::extraction::extract_agent_type_schemas;
use std::path::PathBuf;
use std::str::FromStr;
use test_r::test;

test_r::enable!();

#[test]
async fn can_extract_agent_type_schemas_from_component_with_dynamic_rpc() -> anyhow::Result<()> {
    let result = extract_agent_type_schemas(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc_rust_release.wasm")?,
        false,
        false,
    )
    .await;
    assert!(let Ok(_) = result);
    Ok(())
}

#[test]
async fn can_extract_agent_type_schemas_2() -> anyhow::Result<()> {
    let result = extract_agent_type_schemas(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc.wasm")?,
        false,
        false,
    )
    .await;
    assert!(let Ok(_) = result);
    Ok(())
}

#[test]
async fn can_extract_agent_type_schemas_from_component_importing_p3_http() -> anyhow::Result<()> {
    let wasm_path = PathBuf::from_str("../test-components/golem_it_http_tests_release.wasm")?;

    // Guard: the fixture must actually import P3 `wasi:http`, otherwise a
    // future rebuild of the component would silently defeat the purpose of
    // this regression test.
    let mut config = wasmtime::Config::default();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    config.wasm_component_model_error_context(true);
    let engine = wasmtime::Engine::new(&config)?;
    let component = wasmtime::component::Component::from_file(&engine, &wasm_path)?;
    let imports_p3_http = component
        .component_type()
        .imports(&engine)
        .any(|(name, _)| name.starts_with("wasi:http/") && name.contains("@0.3."));
    assert!(imports_p3_http);

    let result = extract_agent_type_schemas(&wasm_path, false, false).await;
    assert!(let Ok(_) = &result);
    assert!(!result?.is_empty());
    Ok(())
}
