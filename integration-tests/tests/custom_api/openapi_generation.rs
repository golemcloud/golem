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

use crate::custom_api::http_test_context::{HttpTestContext, make_test_context};
use goldenfile::Mint;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use reqwest::header::CONTENT_TYPE;
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
                AgentTypeName("HttpAgent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("CorsAgent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
            (
                AgentTypeName("WebhookAgent".to_string()),
                HttpApiDeploymentAgentOptions::default(),
            ),
        ],
        "golem_it_agent_sdk_rust_release",
        "golem-it:agent-sdk-rust",
    )
    .await
    .unwrap()
}

#[test]
#[tracing::instrument]
async fn test_open_api_yaml_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let mut mint = Mint::new("tests/goldenfiles");
    let mut mint_goldenfile = mint.new_goldenfile("expected_openapi_yaml.yaml")?;

    let response = agent.client.get(agent.base_url.join("/openapi.yaml")?).send().await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_content_type_starts_with(&response, "application/yaml")?;

    let bytes = response.bytes().await?;
    let yaml_value: serde_yaml::Value = serde_yaml::from_slice(&bytes)?;
    let encoded_yaml = serde_yaml::to_string(&yaml_value)?;
    let _ = mint_goldenfile.write(encoded_yaml.as_bytes())?;

    let yaml_as_json = serde_json::to_value(&yaml_value)?;
    assert_openapi_paths_removed(&yaml_as_json);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_json_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent.client.get(agent.base_url.join("/openapi.json")?).send().await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_content_type_starts_with(&response, "application/json")?;

    let bytes = response.bytes().await?;
    let json_value: serde_json::Value = serde_json::from_slice(&bytes)?;
    assert_openapi_paths_removed(&json_value);

    let golden_yaml: serde_yaml::Value =
        serde_yaml::from_str(include_str!("../goldenfiles/expected_openapi_yaml.yaml"))?;
    let golden_as_json = serde_json::to_value(&golden_yaml)?;
    assert_eq!(json_value, golden_as_json);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_json_and_yaml_are_equivalent(agent: &HttpTestContext) -> anyhow::Result<()> {
    let yaml_response = agent.client.get(agent.base_url.join("/openapi.yaml")?).send().await?;
    let json_response = agent.client.get(agent.base_url.join("/openapi.json")?).send().await?;

    assert_eq!(yaml_response.status(), reqwest::StatusCode::OK);
    assert_eq!(json_response.status(), reqwest::StatusCode::OK);

    let yaml_value: serde_yaml::Value = serde_yaml::from_slice(&yaml_response.bytes().await?)?;
    let json_value: serde_json::Value = serde_json::from_slice(&json_response.bytes().await?)?;
    let yaml_as_json = serde_json::to_value(&yaml_value)?;

    assert_eq!(json_value, yaml_as_json);

    Ok(())
}

fn assert_content_type_starts_with(response: &reqwest::Response, expected_prefix: &str) -> anyhow::Result<()> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .ok_or_else(|| anyhow::anyhow!("Missing Content-Type header"))?
        .to_str()?;

    assert!(
        content_type.starts_with(expected_prefix),
        "expected Content-Type starting with {expected_prefix:?}, got {content_type:?}"
    );

    Ok(())
}

fn assert_openapi_paths_removed(document: &serde_json::Value) {
    let paths = document["paths"]
        .as_object()
        .expect("OpenAPI document must have object paths");

    assert!(!paths.contains_key("/openapi.json"));
    assert!(!paths.contains_key("/openapi.yaml"));
}
