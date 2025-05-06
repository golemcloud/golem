use crate::service::account::AccountError;
use crate::service::account_grant::AccountGrantServiceError;
use crate::service::auth::AuthServiceError;
use crate::service::project::ProjectError;
use crate::service::project_grant::ProjectGrantError;
use crate::service::project_policy::ProjectPolicyError;
use crate::service::token::TokenServiceError;
use crate::service::Services;
use cloud_common::clients::plugin::PluginError;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::error::{ErrorBody, ErrorsBody};
use golem_common::SafeDisplay;
use poem::endpoint::PrometheusExporter;
use poem::Route;
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApiService, Tags};
use prometheus::Registry;
use std::ops::Deref;
use std::sync::Arc;

mod account;
mod account_summary;
mod dto;
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

#[derive(ApiResponse, Debug, Clone)]
pub enum ApiError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized request
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Account not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Internal server error
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for ApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            ApiError::BadRequest(_) => "BadRequest",
            ApiError::NotFound(_) => "NotFound",
            ApiError::Unauthorized(_) => "Unauthorized",
            ApiError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            ApiError::BadRequest(_) => true,
            ApiError::NotFound(_) => true,
            ApiError::Unauthorized(_) => true,
            ApiError::InternalError(_) => false,
        }
    }
}

type ApiResult<T> = Result<T, ApiError>;

impl From<AuthServiceError> for ApiError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountAccessForbidden { .. }
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. } => {
                ApiError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

impl From<AccountError> for ApiError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::Internal(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            AccountError::AccountNotFound(_) => ApiError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalPlanError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::AuthError(inner) => inner.into(),
        }
    }
}

