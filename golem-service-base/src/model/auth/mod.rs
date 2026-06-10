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

#[cfg(test)]
mod tests;

use axum::http::header;
use golem_common::SafeDisplay;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_common::model::card::owner::{
    AgentOwnerPattern, ApplicationOwnerPattern, EnvironmentOwnerPattern,
};
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, CardAlgebraError, ClassPermissionTarget,
    ComponentResourcePattern, ComponentVerb, EffectiveSurface, EnvironmentResourcePattern,
    EnvironmentVerb, GrantSurface, PermissionTarget,
};
use golem_common::model::plan::PlanId;
use headers::Cookie as HCookie;
use headers::HeaderMapExt;
use poem::Request;
use poem_openapi::SecurityScheme;
use poem_openapi::auth::{ApiKey, Bearer};
use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::LazyLock;

pub const COOKIE_KEY: &str = "GOLEM_SESSION";
pub const AUTH_ERROR_MESSAGE: &str = "authorization error";
static EMPTY_ACCOUNT_ROLES: LazyLock<BTreeSet<AccountRole>> = LazyLock::new(BTreeSet::new);

#[derive(SecurityScheme)]
#[oai(rename = "Token", ty = "bearer", checker = "bearer_checker")]
pub struct GolemBearer(TokenSecret);

#[derive(SecurityScheme)]
#[oai(
    rename = "Cookie",
    ty = "api_key",
    key_in = "cookie",
    key_name = "GOLEM_SESSION",
    checker = "cookie_checker"
)]
pub struct GolemCookie(TokenSecret);

async fn bearer_checker(_: &Request, bearer: Bearer) -> Option<TokenSecret> {
    TokenSecret::from_str(&bearer.token).ok()
}

async fn cookie_checker(_: &Request, cookie: ApiKey) -> Option<TokenSecret> {
    TokenSecret::from_str(&cookie.key).ok()
}

#[derive(SecurityScheme)]
pub enum GolemSecurityScheme {
    Cookie(GolemCookie),
    Bearer(GolemBearer),
}

impl GolemSecurityScheme {
    pub fn secret(self) -> TokenSecret {
        Into::<TokenSecret>::into(self)
    }

    pub fn from_header_map(
        header_map: &header::HeaderMap,
    ) -> Result<GolemSecurityScheme, &'static str> {
        if let Some(auth_bearer) =
            header_map.typed_get::<headers::Authorization<headers::authorization::Bearer>>()
        {
            return TokenSecret::from_str(auth_bearer.token())
                .map(|token| GolemSecurityScheme::Bearer(GolemBearer(token)))
                .map_err(|_| "Invalid Bearer token");
        };

        if let Some(cookie_header) = header_map.typed_get::<HCookie>()
            && let Some(session_id) = cookie_header.get(COOKIE_KEY)
        {
            return TokenSecret::from_str(session_id)
                .map(|token| GolemSecurityScheme::Cookie(GolemCookie(token)))
                .map_err(|_| "Invalid session ID");
        }

        Err("Authentication failed")
    }
}

impl From<GolemSecurityScheme> for TokenSecret {
    fn from(scheme: GolemSecurityScheme) -> Self {
        match scheme {
            GolemSecurityScheme::Bearer(bearer) => bearer.0,
            GolemSecurityScheme::Cookie(cookie) => cookie.0,
        }
    }
}

impl AsRef<TokenSecret> for GolemSecurityScheme {
    fn as_ref(&self) -> &TokenSecret {
        match self {
            GolemSecurityScheme::Bearer(bearer) => &bearer.0,
            GolemSecurityScheme::Cookie(cookie) => &cookie.0,
        }
    }
}

// For use in non-openapi handlers
// Needs to be wrapped due to conflicting auto trait impls of inner type
pub struct WrappedGolemSecuritySchema(pub GolemSecurityScheme);

