use poem::Request;
use poem_openapi::{
    auth::{ApiKey, Bearer},
    SecurityScheme,
};

use crate::model::TokenSecret;
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
        tracing::info!("Extracting security scheme");
        let result = <GolemSecurityScheme as poem_openapi::ApiExtractor<'a>>::from_request(
            req,
            body,
            Default::default(),
        )
        .await;

        match result {
            Ok(scheme) => Ok(WrappedGolemSecuritySchema(scheme)),
            Err(e) => {
                tracing::info!("Failed to extract security scheme: {e}");
                Err(e)
            }
        }
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

#[cfg(test)]
mod test {
    use http::StatusCode;
    use poem::{
        test::{TestClient, TestResponse},
        web::cookie::Cookie as PoemCookie,
        Request,
    };
    use poem_openapi::{payload::PlainText, OpenApi, OpenApiService};

    use crate::auth::AUTH_ERROR_MESSAGE;

    use super::{GolemSecurityScheme, COOKIE_KEY};

    struct TestApi;

    #[OpenApi]
    impl TestApi {
        #[oai(path = "/test", method = "get")]
        async fn test(
            &self,
            _request: &Request,
            auth: GolemSecurityScheme,
        ) -> poem::Result<PlainText<String>> {
            let prefix = match auth {
                GolemSecurityScheme::Bearer(_) => "bearer",
                GolemSecurityScheme::Cookie(_) => "cookie",
            };
            let value = auth.secret().value;

            Ok(PlainText(format!("{prefix}: {value}")))
        }
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

    #[tokio::test]
    async fn bearer_valid_auth_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let response = make_bearer_request(&client, VALID_UUID).await;

        response.assert_status_is_ok();
        response.assert_text(format!("bearer: {VALID_UUID}")).await;
    }

    #[tokio::test]
    async fn cookie_valid_auth_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let response = make_cookie_request(&client, VALID_UUID).await;

        response.assert_status_is_ok();
        response.assert_text(format!("cookie: {VALID_UUID}")).await;
    }

    #[tokio::test]
    async fn no_auth_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let response = client.get("/test").send().await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_text(AUTH_ERROR_MESSAGE).await;
    }

    #[tokio::test]
    async fn conflict_bearer_valid_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let response = make_both_request(&client, VALID_UUID, "invalid").await;

        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_cookie_valid_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let response = make_both_request(&client, "invalid", VALID_UUID).await;

        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_both_uuid_invalid_cookie_auth_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, VALID_UUID, other_uuid.as_str()).await;

        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_both_uuid_invalid_bearer_auth_openapi() {
        let api = make_openapi();
        let client = TestClient::new(api);

        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, other_uuid.as_str(), VALID_UUID).await;

        response.assert_status_is_ok();
    }
}
