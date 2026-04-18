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

use crate::custom_api::http_test_context::{
    HttpTestContext, make_test_context, make_test_context_with_openapi_endpoint,
};
use goldenfile::Mint;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use std::io::Write;
use std::ops::Deref;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[derive(Debug)]
struct CustomPrefixHttpTestContext(HttpTestContext);

impl Deref for CustomPrefixHttpTestContext {
    type Target = HttpTestContext;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

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

#[test_dep]
async fn custom_prefix_test_context(
    deps: &EnvBasedTestDependencies,
) -> CustomPrefixHttpTestContext {
    CustomPrefixHttpTestContext(
        make_test_context_with_openapi_endpoint(
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
            Some("/docs".to_string()),
        )
        .await
        .unwrap(),
    )
}

async fn fetch_openapi_yaml(
    agent: &HttpTestContext,
    path: &str,
) -> anyhow::Result<serde_yaml::Value> {
    let response = agent.client.get(agent.base_url.join(path)?).send().await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/yaml"))
    );

    Ok(serde_yaml::from_slice(&response.bytes().await?)?)
}

async fn fetch_openapi_json(
    agent: &HttpTestContext,
    path: &str,
) -> anyhow::Result<serde_json::Value> {
    let response = agent.client.get(agent.base_url.join(path)?).send().await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"))
    );

    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

async fn assert_openapi_json_and_yaml_are_equivalent(
    agent: &HttpTestContext,
    yaml_path: &str,
    json_path: &str,
) -> anyhow::Result<()> {
    let yaml_value = fetch_openapi_yaml(agent, yaml_path).await?;
    let json_value = fetch_openapi_json(agent, json_path).await?;
    let yaml_as_json = serde_json::to_value(&yaml_value)?;

    assert_eq!(json_value, yaml_as_json);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_yaml_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let mut mint = Mint::new("tests/goldenfiles");
    let mut mint_goldenfile = mint.new_goldenfile("expected_openapi_yaml.yaml")?;

    let yaml_value = fetch_openapi_yaml(agent, "/openapi.yaml").await?;
    let encoded_yaml = serde_yaml::to_string(&yaml_value)?;
    let _ = mint_goldenfile.write(encoded_yaml.as_bytes())?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_json_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let mut mint = Mint::new("tests/goldenfiles");
    let mut mint_goldenfile = mint.new_goldenfile("expected_openapi_json.json")?;

    let json_value = fetch_openapi_json(agent, "/openapi.json").await?;
    let encoded_json = serde_json::to_string_pretty(&json_value)?;
    let _ = mint_goldenfile.write(encoded_json.as_bytes())?;

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_json_and_yaml_are_equivalent(agent: &HttpTestContext) -> anyhow::Result<()> {
    assert_openapi_json_and_yaml_are_equivalent(agent, "/openapi.yaml", "/openapi.json").await
}

#[test]
#[tracing::instrument]
async fn test_open_api_custom_prefix_json_and_yaml_are_equivalent(
    agent: &CustomPrefixHttpTestContext,
) -> anyhow::Result<()> {
    assert_openapi_json_and_yaml_are_equivalent(agent, "/docs/openapi.yaml", "/docs/openapi.json")
        .await
}

#[test]
#[tracing::instrument]
async fn test_open_api_custom_prefix_moves_routes(
    agent: &CustomPrefixHttpTestContext,
) -> anyhow::Result<()> {
    let _ = fetch_openapi_yaml(agent, "/docs/openapi.yaml").await?;
    let _ = fetch_openapi_json(agent, "/docs/openapi.json").await?;

    let root_yaml_response = agent
        .client
        .get(agent.base_url.join("/openapi.yaml")?)
        .send()
        .await?;
    let root_json_response = agent
        .client
        .get(agent.base_url.join("/openapi.json")?)
        .send()
        .await?;

    assert_eq!(root_yaml_response.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(root_json_response.status(), reqwest::StatusCode::NOT_FOUND);

    Ok(())
}