impl<'a> poem::FromRequest<'a> for WrappedGolemSecuritySchema {
    async fn from_request(req: &'a Request, body: &mut poem::RequestBody) -> poem::Result<Self> {
        use poem::web::cookie::CookieJar;
        use poem::web::headers::{Authorization, HeaderMapExt, authorization::Bearer as BearerWeb};

        fn extract_bearer_token(req: &Request) -> Option<GolemSecurityScheme> {
            req.headers()
                .typed_get::<Authorization<BearerWeb>>()
                .and_then(|Authorization(bearer)| TokenSecret::from_str(bearer.token()).ok())
                .map(|token| GolemSecurityScheme::Bearer(GolemBearer(token)))
        }

        fn extract_cookie_token(cookie_jar: &CookieJar) -> Option<GolemSecurityScheme> {
            cookie_jar
                .get(COOKIE_KEY)
                .and_then(|cookie| TokenSecret::from_str(cookie.value_str()).ok())
                .map(|token| GolemSecurityScheme::Cookie(GolemCookie(token)))
        }

        let cookie_jar = <&CookieJar>::from_request(req, body).await.map_err(|e| {
            tracing::info!("Failed to extract cookie jar: {e}");
            e
        })?;

        let result = extract_bearer_token(req)
            .or_else(|| extract_cookie_token(cookie_jar))
            .ok_or_else(|| {
                tracing::info!("No valid token or cookie present, returning error");
                poem::Error::from_string(AUTH_ERROR_MESSAGE, http::StatusCode::UNAUTHORIZED)
            })?;

        Ok(WrappedGolemSecuritySchema(result))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    #[error("The system-only action {0} is not allowed")]
    SystemOnlyActionNotAllowed(String),
    #[error("The permission target {0:?} is not allowed")]
    PermissionNotAllowed(Box<PermissionTarget>),
    #[error("Failed to evaluate permission target {target:?}: {error:?}")]
    PermissionEvaluationFailed {
        target: Box<PermissionTarget>,
        error: CardAlgebraError,
    },
}

impl SafeDisplay for AuthorizationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserAuthCtx {
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub account_plan_id: PlanId,
    pub account_roles: BTreeSet<AccountRole>,
    pub effective_surface: EffectiveSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// AuthCtx for systems or agents that do requests on behalf of users.
/// A bit more limited in what they can execute, but can be constructed
/// without any data lookups
pub struct AgentAuthCtx {
    pub account_id: AccountId,
    pub account_email: AccountEmail,
    pub effective_surface: EffectiveSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// AuthCtx for admin users impersonating another account via the /v1/admin/impersonate API.
/// Access checks (ownership, visibility) run as the target account, including its roles.
/// Audit writes (created_by) record the admin's account ID.
pub struct AdminImpersonationAuthCtx {
    pub admin_account_id: AccountId,
    pub target_account_id: AccountId,
    pub target_account_email: AccountEmail,
    pub target_account_roles: BTreeSet<AccountRole>,
    pub target_account_plan_id: PlanId,
    pub effective_surface: EffectiveSurface,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuthCtx {
    System,
    User(UserAuthCtx),
    Agent(AgentAuthCtx),
    AdminImpersonation(AdminImpersonationAuthCtx),
}

// Note: Basic visibility of resources is enforced in the repo. Use this to check permissions to modify resource / access restricted resources.
// To support defense-in-depth everything in here should be cheap -- avoid async / fetching data.
//
// The patterns for authorizing actions should be:
// - getting a resource: fetch resource + parent information (either through joins in the repo for non-environment or explicitly for environment case) (404 here) -> check auth using parent information (404 here)
// - listing a resource: fetch parent information explicitly (using rules in (1), so always 404 here) -> check auth using parent information (403 here)
// - creating a new resource: fetch parent information explicitly (using rules in (1), so always 404 here) -> check auth (403 here) -> create resource
// - update/delete a resource -> fetch resource + parent information (using rules in (1), so always 404 here)) + do auth using parent information (403 here)
impl AuthCtx {
    /// Get the sytem AuthCtx for system initiated action
    pub fn system() -> AuthCtx {
        AuthCtx::System
    }

    pub fn agent(account_id: AccountId, account_email: AccountEmail) -> AuthCtx {
        let effective_surface = temporary_agent_effective_surface(&account_email);
        Self::agent_with_effective_surface(account_id, account_email, effective_surface)
    }

    pub fn agent_with_effective_surface(
        account_id: AccountId,
        account_email: AccountEmail,
        effective_surface: EffectiveSurface,
    ) -> AuthCtx {
        AuthCtx::Agent(AgentAuthCtx {
            account_id,
            account_email,
            effective_surface,
        })
    }

