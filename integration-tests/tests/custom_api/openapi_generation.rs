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

use crate::custom_api::http_test_context::{test_context_internal, HttpTestContext};
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use serde_yaml::Value;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

// This is based on the http rust agent in test components
const EXPECTED_OPENAPI_YAML: &str = include_str!("test-data/expected-open-api.yaml");

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    test_context_internal(deps, "http_rust", "http:rust")
        .await
        .unwrap()
}

#[test]
#[tracing::instrument]
async fn test_open_api_generation(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(agent.base_url.join("/openapi.json")?)
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let bytes = response.bytes().await?;
    let actual: serde_yaml::Value = serde_json::from_slice(&bytes)?;
    let expected: Value = serde_yaml::from_str(EXPECTED_OPENAPI_YAML)?;

    assert_eq!(actual, expected);

    Ok(())
}
