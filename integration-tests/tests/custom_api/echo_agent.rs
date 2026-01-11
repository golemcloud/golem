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

use axum::http::{HeaderMap, HeaderValue};
use golem_client::api::RegistryServiceClient;
use golem_client::model::DeploymentCreation;
use golem_common::model::component::ComponentName;
use golem_common::model::deployment::{DeploymentRevision, DeploymentVersion};
use golem_common::model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::http_api_definition::{
    GatewayBinding, HttpApiDefinitionCreation, HttpApiDefinitionName, HttpApiDefinitionVersion,
    HttpApiRoute, RouteMethod, WorkerGatewayBinding,
};
use golem_common::model::http_api_deployment::HttpApiDeploymentCreation;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use pretty_assertions::assert_eq;
use reqwest::Url;
use serde_json::json;
use std::fmt::{Debug, Formatter};
use test_r::test_dep;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(EnvBasedTestDependencies);

#[allow(dead_code)]
pub struct EchoAgent {
    pub user: TestUserContext<EnvBasedTestDependencies>,
    pub env_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub client: reqwest::Client,
    pub base_url: Url,
}

impl Debug for EchoAgent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "EchoAgent")
    }
}

#[test_dep]
async fn echo_agent(deps: &EnvBasedTestDependencies) -> EchoAgent {
    echo_agent_internal(deps).await.unwrap()
}

async fn echo_agent_internal(deps: &EnvBasedTestDependencies) -> anyhow::Result<EchoAgent> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user.app_and_env().await?;

    let domain = Domain(format!("{}.golem.cloud", env.id));

    client
        .create_domain_registration(
            &env.id.0,
            &DomainRegistrationCreation {
                domain: domain.clone(),
            },
        )
        .await?;

    user.component(&env.id, "golem_it_constructor_parameter_echo")
        .name("golem-it:constructor-parameter-echo")
        .store()
        .await?;

    let http_api_definition_creation = HttpApiDefinitionCreation {
        name: HttpApiDefinitionName("echo-api".to_string()),
        version: HttpApiDefinitionVersion("1".to_string()),
        routes: vec![HttpApiRoute {
            method: RouteMethod::Post,
            path: "/echo/{param}".to_string(),
            binding: GatewayBinding::Worker(WorkerGatewayBinding {
                component_name: ComponentName("golem-it:constructor-parameter-echo".to_string()),
                idempotency_key: None,
                invocation_context: None,
                response: r#"
                    let param = request.path.param;
                    let agent = ephemeral-echo-agent("${param}");
                    let result = agent.change-and-get();
                    {
                        body: {
                            result: result
                        },
                        status: 200
                    }
                "#
                .to_string(),
            }),
            security: None,
        }],
    };

    client
        .create_http_api_definition_legacy(&env.id.0, &http_api_definition_creation)
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        api_definitions: vec![HttpApiDefinitionName("echo-api".to_string())],
    };

    client
        .create_http_api_deployment_legacy(&env.id.0, &http_api_deployment_creation)
        .await?;

    let plan = client.get_environment_deployment_plan(&env.id.0).await?;

    let deployment = client
        .deploy_environment(
            &env.id.0,
            &DeploymentCreation {
                current_revision: None,
                expected_deployment_hash: plan.deployment_hash,
                version: DeploymentVersion("0.0.1".to_string()),
            },
        )
        .await?;

    let client = {
        let mut headers = HeaderMap::new();
        headers.insert("Host", HeaderValue::from_str(&domain.0)?);
        reqwest::Client::builder()
            .default_headers(headers)
            .build()?
    };

    let base_url = Url::parse(&format!("http://127.0.0.1:{}", user.custom_request_port()))?;

    Ok(EchoAgent {
        client,
        base_url,
        user,
        env_id: env.id,
        deployment_revision: deployment.revision,
    })
}

#[test]
#[tracing::instrument]
async fn ephemeral_agent_http_call_resets_state(agent: &EchoAgent) -> anyhow::Result<()> {
    // First call with param "hello" - should return "hello!"
    let response1 = agent
        .client
        .post(agent.base_url.join("/echo/hello")?)
        .send()
        .await?;
    assert_eq!(response1.status(), reqwest::StatusCode::OK);
    let body1: serde_json::Value = response1.json().await?;
    assert_eq!(body1, json!({ "result": "hello!" }));

    // Second call with same param "hello" - should also return "hello!" (not "hello!!")
    // This verifies that the ephemeral agent is recreated fresh each time
    let response2 = agent
        .client
        .post(agent.base_url.join("/echo/hello")?)
        .send()
        .await?;
    assert_eq!(response2.status(), reqwest::StatusCode::OK);
    let body2: serde_json::Value = response2.json().await?;
    assert_eq!(body2, json!({ "result": "hello!" }));

    // Call with different param "world" - should return "world!"
    let response3 = agent
        .client
        .post(agent.base_url.join("/echo/world")?)
        .send()
        .await?;
    assert_eq!(response3.status(), reqwest::StatusCode::OK);
    let body3: serde_json::Value = response3.json().await?;
    assert_eq!(body3, json!({ "result": "world!" }));

    Ok(())
}

#[test]
#[tracing::instrument]
async fn url_gets_decoded_for_path_params(agent: &EchoAgent) -> anyhow::Result<()> {
    // Call with param "hello%20world" - should return "hello world!"
    let response1 = agent
        .client
        .post(agent.base_url.join("/echo/hello%20world")?)
        .send()
        .await?;
    assert_eq!(response1.status(), reqwest::StatusCode::OK);
    let body1: serde_json::Value = response1.json().await?;
    assert_eq!(body1, json!({ "result": "hello world!" }));

    Ok(())
}