    pub fn admin_impersonation(
        admin_account_id: AccountId,
        target_account_id: AccountId,
        target_account_email: AccountEmail,
        target_account_roles: BTreeSet<AccountRole>,
        target_account_plan_id: PlanId,
        effective_surface: EffectiveSurface,
    ) -> AuthCtx {
        AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
            admin_account_id,
            target_account_id,
            target_account_email,
            target_account_roles,
            target_account_plan_id,
            effective_surface,
        })
    }

    pub fn is_system(&self) -> bool {
        matches!(self, AuthCtx::System)
    }

    pub fn authorize_system_only(
        &self,
        action: impl Into<String>,
    ) -> Result<(), AuthorizationError> {
        if self.is_system() {
            Ok(())
        } else {
            Err(AuthorizationError::SystemOnlyActionNotAllowed(
                action.into(),
            ))
        }
    }

    /// The account ID recorded in audit fields (created_by).
    /// For admin impersonation this is the admin's account, not the target.
    pub fn actor_account_id(&self) -> AccountId {
        match self {
            Self::System => AccountId::SYSTEM,
            Self::User(user) => user.account_id,
            Self::Agent(user) => user.account_id,
            Self::AdminImpersonation(ctx) => ctx.admin_account_id,
        }
    }

    /// The account ID used for access and visibility checks.
    /// For admin impersonation this is the target account, giving the admin
    /// the same view as the impersonated user.
    pub fn access_account_id(&self) -> AccountId {
        match self {
            Self::System => AccountId::SYSTEM,
            Self::User(user) => user.account_id,
            Self::Agent(user) => user.account_id,
            Self::AdminImpersonation(ctx) => ctx.target_account_id,
        }
    }

    pub fn access_account_email(&self) -> Option<&AccountEmail> {
        match self {
            Self::User(user) => Some(&user.account_email),
            Self::Agent(agent) => Some(&agent.account_email),
            Self::AdminImpersonation(ctx) => Some(&ctx.target_account_email),
            Self::System => None,
        }
    }

    /// Returns the access account ID.
    ///
    /// Prefer calling `actor_account_id()` for audit writes or `access_account_id()` for
    /// visibility/ownership checks explicitly. This method exists for call sites that have
    /// not yet been migrated.
    pub fn account_id(&self) -> AccountId {
        self.access_account_id()
    }

    pub fn account_roles(&self) -> &BTreeSet<AccountRole> {
        match self {
            Self::System => &EMPTY_ACCOUNT_ROLES,
            Self::User(user) => &user.account_roles,
            Self::Agent(_) => &EMPTY_ACCOUNT_ROLES,
            Self::AdminImpersonation(ctx) => &ctx.target_account_roles,
        }
    }

