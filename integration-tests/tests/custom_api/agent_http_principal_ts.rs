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
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_common::model::http_api_deployment::{
    HttpApiDeploymentAgentSecurity, TestSessionHeaderAgentSecurity,
};
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[test_dep]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    make_test_context(
        deps,
        vec![(
            AgentTypeName("http-agent".to_string()),
            HttpApiDeploymentAgentOptions {
                security: Some(HttpApiDeploymentAgentSecurity::TestSessionHeader(
                    TestSessionHeaderAgentSecurity {
                        header_name: "x-golem-test-session".to_string(),
                    },
                )),
            },
        )],
        "ts_principal",
        "ts:principal",
    )
    .await
    .unwrap()
}

#[test]
#[tracing::instrument]
async fn principal_auto_injection(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/echo-principal")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, json!({ "value": {"anonymous": null} }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn principal_auto_injection_middle_segment(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/echo-principal-mid/foo-value/1")?,
        )
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    assert_eq!(
        body,
        json!({ "value": {"anonymous": null}, "foo": "foo-value", "bar": 1.0 })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn principal_auto_injection_last_segment(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/echo-principal-last/foo-value/2")?,
        )
        .send()
        .await?;

    let body: serde_json::Value = response.json().await?;

    assert_eq!(
        body,
        json!({ "value": {"anonymous": null}, "foo": "foo-value", "bar": 2.0 })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn default_test_header_oidc_principal(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/authed-principal")?,
        )
        .header("x-golem-test-session", "{}")
        .send()
        .await?;

    let mut body: serde_json::Value = response.json().await?;
    // claims include exp, which is not deterministic in tests
    if let Some(oidc) = body
        .get_mut("value")
        .and_then(|v| v.get_mut("oidc"))
        .and_then(|v| v.as_object_mut())
    {
        oidc.remove("claims");
    }

    assert_eq!(
        body,
        json!({
            "value": {
                "oidc": {
                    "email": null,
                    "emailVerified": null,
                    "familyName": null,
                    "givenName": null,
                    "issuer": "http://test-idp.com",
                    "name": null,
                    "picture": null,
                    "preferredUsername": null,
                    "sub": "test-user",
                }
            }
        })
    );

    Ok(())
}

#[test]
#[tracing::instrument]
async fn test_header_oidc_principal_with_overrides(agent: &HttpTestContext) -> anyhow::Result<()> {
    let response = agent
        .client
        .get(
            agent
                .base_url
                .join("/http-agents/test-agent/authed-principal")?,
        )
        .header(
            "x-golem-test-session",
            "{ \"subject\": \"bob\", \"email\": \"bob@golem.cloud\"}",
        )
        .send()
        .await?;

    let mut body: serde_json::Value = response.json().await?;
    // claims include exp, which is not deterministic in tests
    if let Some(oidc) = body
        .get_mut("value")
        .and_then(|v| v.get_mut("oidc"))
        .and_then(|v| v.as_object_mut())
    {
        oidc.remove("claims");
    }

    assert_eq!(
        body,
        json!({
            "value": {
                "oidc": {
                    "email": "bob@golem.cloud",
                    "emailVerified": null,
                    "familyName": null,
                    "givenName": null,
                    "issuer": "http://test-idp.com",
                    "name": null,
                    "picture": null,
                    "preferredUsername": null,
                    "sub": "bob",
                }
            }
        })
    );

    Ok(())
}
