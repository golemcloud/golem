use std::ops::Deref;
use std::sync::Arc;

use poem::endpoint::PrometheusExporter;
use poem::Route;
use poem_openapi::{OpenApiService, Tags};
use prometheus::Registry;

use crate::service::Services;

mod account;
mod account_summary;
mod grant;
mod healthcheck;
mod limits;
mod login;
mod project;
mod project_grant;
mod project_policy;
mod token;

#[derive(Tags)]
enum ApiTags {
    /// The account API allows users to query and manipulate their own account data.
    Account,
    AccountSummary,
    Grant,
    HealthCheck,
    /// The limits API allows users to query their current resource limits.
    Limits,
    /// The login endpoints are implementing an OAuth2 flow.
    Login,
    /// Projects are groups of components and their workers, providing both a separate namespace for these entities and allows sharing between accounts.
    ///
    /// Every account has a default project which is assumed when no specific project ID is passed in some component and worker related APIs.
    Project,
    /// Projects can have grants providing access to other accounts than the project's owner.
    ///
    /// The project grant API allows listing, creating and deleting such grants. What the grants allow exactly are defined by policies, covered by the Project policy API.
    ProjectGrant,
    /// Project policies describe a set of actions one account can perform when it was associated with a grant for a project.
    ///
    /// The following actions can be used in the projectActions fields of this API:
    /// - `ViewComponent` grants read access to a component
    /// - `CreateComponent` allows creating new components in a project
    /// - `UpdateComponent` allows uploading new versions for existing components in a project
    /// - `DeleteComponent` allows deleting components from a project
    /// - `ViewWorker` allows querying existing workers of a component belonging to the project
    /// - `CreateWorker` allows launching new workers of a component in the project
    /// - `UpdateWorker` allows manipulating existing workers of a component belonging to the project
    /// - `DeleteWorker` allows deleting workers of a component belonging to the project
    /// - `ViewProjectGrants` allows listing the existing grants of the project
    /// - `CreateProjectGrants` allows creating new grants for the project
    /// - `DeleteProjectGrants` allows deleting existing grants of the project
    ProjectPolicy,
    /// The token API allows creating custom access tokens for the Golem Cloud REST API to be used by tools and services.
    Token,
}

pub fn combined_routes(prometheus_registry: Arc<Registry>, services: &Services) -> Route {
    let api_service = make_open_api_service(services);

    let ui = api_service.swagger_ui();
    let spec = api_service.spec_endpoint_yaml();
    let metrics = PrometheusExporter::new(prometheus_registry.deref().clone());

    Route::new()
        .nest("/", api_service)
        .nest("/docs", ui)
        .nest("/specs", spec)
        .nest("/metrics", metrics)
}

type ApiServices = (
    account::AccountApi,
    account_summary::AccountSummaryApi,
    grant::GrantApi,
    limits::LimitsApi,
    login::LoginApi,
    healthcheck::HealthcheckApi,
    project::ProjectApi,
    project_grant::ProjectGrantApi,
    project_policy::ProjectPolicyApi,
    token::TokenApi,
);

pub fn make_open_api_service(services: &Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            account::AccountApi {
                auth_service: services.auth_service.clone(),
                account_service: services.account_service.clone(),
            },
            account_summary::AccountSummaryApi {
                auth_service: services.auth_service.clone(),
                account_summary_service: services.account_summary_service.clone(),
            },
            grant::GrantApi {
                auth_service: services.auth_service.clone(),
                account_grant_service: services.account_grant_service.clone(),
            },
            limits::LimitsApi {
                auth_service: services.auth_service.clone(),
                plan_limit_service: services.plan_limit_service.clone(),
            },
            login::LoginApi {
                auth_service: services.auth_service.clone(),
                login_service: services.login_service.clone(),
                oauth2_service: services.oauth2_service.clone(),
            },
            healthcheck::HealthcheckApi,
            project::ProjectApi {
                auth_service: services.auth_service.clone(),
                project_service: services.project_service.clone(),
                project_auth_service: services.project_auth_service.clone(),
            },
            project_grant::ProjectGrantApi {
                auth_service: services.auth_service.clone(),
                project_grant_service: services.project_grant_service.clone(),
                project_policy_service: services.project_policy_service.clone(),
            },
            project_policy::ProjectPolicyApi {
                auth_service: services.auth_service.clone(),
                project_policy_service: services.project_policy_service.clone(),
            },
            token::TokenApi {
                auth_service: services.auth_service.clone(),
                token_service: services.token_service.clone(),
            },
        ),
        "Golem API",
        "2.0",
    )
}
