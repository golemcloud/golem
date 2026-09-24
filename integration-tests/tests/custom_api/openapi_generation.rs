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
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_deployment::HttpApiDeploymentAgentOptions;
use golem_test_framework::config::EnvBasedTestDependencies;
use pretty_assertions::assert_eq;
use std::io::Write;
use test_r::{define_matrix_dimension, test_dep};
use test_r::{inherit_test_dep, test};

/// Normalizes server URLs in OpenAPI spec strings by replacing dynamic domains with placeholders
fn normalize_server_servers_urls_in_openapi_spec(spec_str: &str, env: &EnvironmentId) -> String {
    let dynamic_domain = format!("{}.golem.cloud", env.0);
    spec_str.replace(&dynamic_domain, "<ENV_ID>.golem.cloud")
}

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(
    #[tagged_as("postgres")]
    EnvBasedTestDependencies
);
inherit_test_dep!(
    #[tagged_as("sqlite")]
    EnvBasedTestDependencies
);

async fn build_test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
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

#[test_dep(scope = PerWorker)]
async fn test_context(deps: &EnvBasedTestDependencies) -> HttpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "postgres")]
async fn test_context_postgres(
    #[tagged_as("postgres")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    build_test_context(deps).await
}

#[test_dep(scope = PerWorker, tagged_as = "sqlite")]
async fn test_context_sqlite(
    #[tagged_as("sqlite")] deps: &EnvBasedTestDependencies,
) -> HttpTestContext {
    build_test_context(deps).await
}

define_matrix_dimension!(db: HttpTestContext -> "postgres", "sqlite");

#[test]
#[tracing::instrument]
async fn test_open_api_yaml_generation(
    #[dimension(db)] agent: &HttpTestContext,
) -> anyhow::Result<()> {
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
    let encoded_yaml_normalized =
        normalize_server_servers_urls_in_openapi_spec(&encoded_yaml, &agent.env_id);
    let _ = mint_goldenfile.write(encoded_yaml_normalized.as_bytes())?;

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
    let encoded_json_normalized =
        normalize_server_servers_urls_in_openapi_spec(&encoded_json, &agent.env_id);
    let _ = mint_goldenfile.write(encoded_json_normalized.as_bytes())?;

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

#[test]
#[test_r::timeout("180s")]
async fn openapi_provider_context_cache_and_failure_corpus(
    deps: &EnvBasedTestDependencies,
) -> anyhow::Result<()> {
    use crate::custom_api::http_test_context::make_test_context_with_files;
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use golem_test_framework::model::IFSEntry;
    use serde_json::{Value, json};
    use std::time::Duration;

    let corpus: Value = serde_json::from_str(include_str!(
        "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))?;
    let case = |id: &str| {
        corpus["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["id"] == id)
            .unwrap()
    };
    let fixed = case("cache-fixed-provider-context");
    let isolated = case("cache-provider-failure-isolated");
    let component = "golem_it_agent_rpc_rust_release";
    let context = make_test_context_with_files(
        deps,
        vec![(
            AgentTypeName("StaticHttpRouter".into()),
            HttpApiDeploymentAgentOptions::default(),
        )],
        component,
        "golem-it:agent-rpc-rust",
        String::new(),
        &[(
            "StaticHttpRouter",
            vec![IFSEntry {
                source_path: "initial-file-system/files/foo.txt".into(),
                target_path: CanonicalFilePath::from_abs_str("/assets/asset.txt")
                    .map_err(anyhow::Error::msg)?,
                permissions: AgentFilePermissions::ReadOnly,
            }],
        )],
    )
    .await?;
    let selected = fixed["input"]["component_revision"].as_u64().unwrap();
    let mut current = context
        .user
        .get_latest_component_revision(&context.component_id)
        .await?;
    while current.revision.get() < selected {
        // Distinct provision configuration makes each upload a deployable change.
        current = context
            .user
            .update_component_with_env(
                &context.component_id,
                "StaticHttpRouter",
                component,
                &[(
                    "TEST_OPENAPI_REVISION".into(),
                    current.revision.next()?.to_string(),
                )],
            )
            .await?;
    }
    assert_eq!(current.revision.get(), selected);
    context.user.deploy_environment(context.env_id).await?;
    let latest = context
        .user
        .update_component_with_env(
            &context.component_id,
            "StaticHttpRouter",
            component,
            &[("TEST_OPENAPI_INVALID".into(), "true".into())],
        )
        .await?;
    assert_eq!(
        json!(latest.revision),
        fixed["input"]["latest_component_revision"]
    );

    // The uploaded revision deliberately has a broken provider. The provider
    // must still run on the deployed revision.
    for path in ["/raw/empty", "/raw/favicon"] {
        let response = context
            .client
            .get(context.base_url.join(path)?)
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }
    let json_response = context
        .client
        .get(context.base_url.join("/openapi.json")?)
        .header("x-end-user", "alice")
        .header("cookie", "untrusted=alice")
        .send()
        .await?;
    let status = json_response.status();
    let body = json_response.text().await?;
    assert_eq!(status, reqwest::StatusCode::OK, "{}: {body}", fixed["id"]);
    let json: Value = serde_json::from_str(&body)?;
    assert_eq!(
        json["x-provider-revision"],
        fixed["expect"]["component_revision"]
    );
    assert!(
        json["x-provider-agent"]
            .as_str()
            .unwrap()
            .starts_with("StaticHttpRouter()[")
    );
    assert_eq!(
        json["paths"]["/raw/echo/"]["post"]["operationId"],
        "rawEcho"
    );
    assert!(json["paths"]["/raw"]["get"].is_object());
    let yaml_response = context
        .client
        .get(context.base_url.join("/openapi.yaml")?)
        .header("x-end-user", "bob")
        .header("cookie", "untrusted=bob")
        .send()
        .await?;
    assert_eq!(yaml_response.status(), reqwest::StatusCode::OK);
    let yaml: Value = serde_yaml::from_slice(&yaml_response.bytes().await?)?;
    assert_eq!(
        json, yaml,
        "{}: cached provider identity must be shared",
        fixed["id"]
    );

    context.user.deploy_environment(context.env_id).await?;
    let response = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let response = context
                .client
                .get(context.base_url.join("/openapi.json")?)
                .send()
                .await?;
            if response.status() == reqwest::StatusCode::BAD_GATEWAY {
                return anyhow::Ok(response);
            }
            anyhow::ensure!(
                response.status().is_success()
                    || response.status() == reqwest::StatusCode::GATEWAY_TIMEOUT,
                "unexpected OpenAPI status: {}",
                response.status()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    let mut statuses = vec![response.status().as_u16()];
    let error = response.text().await?;
    assert!(error.contains("provider-json"));
    assert!(!error.contains("invalid-provider-document"));
    for path in ["/raw/empty", "/raw/favicon"] {
        statuses.push(
            context
                .client
                .get(context.base_url.join(path)?)
                .send()
                .await?
                .status()
                .as_u16(),
        );
    }
    assert_eq!(
        json!(statuses),
        isolated["expect"]["statuses"],
        "{}",
        isolated["id"]
    );
    Ok(())
}
