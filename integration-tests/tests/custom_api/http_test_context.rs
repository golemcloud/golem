use axum::http::{HeaderMap, HeaderValue};
use futures::future::BoxFuture;
use golem_client::api::{RegistryServiceClient, RegistryServiceClientLive};
use golem_common::base_model::agent::AgentTypeName;
use golem_common::base_model::deployment::DeploymentRevision;
use golem_common::base_model::domain_registration::{Domain, DomainRegistrationCreation};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::base_model::http_api_deployment::{
    HttpApiDeploymentAgentOptions, HttpApiDeploymentCreation,
};
use golem_common::model::application::Application;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::Environment;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{EnvironmentOptions, TestDsl, TestDslExtended};
use golem_test_framework::model::IFSEntry;
use reqwest::Url;
use std::collections::BTreeMap;
use std::fmt::{Debug, Formatter};
use std::future::Future;

#[allow(dead_code)]
pub struct HttpTestContext {
    pub user: TestUserContext<EnvBasedTestDependencies>,
    pub env_id: EnvironmentId,
    pub component_id: ComponentId,
    pub application_name: String,
    pub environment_name: String,
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

impl HttpTestContext {
    /// Starts a context that deploys the component built as `wasm_name` under `package_name`.
    pub fn builder(wasm_name: &str, package_name: &str) -> HttpTestContextBuilder {
        HttpTestContextBuilder {
            wasm_name: wasm_name.to_string(),
            package_name: package_name.to_string(),
            agents: Vec::new(),
            openapi_endpoint: HttpApiDeploymentCreation::default_openapi_endpoint_prefix(),
            files: Vec::new(),
            before_component: None,
        }
    }
}

type BeforeComponent = Box<
    dyn FnOnce(
            TestUserContext<EnvBasedTestDependencies>,
            EnvironmentId,
            Domain,
        ) -> BoxFuture<'static, anyhow::Result<()>>
        + Send,
>;

/// Builds an [`HttpTestContext`]: the user, environment and domain of a [`DomainEnvironment`],
/// the component, and an HTTP API deployment of the agents on the domain.
/// The options default to no agents, the default OpenAPI endpoint prefix and no files.
#[must_use]
pub struct HttpTestContextBuilder {
    wasm_name: String,
    package_name: String,
    agents: Vec<(AgentTypeName, HttpApiDeploymentAgentOptions)>,
    openapi_endpoint: String,
    files: Vec<(String, Vec<IFSEntry>)>,
    before_component: Option<BeforeComponent>,
}

// Three test binaries include this file, and each uses a subset of the options.
#[allow(dead_code)]
impl HttpTestContextBuilder {
    /// Deploys each agent type with the default HTTP options.
    pub fn agents<'a>(mut self, agent_types: impl IntoIterator<Item = &'a str>) -> Self {
        self.agents
            .extend(agent_types.into_iter().map(|agent_type| {
                (
                    AgentTypeName(agent_type.to_string()),
                    HttpApiDeploymentAgentOptions::default(),
                )
            }));
        self
    }

    /// Deploys `agent_type` with its own HTTP options.
    pub fn agent_with(mut self, agent_type: &str, options: HttpApiDeploymentAgentOptions) -> Self {
        self.agents
            .push((AgentTypeName(agent_type.to_string()), options));
        self
    }

    /// The prefix of the OpenAPI endpoint of the deployment.
    pub fn openapi_endpoint(mut self, prefix: impl Into<String>) -> Self {
        self.openapi_endpoint = prefix.into();
        self
    }

    /// Adds `entries` to the initial file system of `agent_type`.
    pub fn files(mut self, agent_type: &str, entries: Vec<IFSEntry>) -> Self {
        self.files.push((agent_type.to_string(), entries));
        self
    }

