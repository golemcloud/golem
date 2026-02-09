use axum::http::{HeaderMap, HeaderValue};
use axum::{body::Bytes, routing::post, Router};
use golem_client::api::RegistryServiceClient;
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::base_model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use reqwest::Client;
use reqwest::Url;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Mutex;

#[allow(dead_code)]
pub struct HttpTestContext {
    pub user: TestUserContext<EnvBasedTestDependencies>,
    pub env_id: EnvironmentId,
    pub deployment_revision: DeploymentRevision,
    pub client: reqwest::Client,
    pub base_url: Url,
    pub host_header: HeaderValue,
}

impl Debug for HttpTestContext {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "HttpTestContext")
    }
}

pub async fn test_context_internal(
    deps: &EnvBasedTestDependencies,
    component_name: &str,
    package_name: &str,
) -> anyhow::Result<HttpTestContext> {
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

    user.component(&env.id, component_name)
        .name(package_name)
        .store()
        .await?;

    let http_api_deployment_creation = HttpApiDeploymentCreation {
        domain: domain.clone(),
        agents: BTreeMap::from_iter([
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
        ]),
        webhooks_url: None,
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let deployment = user.deploy_environment(&env.id).await?;
    let host_header = HeaderValue::from_str(&domain.0)?;

    let client = {
        let mut headers = HeaderMap::new();
        headers.insert("Host", host_header.clone());
        reqwest::Client::builder()
            .default_headers(headers)
            .build()?
    };

    let base_url = Url::parse(&format!("http://127.0.0.1:{}", user.custom_request_port()))?;

    Ok(HttpTestContext {
        client,
        base_url,
        user,
        env_id: env.id,
        deployment_revision: deployment.revision,
        host_header,
    })
}

pub async fn run_webhook_callback_test(
    agent: &HttpTestContext,
    expected_body: serde_json::Value,
) -> anyhow::Result<()> {
    let host_header = agent.host_header.clone();
    let (agent_host, agent_port) = agent.base_url.authority().split_once(':').unwrap();
    let agent_host = agent_host.to_string();
    let agent_port = agent_port.parse::<u16>().unwrap();

    let received_webhook_request = Arc::new(Mutex::new(Vec::new()));
    let received_webhook_request_clone = received_webhook_request.clone();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let http_server = spawn(async move {
        let route = Router::new().route(
            "/",
            post(move |body: Bytes| {
                let received_webhook_request_clone = received_webhook_request_clone.clone();
                async move {
                    let body_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let webhook_url_str = body_json["webhookUrl"].as_str().unwrap();

                    let mut lock = received_webhook_request_clone.lock().await;
                    *lock = body.to_vec();

                    let mut url: Url = webhook_url_str.parse().unwrap();
                    url.set_host(Some(&agent_host)).unwrap();
                    url.set_port(Some(agent_port)).unwrap();

                    let client = Client::new();
                    let payload = vec![1u8, 2, 3, 4, 5];

                    client
                        .post(url)
                        .header("Host", host_header.clone())
                        .body(payload)
                        .send()
                        .await
                        .unwrap();

                    "ok"
                }
            }),
        );

        axum::serve(listener, route).await.unwrap();
    });

    let test_server_url = format!("http://127.0.0.1:{}/", port);

    agent
        .client
        .post(
            agent
                .base_url
                .join("/webhook-agents/test-agent/set-test-server-url")?,
        )
        .json(&serde_json::json!({ "test-server-url": test_server_url }))
        .send()
        .await?
        .error_for_status()?;

    let response = agent
        .client
        .post(
            agent
                .base_url
                .join("/webhook-agents/test-agent/test-webhook")?,
        )
        .send()
        .await?;

    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body, expected_body);

    http_server.abort();

    Ok(())
}
