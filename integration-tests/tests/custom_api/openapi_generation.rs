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

use crate::custom_api::http_test_context::{make_test_context, HttpTestContext};
use goldenfile::Mint;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use std::io::Write;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    make_test_context(
        deps,
        vec![
            (
                AgentTypeName("http-agent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("cors-agent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("webhook-agent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
        ],
        "http_rust",
        "http:rust",
    )
    .await
    .unwrap()
}

#[test]
#[tracing::instrument]
async fn test_open_api_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let mut mint = Mint::new("tests/goldenfiles");

    let mut mint_goldenfile = mint.new_goldenfile("expected_openapi_yaml.yaml")?;

    let response = agent
        .client
        .get(agent.base_url.join("/openapi.json")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let bytes = response.bytes().await?;
    let decoded_yaml: serde_yaml::Value = serde_yaml::from_slice(&bytes)?;
    let encoded_yaml = serde_yaml::to_string(&decoded_yaml)?;
    let _ = mint_goldenfile.write(encoded_yaml.as_bytes())?;

    Ok(())
}
