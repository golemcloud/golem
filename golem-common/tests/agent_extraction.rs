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
use golem_common::schema::agent::{AgentTypeKind, AgentTypeSchema};
use golem_common::wasmtime_config::create_wasmtime_config_without_fs_cache;
use std::path::PathBuf;
use std::str::FromStr;
use test_r::test;

test_r::enable!();

fn assert_valid_regular_agent_types(agent_types: &[AgentTypeSchema]) {
    assert!(!agent_types.is_empty());
    for agent_type in agent_types {
        assert!(agent_type.kind == AgentTypeKind::Regular);
        agent_type.validate().unwrap();
        if let Some(mount) = &agent_type.http_mount {
            assert!(mount.static_bindings.is_empty());
            assert!(mount.filesystem_bindings.is_empty());
            assert!(mount.openapi_provider.is_none());
        }
    }
}

#[test]
async fn can_extract_agent_type_schemas_from_component_with_dynamic_rpc() -> anyhow::Result<()> {
    let agent_types = extract_agent_type_schemas(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc_rust_release.wasm")?,
        false,
        false,
    )
    .await?;
    assert_valid_regular_agent_types(&agent_types);
    Ok(())
}

#[test]
async fn can_extract_agent_type_schemas_2() -> anyhow::Result<()> {
    let agent_types = extract_agent_type_schemas(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc.wasm")?,
        false,
        false,
    )
    .await?;
    assert_valid_regular_agent_types(&agent_types);
    Ok(())
}

#[test]
async fn can_extract_http_mounts_from_rust_and_typescript_components() -> anyhow::Result<()> {
    for artifact in [
        "golem_it_agent_sdk_rust_release.wasm",
        "golem_it_agent_sdk_ts.wasm",
    ] {
        let agent_types = extract_agent_type_schemas(
            &PathBuf::from("../test-components").join(artifact),
            false,
            false,
        )
        .await?;
        assert_valid_regular_agent_types(&agent_types);
        let agent = agent_types
            .iter()
            .find(|agent| agent.type_name.0 == "HttpAgent")
            .expect("fixture must export HttpAgent");
        let mount = agent
            .http_mount
            .as_ref()
            .expect("HttpAgent must be mounted");
        assert!(mount.path_prefix.len() == 2);
        assert!(mount.static_bindings.is_empty());
        assert!(mount.filesystem_bindings.is_empty());
        assert!(mount.openapi_provider.is_none());
    }
    Ok(())
}

#[test]
async fn can_extract_agent_type_schemas_from_component_importing_p3_http() -> anyhow::Result<()> {
    let wasm_path = PathBuf::from_str("../test-components/golem_it_http_tests_release.wasm")?;

    // Guard: the fixture must actually import P3 `wasi:http`, otherwise a
    // future rebuild of the component would silently defeat the purpose of
    // this regression test.
    let config = create_wasmtime_config_without_fs_cache();
    let engine = wasmtime::Engine::new(&config)?;
    let component = wasmtime::component::Component::from_file(&engine, &wasm_path)?;
    let imports_p3_http = component
        .component_type()
        .imports(&engine)
        .any(|(name, _)| name.starts_with("wasi:http/") && name.contains("@0.3."));
    assert!(imports_p3_http);

    let agent_types = extract_agent_type_schemas(&wasm_path, false, false).await?;
    assert_valid_regular_agent_types(&agent_types);
    Ok(())
}