    pub fn authorize_permission(
        &self,
        target: &PermissionTarget,
    ) -> Result<(), AuthorizationError> {
        match self {
            Self::System => Ok(()),
            Self::User(user) => {
                authorize_effective_surface_permission(&user.effective_surface, target)
            }
            Self::AdminImpersonation(ctx) => {
                authorize_effective_surface_permission(&ctx.effective_surface, target)
            }
            Self::Agent(agent) => {
                authorize_effective_surface_permission(&agent.effective_surface, target)
            }
        }
    }
}

fn authorize_effective_surface_permission(
    effective_surface: &EffectiveSurface,
    target: &PermissionTarget,
) -> Result<(), AuthorizationError> {
    if effective_surface.authorize(target).map_err(|error| {
        AuthorizationError::PermissionEvaluationFailed {
            target: Box::new(target.clone()),
            error,
        }
    })? {
        Ok(())
    } else {
        Err(AuthorizationError::PermissionNotAllowed(Box::new(
            target.clone(),
        )))
    }
}

fn temporary_agent_effective_surface(account: &AccountEmail) -> EffectiveSurface {
    EffectiveSurface {
        source_card_ids: Vec::new(),
        lower: vec![GrantSurface {
            positive: vec![
                PermissionTarget::Environment(ClassPermissionTarget {
                    owner: ApplicationOwnerPattern::AccountApplications {
                        account: account.clone(),
                    },
                    verb: Some(EnvironmentVerb::View),
                    resource: EnvironmentResourcePattern::Any,
                }),
                PermissionTarget::Component(ClassPermissionTarget {
                    owner: EnvironmentOwnerPattern::AccountEnvironments {
                        account: account.clone(),
                    },
                    verb: Some(ComponentVerb::View),
                    resource: ComponentResourcePattern::Any,
                }),
                PermissionTarget::Agent(ClassPermissionTarget {
                    owner: AgentOwnerPattern::AccountAgents {
                        account: account.clone(),
                    },
                    verb: Some(AgentVerb::View),
                    resource: AgentResourcePattern::Any,
                }),
                PermissionTarget::Agent(ClassPermissionTarget {
                    owner: AgentOwnerPattern::AccountAgents {
                        account: account.clone(),
                    },
                    verb: Some(AgentVerb::Invoke),
                    resource: AgentResourcePattern::Any,
                }),
                PermissionTarget::Agent(ClassPermissionTarget {
                    owner: AgentOwnerPattern::AccountAgents {
                        account: account.clone(),
                    },
                    verb: Some(AgentVerb::Resume),
                    resource: AgentResourcePattern::Any,
                }),
                PermissionTarget::Agent(ClassPermissionTarget {
                    owner: AgentOwnerPattern::AccountAgents {
                        account: account.clone(),
                    },
                    verb: Some(AgentVerb::UpdateRevision),
                    resource: AgentResourcePattern::Any,
                }),
            ],
            negative: Vec::new(),
        }],
        upper: Vec::new(),
    }
}

#[cfg(test)]
mod test {
    use super::AUTH_ERROR_MESSAGE;
    use super::{COOKIE_KEY, GolemSecurityScheme, WrappedGolemSecuritySchema};
    use http::StatusCode;
    use poem::{
        EndpointExt, Request,
        middleware::CookieJarManager,
        test::{TestClient, TestResponse},
        web::cookie::Cookie as PoemCookie,
    };
    use poem_openapi::{OpenApi, OpenApiService, payload::PlainText};
    use test_r::test;

    struct TestApi;

    #[OpenApi]
    impl TestApi {
        #[oai(path = "/test", method = "get")]
        async fn test(
            &self,
            _request: &Request,
            auth: GolemSecurityScheme,
        ) -> poem::Result<PlainText<String>> {
            Ok(handle_security_scheme(auth))
        }
    }

    #[poem::handler]
    fn handle(auth: WrappedGolemSecuritySchema) -> impl poem::IntoResponse {
        handle_security_scheme(auth.0)
    }

    fn handle_security_scheme(auth: GolemSecurityScheme) -> PlainText<String> {
        let prefix = match auth {
            GolemSecurityScheme::Bearer(_) => "bearer",
            GolemSecurityScheme::Cookie(_) => "cookie",
        };
        let value = auth.secret().into_secret();

        PlainText(format!("{prefix}: {value}"))
    }

    const VALID_UUID: &str = "0f1983af-993b-40ce-9f52-194c864d6aa3";

    async fn make_bearer_request<E: poem::Endpoint>(
        client: &TestClient<E>,
        auth: &str,
    ) -> TestResponse {
        client
            .get("/test")
            .typed_header(
                poem::web::headers::Authorization::bearer(auth).expect("Failed to create bearer"),
            )
            .send()
            .await
    }

    async fn make_cookie_request<E: poem::Endpoint>(
        client: &TestClient<E>,
        auth: &str,
    ) -> TestResponse {
        client
            .get("/test")
            .header(
                http::header::COOKIE,
                PoemCookie::new_with_str(COOKIE_KEY, auth).to_string(),
            )
            .send()
            .await
    }

    async fn make_both_request<E: poem::Endpoint>(
        client: &TestClient<E>,
        bearer: &str,
        cookie: &str,
    ) -> TestResponse {
        client
            .get("/test")
            .typed_header(
                poem::web::headers::Authorization::bearer(bearer).expect("Failed to create bearer"),
            )
            .header(
                http::header::COOKIE,
                PoemCookie::new_with_str(COOKIE_KEY, cookie).to_string(),
            )
            .send()
            .await
    }

    fn make_openapi() -> poem::Route {
        poem::Route::new().nest("/", OpenApiService::new(TestApi, "test", "1.0"))
    }

    fn make_non_openapi() -> poem::Route {
        let route = poem::Route::new()
            .at("/test", handle)
            .with(CookieJarManager::new());

        poem::Route::new().nest("/", route)
    }

