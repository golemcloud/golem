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
use golem_common::model::agent::extraction::extract_agent_types;
use std::path::PathBuf;
use std::str::FromStr;
use test_r::test;

test_r::enable!();

#[test]
async fn can_extract_agent_types_from_component_with_dynamic_rpc() -> anyhow::Result<()> {
    let result = extract_agent_types(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc_rust_release.wasm")?,
        false,
        false,
    )
    .await;
    assert!(let Ok(_) = result);
    Ok(())
}

#[test]
async fn can_extract_agent_types_2() -> anyhow::Result<()> {
    let result = extract_agent_types(
        &PathBuf::from_str("../test-components/golem_it_agent_rpc.wasm")?,
        false,
        false,
    )
    .await;
    assert!(let Ok(_) = result);
    Ok(())
}
