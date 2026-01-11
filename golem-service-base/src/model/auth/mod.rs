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

#[cfg(test)]
mod tests;

use axum::http::header;
use golem_common::SafeDisplay;
use golem_common::model::account::AccountId;
use golem_common::model::auth::{AccountRole, EnvironmentRole, TokenSecret};
use golem_common::model::plan::PlanId;
use headers::Cookie as HCookie;
use headers::HeaderMapExt;
use poem::Request;
use poem_openapi::SecurityScheme;
use poem_openapi::auth::{ApiKey, Bearer};
use std::collections::BTreeSet;
use std::hash::Hash;
use std::str::FromStr;
use std::sync::LazyLock;

pub const COOKIE_KEY: &str = "GOLEM_SESSION";
pub const AUTH_ERROR_MESSAGE: &str = "authorization error";
static SYSTEM_ACCOUNT_ROLES: LazyLock<BTreeSet<AccountRole>> =
    LazyLock::new(|| BTreeSet::from_iter([AccountRole::Admin]));
static IMPERSONATED_USER_ACCOUNT_ROLES: LazyLock<BTreeSet<AccountRole>> =
    LazyLock::new(BTreeSet::new);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum GlobalAction {
    CreateAccount,
    GetDefaultPlan,
    GetReports,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum PlanAction {
    ViewPlan,
    CreateOrUpdatePlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum AccountAction {
    CreateApplication,
    CreateEnvironment,
    CreateKnownSecret,
    CreateToken,
    DeleteAccount,
    DeleteApplication,
    DeletePlugin,
    DeleteToken,
    ListAllApplicationEnvironments,
    RegisterPlugin,
    SetRoles,
    SetPlan,
    UpdateAccount,
    UpdateApplication,
    UpdateUsage,
    ViewAccount,
    ViewApplications,
    ViewPlugin,
    ViewToken,
    ViewUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, strum_macros::Display)]
pub enum EnvironmentAction {
    CreateComponent,
    CreateDomainRegistration,
    CreateEnvironmentPluginGrant,
    CreateHttpApiDefinition,
    CreateHttpApiDeployment,
    CreateSecurityScheme,
    CreateShare,
    CreateWorker,
    DebugWorker,
    DeleteDomainRegistration,
    DeleteEnvironment,
    DeleteEnvironmentPluginGrant,
    DeleteHttpApiDefinition,
    DeleteHttpApiDeployment,
    DeleteSecurityScheme,
    DeleteShare,
    DeleteWorker,
    DeployEnvironment,
    UpdateComponent,
    UpdateEnvironment,
    UpdateHttpApiDefinition,
    UpdateHttpApiDeployment,
    UpdateSecurityScheme,
    UpdateShare,
    UpdateWorker,
    ViewAgentTypes,
    ViewComponent,
    ViewDeployment,
    ViewDeploymentPlan,
    ViewDomainRegistration,
    ViewEnvironment,
    ViewEnvironmentPluginGrant,
    ViewHttpApiDefinition,
    ViewHttpApiDeployment,
    ViewSecurityScheme,
    ViewShares,
    ViewWorker,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    #[error("The global action {0} is not allowed")]
    GlobalActionNotAllowed(GlobalAction),
    #[error("The plan action {0} is not allowed")]
    PlanActionNotAllowed(PlanAction),
    #[error("The account action {0} is not allowed")]
    AccountActionNotAllowed(AccountAction),
    #[error("The environment action {0} is not allowed")]
    EnvironmentActionNotAllowed(EnvironmentAction),
}

impl SafeDisplay for AuthorizationError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

/// All information required to answer environment level authorization questions
/// together with an AuthCtx
#[derive(Debug, Clone)]
pub struct AuthDetailsForEnvironment {
    pub account_id_owning_environment: AccountId,
    pub environment_roles_from_shares: BTreeSet<EnvironmentRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserAuthCtx {
    pub account_id: AccountId,
    pub account_plan_id: PlanId,
    pub account_roles: BTreeSet<AccountRole>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// AuthCtx for systems that do requests on behalf of users.
/// A bit more limited in what they can execute, but can be constructed
/// without any data lookups
pub struct ImpersonatedUserAuthCtx {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuthCtx {
    System,
    User(UserAuthCtx),
    ImpersonatedUser(ImpersonatedUserAuthCtx),
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

    pub fn impersonated_user(account_id: AccountId) -> AuthCtx {
        AuthCtx::ImpersonatedUser(ImpersonatedUserAuthCtx { account_id })
    }

    pub fn impersonated(&self) -> Self {
        match self {
            Self::User(inner) => Self::ImpersonatedUser(ImpersonatedUserAuthCtx {
                account_id: inner.account_id,
            }),
            Self::ImpersonatedUser(inner) => Self::ImpersonatedUser(ImpersonatedUserAuthCtx {
                account_id: inner.account_id,
            }),
            Self::System => Self::System,
        }
    }

    pub fn is_system(&self) -> bool {
        matches!(self, AuthCtx::System)
    }

    pub fn account_id(&self) -> AccountId {
        match self {
            Self::System => AccountId::SYSTEM,
            Self::User(user) => user.account_id,
            Self::ImpersonatedUser(user) => user.account_id,
        }
    }

    pub fn account_roles(&self) -> &BTreeSet<AccountRole> {
        match self {
            Self::System => &SYSTEM_ACCOUNT_ROLES,
            Self::User(user) => &user.account_roles,
            Self::ImpersonatedUser(_) => &IMPERSONATED_USER_ACCOUNT_ROLES,
        }
    }

    fn has_any_account_role(&self, allowed: &[AccountRole]) -> bool {
        let account_roles = self.account_roles();
        allowed.iter().any(|r| account_roles.contains(r))
    }

    fn account_plan_id(&self) -> Option<&PlanId> {
        match self {
            Self::System => None,
            Self::User(user) => Some(&user.account_plan_id),
            Self::ImpersonatedUser(_) => None,
        }
    }

    /// Whether storage level visibility rules (e.g. does this account have any shares for this environment / does this user own the environment)
    /// should be disabled for this user.
    pub fn should_override_storage_visibility_rules(&self) -> bool {
        self.has_any_account_role(&[AccountRole::Admin])
    }

    pub fn authorize_global_action(&self, action: GlobalAction) -> Result<(), AuthorizationError> {
        let is_allowed = match action {
            GlobalAction::CreateAccount => self.has_any_account_role(&[AccountRole::Admin]),
            GlobalAction::GetDefaultPlan => self.has_any_account_role(&[AccountRole::Admin]),
            GlobalAction::GetReports => {
                self.has_any_account_role(&[AccountRole::Admin, AccountRole::MarketingAdmin])
            }
        };

        if !is_allowed {
            Err(AuthorizationError::GlobalActionNotAllowed(action))?
        }

        Ok(())
    }

    pub fn authorize_plan_action(
        &self,
        plan_id: &PlanId,
        action: PlanAction,
    ) -> Result<(), AuthorizationError> {
        match action {
            PlanAction::ViewPlan => {
                // Users are allowed to see their own plan
                if let Some(account_plan_id) = self.account_plan_id()
                    && account_plan_id == plan_id
                {
                    return Ok(());
                }

                // admins are allowed to see all plans
                if self.has_any_account_role(&[AccountRole::Admin]) {
                    return Ok(());
                }

                Err(AuthorizationError::PlanActionNotAllowed(action))
            }
            PlanAction::CreateOrUpdatePlan => {
                // Only admins can change plan details
                if self.has_any_account_role(&[AccountRole::Admin]) {
                    Ok(())
                } else {
                    Err(AuthorizationError::PlanActionNotAllowed(action))
                }
            }
        }
    }

    pub fn authorize_account_action(
        &self,
        target_account_id: AccountId,
        action: AccountAction,
    ) -> Result<(), AuthorizationError> {
        let is_allowed = match action {
            AccountAction::SetPlan | AccountAction::SetRoles => {
                self.has_any_account_role(&[AccountRole::Admin])
            }
            _ => {
                (self.account_id() == target_account_id)
                    || self.has_any_account_role(&[AccountRole::Admin])
            }
        };

        if !is_allowed {
            Err(AuthorizationError::AccountActionNotAllowed(action))?
        }

        Ok(())
    }

    pub fn authorize_environment_action(
        &self,
        account_owning_enviroment: AccountId,
        roles_from_shares: &BTreeSet<EnvironmentRole>,
        action: EnvironmentAction,
    ) -> Result<(), AuthorizationError> {
        // Environment owners, admins and system users are allowed to do everything with their environments
        if self.account_id() == account_owning_enviroment
            || self.has_any_account_role(&[AccountRole::Admin])
        {
            return Ok(());
        }

        let is_allowed = match action {
            // environments
            EnvironmentAction::ViewEnvironment => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::UpdateEnvironment => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::DeleteEnvironment => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            // Components
            EnvironmentAction::CreateComponent => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::UpdateComponent => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewComponent => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // Environment shares
            EnvironmentAction::ViewShares => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::UpdateShare => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::CreateShare => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::DeleteShare => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            // Deployments
            EnvironmentAction::DeployEnvironment => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewDeployment => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::ViewDeploymentPlan => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // Environment plugin grants
            EnvironmentAction::CreateEnvironmentPluginGrant => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::ViewEnvironmentPluginGrant => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::DeleteEnvironmentPluginGrant => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            // Domain registrations
            EnvironmentAction::CreateDomainRegistration => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::DeleteDomainRegistration => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::ViewDomainRegistration => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // Workers
            EnvironmentAction::CreateWorker => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::DebugWorker => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::DeleteWorker => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewWorker => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            EnvironmentAction::UpdateWorker => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            // Security Schemes
            EnvironmentAction::CreateSecurityScheme => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::UpdateSecurityScheme => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::DeleteSecurityScheme => {
                has_any_role(roles_from_shares, &[EnvironmentRole::Admin])
            }
            EnvironmentAction::ViewSecurityScheme => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // Http api definitions
            EnvironmentAction::CreateHttpApiDefinition => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::UpdateHttpApiDefinition => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::DeleteHttpApiDefinition => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewHttpApiDefinition => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // Http api deployment
            EnvironmentAction::CreateHttpApiDeployment => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::UpdateHttpApiDeployment => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::DeleteHttpApiDeployment => has_any_role(
                roles_from_shares,
                &[EnvironmentRole::Admin, EnvironmentRole::Deployer],
            ),
            EnvironmentAction::ViewHttpApiDeployment => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
            // agent types
            EnvironmentAction::ViewAgentTypes => has_any_role(
                roles_from_shares,
                &[
                    EnvironmentRole::Admin,
                    EnvironmentRole::Deployer,
                    EnvironmentRole::Viewer,
                ],
            ),
        };

        if !is_allowed {
            Err(AuthorizationError::EnvironmentActionNotAllowed(action))?
        };

        Ok(())
    }
}

fn has_any_role<T: Eq + Hash + Ord>(roles: &BTreeSet<T>, allowed: &[T]) -> bool {
    allowed.iter().any(|r| roles.contains(r))
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
    use super::AuthDetailsForEnvironment;
    use super::{AuthCtx, AuthorizationError, ImpersonatedUserAuthCtx, UserAuthCtx};
    use golem_common::model::auth::{AccountRole, EnvironmentRole};

    impl TryFrom<golem_api_grpc::proto::golem::auth::AuthDetailsForEnvironment>
        for AuthDetailsForEnvironment
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::AuthDetailsForEnvironment,
        ) -> Result<Self, Self::Error> {
            let environment_roles_from_shares = value
                .environment_roles_from_shares()
                .map(EnvironmentRole::try_from)
                .collect::<Result<_, _>>()?;

            let account_id_owning_environment = value
                .account_id_owning_environment
                .ok_or("missing account_id")?
                .try_into()?;

            Ok(Self {
                account_id_owning_environment,
                environment_roles_from_shares,
            })
        }
    }

    impl From<AuthDetailsForEnvironment>
        for golem_api_grpc::proto::golem::auth::AuthDetailsForEnvironment
    {
        fn from(value: AuthDetailsForEnvironment) -> Self {
            Self {
                account_id_owning_environment: Some(value.account_id_owning_environment.into()),
                environment_roles_from_shares: value
                    .environment_roles_from_shares
                    .into_iter()
                    .map(|er| golem_api_grpc::proto::golem::auth::EnvironmentRole::from(er) as i32)
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::UserAuthCtx> for UserAuthCtx {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::UserAuthCtx,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                account_roles: value
                    .account_roles()
                    .map(AccountRole::try_from)
                    .collect::<Result<_, _>>()?,
                account_id: value.account_id.ok_or("missing account id")?.try_into()?,
                account_plan_id: value.plan_id.ok_or("missing plan id")?.try_into()?,
            })
        }
    }

    impl From<UserAuthCtx> for golem_api_grpc::proto::golem::auth::UserAuthCtx {
        fn from(value: UserAuthCtx) -> Self {
            Self {
                account_id: Some(value.account_id.into()),
                plan_id: Some(value.account_plan_id.into()),
                account_roles: value
                    .account_roles
                    .into_iter()
                    .map(|ar| golem_api_grpc::proto::golem::auth::AccountRole::from(ar) as i32)
                    .collect(),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::auth::ImpersonatedUserAuthCtx>
        for ImpersonatedUserAuthCtx
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::auth::ImpersonatedUserAuthCtx,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                account_id: value.account_id.ok_or("missing account id")?.try_into()?,
            })
        }
    }

    impl From<ImpersonatedUserAuthCtx> for golem_api_grpc::proto::golem::auth::ImpersonatedUserAuthCtx {
        fn from(value: ImpersonatedUserAuthCtx) -> Self {
            Self {
                account_id: Some(value.account_id.into()),
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
                Some(golem_api_grpc::proto::golem::auth::auth_ctx::Value::ImpersonatedUser(
                    impersonated_user,
                )) => Ok(AuthCtx::ImpersonatedUser(impersonated_user.try_into()?)),
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
                AuthCtx::ImpersonatedUser(impersonated_user) => Self {
                    value: Some(
                        golem_api_grpc::proto::golem::auth::auth_ctx::Value::ImpersonatedUser(
                            impersonated_user.into(),
                        ),
                    ),
                },
            }
        }
    }

    impl From<AuthorizationError> for golem_api_grpc::proto::golem::worker::v1::WorkerError {
        fn from(error: AuthorizationError) -> Self {
            Self {
                error: Some(error.into()),
            }
        }
    }

    impl From<AuthorizationError> for golem_api_grpc::proto::golem::worker::v1::worker_error::Error {
        fn from(error: AuthorizationError) -> Self {
            use golem_api_grpc::proto::golem::common::ErrorBody;

            Self::Unauthorized(ErrorBody {
                error: error.to_string(),
            })
        }
    }
}