    async fn bearer_valid_auth(api: poem::Route) {
        let client = TestClient::new(api);
        let response = make_bearer_request(&client, VALID_UUID).await;
        response.assert_status_is_ok();
        response.assert_text(format!("bearer: {VALID_UUID}")).await;
    }

    async fn cookie_valid_auth(api: poem::Route) {
        let client = TestClient::new(api);
        let response = make_cookie_request(&client, VALID_UUID).await;
        response.assert_status_is_ok();
        response.assert_text(format!("cookie: {VALID_UUID}")).await;
    }

    async fn no_auth(api: poem::Route) {
        let client = TestClient::new(api);
        let response = client.get("/test").send().await;
        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_text(AUTH_ERROR_MESSAGE).await;
    }

    async fn conflict_bearer_valid(api: poem::Route) {
        let client = TestClient::new(api);
        let response = make_both_request(&client, VALID_UUID, "invalid").await;
        response.assert_status_is_ok();
    }

    async fn conflict_cookie_valid(api: poem::Route) {
        let client = TestClient::new(api);
        let response = make_both_request(&client, "invalid", VALID_UUID).await;
        response.assert_status_is_ok();
    }

    async fn conflict_both_uuid_invalid_cookie_auth(api: poem::Route) {
        let client = TestClient::new(api);
        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, VALID_UUID, other_uuid.as_str()).await;
        response.assert_status_is_ok();
    }

    async fn conflict_both_uuid_invalid_bearer_auth(api: poem::Route) {
        let client = TestClient::new(api);
        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, other_uuid.as_str(), VALID_UUID).await;
        response.assert_status_is_ok();
    }

    // OpenAPI tests
    #[test]
    async fn bearer_valid_auth_openapi() {
        bearer_valid_auth(make_openapi()).await;
    }

    #[test]
    async fn cookie_valid_auth_openapi() {
        cookie_valid_auth(make_openapi()).await;
    }

    #[test]
    async fn no_auth_openapi() {
        no_auth(make_openapi()).await;
    }

    #[test]
    async fn conflict_bearer_valid_openapi() {
        conflict_bearer_valid(make_openapi()).await;
    }

    #[test]
    async fn conflict_cookie_valid_openapi() {
        conflict_cookie_valid(make_openapi()).await;
    }

    #[test]
    async fn conflict_both_uuid_invalid_cookie_auth_openapi() {
        conflict_both_uuid_invalid_cookie_auth(make_openapi()).await;
    }

    #[test]
    async fn conflict_both_uuid_invalid_bearer_auth_openapi() {
        conflict_both_uuid_invalid_bearer_auth(make_openapi()).await;
    }

    // Non-OpenAPI tests
    #[test]
    async fn bearer_valid_auth_non_openapi() {
        bearer_valid_auth(make_non_openapi()).await;
    }

    #[test]
    async fn cookie_valid_auth_non_openapi() {
        cookie_valid_auth(make_non_openapi()).await;
    }

    #[test]
    async fn no_auth_non_openapi() {
        no_auth(make_non_openapi()).await;
    }

    #[test]
    async fn conflict_bearer_valid_non_openapi() {
        conflict_bearer_valid(make_non_openapi()).await;
    }

    #[test]
    async fn conflict_cookie_valid_non_openapi() {
        conflict_cookie_valid(make_non_openapi()).await;
    }

    #[test]
    async fn conflict_both_uuid_invalid_cookie_auth_non_openapi() {
        conflict_both_uuid_invalid_cookie_auth(make_non_openapi()).await;
    }

    #[test]
    async fn conflict_both_uuid_invalid_bearer_auth_non_openapi() {
        conflict_both_uuid_invalid_bearer_auth(make_non_openapi()).await;
    }
}

mod protobuf {
    use super::{
        AdminImpersonationAuthCtx, AgentAuthCtx, AuthCtx, AuthorizationError, UserAuthCtx,
        temporary_agent_effective_surface,
    };
    use golem_common::model::account::AccountEmail;
    use golem_common::model::auth::AccountRole;
    use golem_common::model::card::{CardId, EffectiveSurface, GrantSurface, PermissionTarget};

