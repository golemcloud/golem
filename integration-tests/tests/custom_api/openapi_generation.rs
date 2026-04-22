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

    let yaml_response = agent
        .client
        .get(agent.base_url.join("/openapi.yaml")?)
        .send()
        .await?;

    assert_eq!(yaml_response.status(), reqwest::StatusCode::OK);
    assert!(
        yaml_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/yaml"))
    );

    let yaml_value: serde_yaml::Value = serde_yaml::from_slice(&yaml_response.bytes().await?)?;
    let encoded_yaml = serde_yaml::to_string(&yaml_value)?;
    let _ = mint_goldenfile.write(encoded_yaml.as_bytes())?;

    let json_response = agent
        .client
        .get(agent.base_url.join("/openapi.json")?)
        .send()
        .await?;

    assert_eq!(json_response.status(), reqwest::StatusCode::OK);
    assert!(
        json_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"))
    );

    let json_value: serde_json::Value = serde_json::from_slice(&json_response.bytes().await?)?;
    let yaml_as_json = serde_json::to_value(&yaml_value)?;

    assert_eq!(json_value, yaml_as_json);

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_open_api_custom_prefix_json_generation(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    let mut mint = Mint::new("tests/goldenfiles");
    let mut mint_goldenfile = mint.new_goldenfile("expected_openapi_json.json")?;

    let agent = make_test_context_with_openapi_endpoint(
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
        "/docs".to_string(),
    )
    .await?;

    let json_response = agent
        .client
        .get(agent.base_url.join("/docs/openapi.json")?)
        .send()
        .await?;

    assert_eq!(json_response.status(), reqwest::StatusCode::OK);
    assert!(
        json_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"))
    );

    let json_value: serde_json::Value = serde_json::from_slice(&json_response.bytes().await?)?;
    let encoded_json = serde_json::to_string_pretty(&json_value)?;
    let _ = mint_goldenfile.write(encoded_json.as_bytes())?;

    let yaml_response = agent
        .client
        .get(agent.base_url.join("/docs/openapi.yaml")?)
        .send()
        .await?;

    assert_eq!(yaml_response.status(), reqwest::StatusCode::OK);
    assert!(
        yaml_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/yaml"))
    );

    let yaml_value: serde_yaml::Value = serde_yaml::from_slice(&yaml_response.bytes().await?)?;
    let yaml_as_json = serde_json::to_value(&yaml_value)?;

    assert_eq!(json_value, yaml_as_json);

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
