use crate::model::TokenSecret;
use axum::http::header;
use bincode::{Decode, Encode};
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::auth::{GolemAuthCtx, GolemNamespace};
use headers::authorization::Bearer as HBearer;
use headers::Cookie as HCookie;
use poem::web::headers::{Authorization, HeaderMapExt};
use poem::Request;
use poem_openapi::{
    auth::{ApiKey, Bearer},
    SecurityScheme,
};
use serde::Deserialize;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

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
        if let Some(auth_bearer) = header_map.typed_get::<Authorization<HBearer>>() {
            return TokenSecret::from_str(auth_bearer.token())
                .map(|token| GolemSecurityScheme::Bearer(GolemBearer(token)))
                .map_err(|_| "Invalid Bearer token");
        };

        if let Some(cookie_header) = header_map.typed_get::<HCookie>() {
            if let Some(session_id) = cookie_header.get(COOKIE_KEY) {
                return TokenSecret::from_str(session_id)
                    .map(|token| GolemSecurityScheme::Cookie(GolemCookie(token)))
                    .map_err(|_| "Invalid session ID");
            }
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
        use poem::web::headers::{authorization::Bearer as BearerWeb, Authorization, HeaderMapExt};

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

pub const COOKIE_KEY: &str = "GOLEM_SESSION";
pub const AUTH_ERROR_MESSAGE: &str = "authorization error";

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct CloudAuthCtx {
    pub token_secret: TokenSecret,
}

impl CloudAuthCtx {
    pub fn new(token_secret: TokenSecret) -> Self {
        Self { token_secret }
    }
}

impl IntoIterator for CloudAuthCtx {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        vec![(
            "authorization".to_string(),
            format!("Bearer {}", self.token_secret.value),
        )]
        .into_iter()
    }
}

impl GolemAuthCtx for CloudAuthCtx {}

#[derive(Clone, Debug, Hash, Eq, PartialEq, Encode, Decode, Deserialize)]
pub struct CloudNamespace {
    pub project_id: ProjectId,
    // project owner account
    pub account_id: AccountId,
}

impl CloudNamespace {
    pub fn new(project_id: ProjectId, account_id: AccountId) -> Self {
        Self {
            project_id,
            account_id,
        }
    }
}

impl Display for CloudNamespace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.account_id, self.project_id)
    }
}

impl TryFrom<String> for CloudNamespace {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid namespace: {s}"));
        }

        Ok(Self {
            project_id: ProjectId::try_from(parts[1])?,
            account_id: AccountId::from(parts[0]),
        })
    }
}

impl GolemNamespace for CloudNamespace {
    fn account_id(&self) -> AccountId {
        self.account_id.clone()
    }
    fn project_id(&self) -> Option<ProjectId> {
        Some(self.project_id.clone())
    }
}

#[cfg(test)]
mod test {
    use test_r::test;

    use http::StatusCode;
    use poem::{
        middleware::CookieJarManager,
        test::{TestClient, TestResponse},
        web::cookie::Cookie as PoemCookie,
        EndpointExt, Request,
    };
    use poem_openapi::{payload::PlainText, OpenApi, OpenApiService};

    use crate::auth::AUTH_ERROR_MESSAGE;

    use super::{GolemSecurityScheme, WrappedGolemSecuritySchema, COOKIE_KEY};

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
        let value = auth.secret().value;

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