    fn deserialize_effective_surface(
        value: golem_api_grpc::proto::golem::auth::AuthEffectiveSurface,
    ) -> Result<EffectiveSurface, String> {
        Ok(EffectiveSurface {
            source_card_ids: value
                .source_card_ids
                .into_iter()
                .map(|id| CardId(id.into()))
                .collect(),
            lower: deserialize_grant_surfaces(value.lower)?,
            upper: deserialize_grant_surfaces(value.upper)?,
        })
    }

    fn serialize_effective_surface(
        value: EffectiveSurface,
    ) -> golem_api_grpc::proto::golem::auth::AuthEffectiveSurface {
        golem_api_grpc::proto::golem::auth::AuthEffectiveSurface {
            source_card_ids: value
                .source_card_ids
                .into_iter()
                .map(|id| id.0.into())
                .collect(),
            lower: serialize_grant_surfaces(value.lower),
            upper: serialize_grant_surfaces(value.upper),
        }
    }

    fn serialize_grant_surfaces(
        surfaces: Vec<GrantSurface>,
    ) -> Vec<golem_api_grpc::proto::golem::auth::AuthGrantSurface> {
        surfaces
            .into_iter()
            .map(
                |surface| golem_api_grpc::proto::golem::auth::AuthGrantSurface {
                    positive: serialize_permission_targets(surface.positive),
                    negative: serialize_permission_targets(surface.negative),
                },
            )
            .collect()
    }

    fn deserialize_grant_surfaces(
        values: Vec<golem_api_grpc::proto::golem::auth::AuthGrantSurface>,
    ) -> Result<Vec<GrantSurface>, String> {
        values
            .into_iter()
            .map(|surface| {
                Ok(GrantSurface {
                    positive: deserialize_permission_targets(surface.positive)?,
                    negative: deserialize_permission_targets(surface.negative)?,
                })
            })
            .collect()
    }

    fn serialize_permission_targets(patterns: Vec<PermissionTarget>) -> Vec<Vec<u8>> {
        patterns
            .into_iter()
            .map(|pattern| {
                desert_rust::serialize_to_byte_vec(&pattern)
                    .expect("PermissionTarget Desert serialization failed")
            })
            .collect()
    }

    fn deserialize_permission_targets(
        values: Vec<Vec<u8>>,
    ) -> Result<Vec<PermissionTarget>, String> {
        values
            .into_iter()
            .map(|value| {
                desert_rust::deserialize(&value)
                    .map_err(|err| format!("invalid permission target: {err}"))
            })
            .collect()
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::UserAuthCtx> for UserAuthCtx {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::UserAuthCtx,
        ) -> Result<Self, Self::Error> {
            let account_roles = value
                .account_roles()
                .map(AccountRole::try_from)
                .collect::<Result<_, _>>()?;
            let effective_surface = value
                .effective_surface
                .ok_or_else(|| "missing effective_surface".to_string())
                .and_then(deserialize_effective_surface)?;

            Ok(Self {
                account_roles,
                account_id: value.account_id.ok_or("missing account id")?.try_into()?,
                account_email: AccountEmail::new(value.account_email),
                account_plan_id: value.plan_id.ok_or("missing plan id")?.try_into()?,
                effective_surface,
            })
        }
    }

    impl From<UserAuthCtx> for golem_api_grpc::proto::golem::auth::UserAuthCtx {
        fn from(value: UserAuthCtx) -> Self {
            Self {
                account_id: Some(value.account_id.into()),
                account_email: value.account_email.into_inner(),
                plan_id: Some(value.account_plan_id.into()),
                account_roles: value
                    .account_roles
                    .into_iter()
                    .map(|ar| golem_api_grpc::proto::golem::auth::AccountRole::from(ar).into())
                    .collect(),
                effective_surface: Some(serialize_effective_surface(value.effective_surface)),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::AgentAuthCtx> for AgentAuthCtx {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AgentAuthCtx,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                account_id: value.account_id.ok_or("missing account id")?.try_into()?,
                account_email: AccountEmail::new(value.account_email.clone()),
                effective_surface: value
                    .effective_surface
                    .map(deserialize_effective_surface)
                    .transpose()?
                    .unwrap_or_else(|| {
                        temporary_agent_effective_surface(&AccountEmail::new(value.account_email))
                    }),
            })
        }
    }

