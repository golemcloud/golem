use axum::http::{HeaderMap, HeaderValue};
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
use golem_test_framework::dsl::{EnvironmentOptions, TestDsl, TestDslExtended};
use reqwest::Url;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};

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

pub async fn make_test_context(
    deps: &EnvBasedTestDependencies,
    agent_and_http_options: Vec<(AgentTypeName, HttpApiDeploymentAgentOptions)>,
    component_name: &str,
    package_name: &str,
) -> anyhow::Result<HttpTestContext> {
    let user = deps.user().await?.with_auto_deploy(false);
    let client = deps.registry_service().client(&user.token).await;
    let (_, env) = user
        .app_and_env_custom(&EnvironmentOptions {
            security_overrides: true,
            version_check: false,
            compatibility_check: false,
        })
        .await?;

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
        agents: BTreeMap::from_iter(agent_and_http_options),
        webhooks_url: HttpApiDeploymentCreation::default_webhooks_url(),
    };

    client
        .create_http_api_deployment(&env.id.0, &http_api_deployment_creation)
        .await?;

    let deployment = user.deploy_environment(env.id).await?;
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