impl From<TokenServiceError> for ApiError {
    fn from(value: TokenServiceError) -> Self {
        match value {
            TokenServiceError::Unauthorized(_) => ApiError::Unauthorized(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            TokenServiceError::InternalTokenError(_)
            | TokenServiceError::InternalRepoError(_)
            | TokenServiceError::InternalSerializationError { .. }
            | TokenServiceError::InternalSecretAlreadyExists { .. } => {
                ApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            TokenServiceError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            TokenServiceError::UnknownToken(_) => ApiError::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            TokenServiceError::AccountNotFound(_) => ApiError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            TokenServiceError::UnknownTokenState(_) => ApiError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
        }
    }
}

impl From<AccountGrantServiceError> for ApiError {
    fn from(value: AccountGrantServiceError) -> Self {
        match value {
            AccountGrantServiceError::InternalRepoError(_) => {
                ApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AccountGrantServiceError::ArgValidation(errors) => {
                ApiError::BadRequest(Json(ErrorsBody { errors }))
            }
            AccountGrantServiceError::AccountNotFound(_) => {
                ApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            AccountGrantServiceError::InternalAuthError(inner) => inner.into(),
        }
    }
}

impl From<ProjectPolicyError> for ApiError {
    fn from(value: ProjectPolicyError) -> Self {
        match value {
            ProjectPolicyError::InternalRepoError(_) => ApiError::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
        }
    }
}

#[derive(ApiResponse, Debug, Clone)]
pub enum LimitedApiError {
    /// Invalid request, returning with a list of issues detected in the request
    #[oai(status = 400)]
    BadRequest(Json<ErrorsBody>),
    /// Unauthorized
    #[oai(status = 401)]
    Unauthorized(Json<ErrorBody>),
    /// Maximum number of projects exceeded
    #[oai(status = 403)]
    LimitExceeded(Json<ErrorBody>),
    /// Project not found
    #[oai(status = 404)]
    NotFound(Json<ErrorBody>),
    /// Project already exists
    #[oai(status = 500)]
    InternalError(Json<ErrorBody>),
}

impl TraceErrorKind for LimitedApiError {
    fn trace_error_kind(&self) -> &'static str {
        match &self {
            LimitedApiError::BadRequest(_) => "BadRequest",
            LimitedApiError::NotFound(_) => "NotFound",
            LimitedApiError::LimitExceeded(_) => "LimitExceeded",
            LimitedApiError::Unauthorized(_) => "Unauthorized",
            LimitedApiError::InternalError(_) => "InternalError",
        }
    }

    fn is_expected(&self) -> bool {
        match &self {
            LimitedApiError::BadRequest(_) => true,
            LimitedApiError::NotFound(_) => true,
            LimitedApiError::LimitExceeded(_) => true,
            LimitedApiError::Unauthorized(_) => true,
            LimitedApiError::InternalError(_) => false,
        }
    }
}

type LimitedApiResult<T> = Result<T, LimitedApiError>;

impl From<AuthServiceError> for LimitedApiError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(_)
            | AuthServiceError::ProjectAccessForbidden { .. }
            | AuthServiceError::ProjectActionForbidden { .. }
            | AuthServiceError::RoleMissing { .. }
            | AuthServiceError::AccountOwnershipRequired
            | AuthServiceError::AccountAccessForbidden { .. } => {
                LimitedApiError::Unauthorized(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            AuthServiceError::InternalTokenServiceError(_)
            | AuthServiceError::InternalRepoError(_) => {
                LimitedApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

impl From<ProjectError> for LimitedApiError {
    fn from(value: ProjectError) -> Self {
        match value {
            ProjectError::InternalRepoError(_)
            | ProjectError::FailedToCreateDefaultProject(_)
            | ProjectError::InternalConversionError { .. }
            | ProjectError::InternalPlanLimitError(_) => {
                LimitedApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            ProjectError::LimitExceeded(_) => LimitedApiError::LimitExceeded(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            ProjectError::PluginNotFound { .. } => LimitedApiError::BadRequest(Json(ErrorsBody {
                errors: vec![value.to_safe_string()],
            })),
            ProjectError::InternalPluginError(_) => {
                LimitedApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            ProjectError::CannotDeleteDefaultProject => {
                LimitedApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            ProjectError::InternalProjectAuthorisationError(inner) => inner.into(),
        }
    }
}

impl From<ProjectGrantError> for LimitedApiError {
    fn from(value: ProjectGrantError) -> Self {
        match value {
            ProjectGrantError::InternalRepoError(_) => {
                LimitedApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
            ProjectGrantError::AuthError(inner) => inner.into(),
            ProjectGrantError::ProjectNotFound(_) => {
                LimitedApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            ProjectGrantError::ProjectPolicyNotFound(_) => {
                LimitedApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
            ProjectGrantError::AccountNotFound(_) => {
                LimitedApiError::BadRequest(Json(ErrorsBody {
                    errors: vec![value.to_safe_string()],
                }))
            }
        }
    }
}

impl From<ProjectPolicyError> for LimitedApiError {
    fn from(value: ProjectPolicyError) -> Self {
        match value {
            ProjectPolicyError::InternalRepoError(_) => {
                LimitedApiError::InternalError(Json(ErrorBody {
                    error: value.to_safe_string(),
                }))
            }
        }
    }
}

impl From<PluginError> for LimitedApiError {
    fn from(value: PluginError) -> Self {
        LimitedApiError::InternalError(Json(ErrorBody {
            error: value.to_safe_string(),
        }))
    }
}

impl From<AccountError> for LimitedApiError {
    fn from(value: AccountError) -> Self {
        match value {
            AccountError::Internal(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::ArgValidation(errors) => Self::BadRequest(Json(ErrorsBody { errors })),
            AccountError::AccountNotFound(_) => Self::NotFound(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalRepoError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::InternalPlanError(_) => Self::InternalError(Json(ErrorBody {
                error: value.to_safe_string(),
            })),
            AccountError::AuthError(inner) => inner.into(),
        }
    }
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
                api_mapper: services.api_mapper.clone(),
            },
            project_grant::ProjectGrantApi {
                auth_service: services.auth_service.clone(),
                project_grant_service: services.project_grant_service.clone(),
                project_policy_service: services.project_policy_service.clone(),
                account_service: services.account_service.clone(),
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
        "1.0",
    )
}