    impl From<AgentAuthCtx> for golem_api_grpc::proto::golem::auth::AgentAuthCtx {
        fn from(value: AgentAuthCtx) -> Self {
            Self {
                account_id: Some(value.account_id.into()),
                account_email: value.account_email.into_inner(),
                effective_surface: Some(serialize_effective_surface(value.effective_surface)),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::AdminImpersonationAuthCtx>
        for AdminImpersonationAuthCtx
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AdminImpersonationAuthCtx,
        ) -> Result<Self, Self::Error> {
            let target_account_roles = value
                .target_account_roles
                .into_iter()
                .map(|r| {
                    golem_api_grpc::proto::golem::auth::AccountRole::try_from(r)
                        .map_err(|_| "invalid account role".to_string())
                        .and_then(AccountRole::try_from)
                })
                .collect::<Result<_, _>>()?;
            let effective_surface = value
                .effective_surface
                .ok_or_else(|| "missing effective_surface".to_string())
                .and_then(deserialize_effective_surface)?;

            Ok(Self {
                admin_account_id: value
                    .admin_account_id
                    .ok_or("missing admin_account_id")?
                    .try_into()?,
                target_account_id: value
                    .target_account_id
                    .ok_or("missing target_account_id")?
                    .try_into()?,
                target_account_email: AccountEmail::new(value.target_account_email),
                target_account_roles,
                target_account_plan_id: value
                    .target_account_plan_id
                    .ok_or("missing target_account_plan_id")?
                    .try_into()?,
                effective_surface,
            })
        }
    }

    impl From<AdminImpersonationAuthCtx>
        for golem_api_grpc::proto::golem::auth::AdminImpersonationAuthCtx
    {
        fn from(value: AdminImpersonationAuthCtx) -> Self {
            Self {
                admin_account_id: Some(value.admin_account_id.into()),
                target_account_id: Some(value.target_account_id.into()),
                target_account_email: value.target_account_email.into_inner(),
                target_account_roles: value
                    .target_account_roles
                    .into_iter()
                    .map(|r| golem_api_grpc::proto::golem::auth::AccountRole::from(r).into())
                    .collect(),
                target_account_plan_id: Some(value.target_account_plan_id.into()),
                effective_surface: Some(serialize_effective_surface(value.effective_surface)),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::AuthCtx> for AuthCtx {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AuthCtx,
        ) -> Result<Self, Self::Error> {
            match value.value {
                Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::System(_)) => {
                    Ok(AuthCtx::System)
                }
                Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::User(user)) => {
                    Ok(AuthCtx::User(user.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::Agent(agent)) => {
                    Ok(AuthCtx::Agent(agent.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::AdminImpersonation(
                    ctx,
                )) => Ok(AuthCtx::AdminImpersonation(ctx.try_into()?)),
                None => Err("No auth_ctx value present".to_string()),
            }
        }
    }

    impl From<AuthCtx> for golem_api_grpc::proto::golem::auth::AuthCtx {
        fn from(value: AuthCtx) -> Self {
            match value {
                AuthCtx::System => Self {
                    value: Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::System(
                        golem_api_grpc::proto::golem::common::Empty {},
                    )),
                },
                AuthCtx::User(user) => Self {
                    value: Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::User(
                        user.into(),
                    )),
                },
                AuthCtx::Agent(impersonated_user) => Self {
                    value: Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::Agent(
                        impersonated_user.into(),
                    )),
                },
                AuthCtx::AdminImpersonation(ctx) => Self {
                    value: Some(
                        golem_api_grpc::proto::golem::auth::auth_ctx::Value::AdminImpersonation(
                            ctx.into(),
                        ),
                    ),
                },
            }
        }
    }

    impl From<AuthorizationError> for golem_api_grpc::proto::golem::worker::v1::AgentError {
        fn from(error: AuthorizationError) -> Self {
            Self {
                error: Some(error.into()),
            }
        }
    }

    impl From<AuthorizationError> for golem_api_grpc::proto::golem::worker::v1::agent_error::Error {
        fn from(error: AuthorizationError) -> Self {
            use golem_api_grpc::proto::golem::common::ErrorBody;

            Self::Unauthorized(ErrorBody {
                error: error.to_string(),
                code: golem_common::base_model::api::error_code::AUTH_UNAUTHORIZED.to_string(),
            })
        }
    }
}
