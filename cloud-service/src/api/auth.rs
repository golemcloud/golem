use std::{str::FromStr, sync::Arc};

use cloud_common::{auth::COOKIE_KEY, model::TokenSecret};
use golem_service_base::model::{ErrorBody, ErrorsBody};
use poem::{
    web::headers::{Authorization, HeaderMapExt},
    Request,
};

use crate::{
    auth::AccountAuthorisation,
    service::auth::{AuthService, AuthServiceError},
};

#[derive(Debug, Clone)]
pub struct AuthData {
    pub auth: AccountAuthorisation,
    pub secret: TokenSecret,
}

#[async_trait::async_trait]
impl<'a> poem::FromRequest<'a> for AuthData {
    async fn from_request(req: &'a Request, _: &mut poem::RequestBody) -> poem::Result<Self> {
        let secret = {
            req.headers()
                .typed_get::<Authorization<poem::web::headers::authorization::Bearer>>()
                .and_then(|auth| TokenSecret::from_str(auth.token()).ok())
        }
        .or_else(|| {
            req.cookie()
                .get(COOKIE_KEY)
                .and_then(|cookie| TokenSecret::from_str(cookie.value_str()).ok())
        })
        .ok_or(AuthError::MissingToken)?;

        let auth_service = req
            .data::<Arc<dyn AuthService + Send + Sync>>()
            .expect("AuthService to be present");

        let auth: AccountAuthorisation = auth_service
            .authorization(&secret)
            .await
            .map_err(Into::<AuthError>::into)?;

        Ok(AuthData { auth, secret })
    }
}

#[derive(Debug)]
enum AuthError {
    // Returns Generic Error response with no detail.
    MissingToken,
    // Invalid token from AuthService
    InvalidToken(String),
    // Unexpected error from AuthService
    Unexpected(String),
}

const AUTH_ERROR_MESSAGE: &str = "authorization error";

impl poem::error::ResponseError for AuthError {
    fn status(&self) -> http::StatusCode {
        match self {
            AuthError::MissingToken => http::StatusCode::UNAUTHORIZED,
            AuthError::InvalidToken(_) => http::StatusCode::UNAUTHORIZED,
            AuthError::Unexpected(_) => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl std::error::Error for AuthError {}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::MissingToken => f.write_str(AUTH_ERROR_MESSAGE),
            AuthError::InvalidToken(error) => {
                let error = ErrorBody {
                    error: error.into(),
                };
                let string = serde_json::to_string(&error).expect("Serialize ErrorBody");
                f.write_str(&string)
            }
            AuthError::Unexpected(error) => {
                let error = ErrorsBody {
                    errors: vec![error.into()],
                };
                let string = serde_json::to_string(&error).expect("Serialize ErrorsBody");
                f.write_str(&string)
            }
        }
    }
}

impl From<AuthServiceError> for AuthError {
    fn from(value: AuthServiceError) -> Self {
        match value {
            AuthServiceError::InvalidToken(error) => AuthError::InvalidToken(error),
            AuthServiceError::Unexpected(error) => AuthError::Unexpected(error),
        }
    }
}

#[cfg(test)]
mod test {
    use cloud_common::model::TokenSecret;
    use golem_common::model::AccountId;
    use http::StatusCode;
    use poem::{
        middleware::CookieJarManager,
        test::{TestClient, TestResponse},
        web::cookie::Cookie as PoemCookie,
        EndpointExt,
    };
    use poem_openapi::payload::PlainText;

    use super::*;

    #[poem::handler]
    async fn authorized_route(request: &Request, auth: AuthData) -> PlainText<String> {
        let prefix = {
            if request
                .headers()
                .typed_get::<Authorization<poem::web::headers::authorization::Bearer>>()
                .is_some()
            {
                "bearer"
            } else {
                "cookie"
            }
        };
        let value = auth.secret.value;

        PlainText(format!("{prefix}: {value}"))
    }

    struct DummyAuthService;

    #[async_trait::async_trait]
    impl AuthService for DummyAuthService {
        async fn authorization(
            &self,
            secret: &TokenSecret,
        ) -> Result<AccountAuthorisation, AuthServiceError> {
            if secret.value.to_string() == VALID_UUID {
                let account_id = AccountId::generate();
                let account_auth = AccountAuthorisation::new_test(&account_id, vec![]);

                Ok(account_auth)
            } else {
                Err(AuthServiceError::InvalidToken("invalid token".into()))
            }
        }
    }

    const VALID_UUID: &str = "0f1983af-993b-40ce-9f52-194c864d6aa3";

    fn make_extractor_api() -> poem::Route {
        let auth_service: Arc<dyn AuthService + Sync + Send> = Arc::new(DummyAuthService);

        poem::Route::new().at(
            "/test",
            authorized_route
                .data(auth_service)
                .with(CookieJarManager::new()),
        )
    }

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

    #[tokio::test]
    async fn bearer_valid_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_bearer_request(&client, VALID_UUID).await;

        response.assert_status_is_ok();
        response.assert_text(format!("bearer: {VALID_UUID}")).await;
    }

    #[tokio::test]
    async fn cookie_valid_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_cookie_request(&client, VALID_UUID).await;

        response.assert_status_is_ok();
        response.assert_text(format!("cookie: {VALID_UUID}")).await;
    }

    #[tokio::test]
    async fn no_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = client.get("/test").send().await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_text(AUTH_ERROR_MESSAGE).await;
    }

    #[tokio::test]
    async fn bearer_invalid_uuid_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_bearer_request(&client, "invalid").await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_text(AUTH_ERROR_MESSAGE).await;
    }

    #[tokio::test]
    async fn cookie_invalid_uuid_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_cookie_request(&client, "invalid").await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response.assert_text(AUTH_ERROR_MESSAGE).await;
    }

    // Test that AuthServiceError gets propagated properly.
    #[tokio::test]
    async fn invalid_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let unauthorized_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_bearer_request(&client, unauthorized_uuid.as_str()).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response
            .assert_json(ErrorBody {
                error: "invalid token".into(),
            })
            .await;
    }

    #[tokio::test]
    async fn conflict_bearer_valid_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_both_request(&client, VALID_UUID, "invalid").await;
        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_cookie_valid_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let response = make_both_request(&client, "invalid", VALID_UUID).await;
        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_both_uuid_invalid_cookie_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, VALID_UUID, other_uuid.as_str()).await;

        response.assert_status_is_ok();
    }

    #[tokio::test]
    async fn conflict_both_uuid_invalid_bearer_auth_extractor() {
        let api = make_extractor_api();
        let client = TestClient::new(api);

        let other_uuid = uuid::Uuid::new_v4().to_string();
        let response = make_both_request(&client, other_uuid.as_str(), VALID_UUID).await;

        response.assert_status(StatusCode::UNAUTHORIZED);
        response
            .assert_json(ErrorBody {
                error: "invalid token".into(),
            })
            .await;
    }
}