    /// Runs `setup` after the domain registration and before the component is stored.
    pub fn before_component<F, Fut>(mut self, setup: F) -> Self
    where
        F: FnOnce(TestUserContext<EnvBasedTestDependencies>, EnvironmentId, Domain) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.before_component = Some(Box::new(move |user, env_id, domain| {
            Box::pin(setup(user, env_id, domain))
        }));
        self
    }

    pub async fn build(self, deps: &EnvBasedTestDependencies) -> anyhow::Result<HttpTestContext> {
        let DomainEnvironment {
            user,
            application,
            environment,
            domain,
            client,
        } = DomainEnvironment::create(deps).await?;

        if let Some(setup) = self.before_component {
            setup(user.clone(), environment.id, domain.clone()).await?;
        }

        let mut component = user
            .component(&environment.id, &self.wasm_name)
            .name(&self.package_name);
        for (agent_type, files) in &self.files {
            component = component.with_files(agent_type, files);
        }
        let component = component.store().await?;

        let http_api_deployment_creation = HttpApiDeploymentCreation {
            scheme: Default::default(),
            domain: domain.clone(),
            agents: BTreeMap::from_iter(self.agents),
            webhooks_prefix: HttpApiDeploymentCreation::default_webhooks_prefix(),
            openapi_endpoint_prefix: self.openapi_endpoint,
        };

        client
            .create_http_api_deployment(&environment.id.0, &http_api_deployment_creation)
            .await?;

        let deployment = user.deploy_environment(environment.id).await?;
        let host_header = HeaderValue::from_str(&domain.0)?;

        let client = {
            let mut headers = HeaderMap::new();
            headers.insert("Host", host_header.clone());
            reqwest::Client::builder()
                .default_headers(headers)
                .redirect(reqwest::redirect::Policy::none())
                .build()?
        };

        let base_url = Url::parse(&format!("http://127.0.0.1:{}", user.custom_request_port()))?;

        Ok(HttpTestContext {
            client,
            base_url,
            user,
            env_id: environment.id,
            component_id: component.id,
            application_name: application.name.0,
            environment_name: environment.name.0,
            deployment_revision: deployment.revision,
            host_header,
        })
    }
}

/// A user without auto-deploy, an application and an environment with security overrides and
/// without version and compatibility checks, and the registered domain `{environment}.golem.cloud`.
pub struct DomainEnvironment {
    pub user: TestUserContext<EnvBasedTestDependencies>,
    pub application: Application,
    pub environment: Environment,
    pub domain: Domain,
    /// The registry client of the user.
    pub client: RegistryServiceClientLive,
}

impl DomainEnvironment {
    pub async fn create(deps: &EnvBasedTestDependencies) -> anyhow::Result<Self> {
        let user = deps.user().await?.with_auto_deploy(false);
        let client = deps.registry_service().client(&user.token).await;
        let (application, environment) = user
            .app_and_env_custom(&EnvironmentOptions {
                security_overrides: true,
                version_check: false,
                compatibility_check: false,
            })
            .await?;

        let domain = Domain(format!("{}.golem.cloud", environment.id));

        client
            .create_domain_registration(
                &environment.id.0,
                &DomainRegistrationCreation {
                    domain: domain.clone(),
                },
            )
            .await?;

        Ok(Self {
            user,
            application,
            environment,
            domain,
            client,
        })
    }
}

/// Writes the three `PerWorker` dependencies of type `$context`: the untagged one and the ones
/// tagged `postgres` and `sqlite`, each built by `$build(deps).await` from the
/// `EnvBasedTestDependencies` with the same tag. The calling module must import
/// `EnvBasedTestDependencies`, because `test_r` names a dependency after the path of its type.
#[allow(unused_macros)]
macro_rules! per_database_test_context {
    ($context:ident, $build:ident) => {
        #[test_r::test_dep(scope = PerWorker)]
        async fn test_context(deps: &EnvBasedTestDependencies) -> $context {
            $build(deps).await
        }

        #[test_r::test_dep(scope = PerWorker, tagged_as = "postgres")]
        async fn test_context_postgres(
            #[tagged_as("postgres")] deps: &EnvBasedTestDependencies,
        ) -> $context {
            $build(deps).await
        }

        #[test_r::test_dep(scope = PerWorker, tagged_as = "sqlite")]
        async fn test_context_sqlite(
            #[tagged_as("sqlite")] deps: &EnvBasedTestDependencies,
        ) -> $context {
            $build(deps).await
        }
    };
}
#[allow(unused_imports)]
pub(crate) use per_database_test_context;
