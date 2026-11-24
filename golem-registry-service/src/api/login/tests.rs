use crate::api::make_open_api_service;
use crate::bootstrap::Services;
use crate::bootstrap::login::{LoginSystem, LoginSystemEnabled};
use crate::config::{ComponentCompilationConfig, LoginConfig, OAuth2Config, RegistryServiceConfig};
use crate::model::login::ExternalLogin;
use crate::repo::oauth2_token::DbOAuth2TokenRepo;
use crate::repo::oauth2_webflow_state::DbOAuth2WebflowStateRepo;
use crate::services::oauth2::OAuth2Service;
use crate::services::oauth2_github_client::{OAuth2GithubClient, OAuth2GithubClientError};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::login::OAuth2WebflowStateId;
use golem_schema::Empty;
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use poem::http::{Method, StatusCode, Uri};
use poem::test::TestClient;
use poem::{Endpoint, IntoEndpoint, Request};
use serde_json::json;
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use tokio::task::JoinSet;

/// Records the access tokens that the registry sends to GitHub.
#[derive(Default)]
struct RecordingGithubClient {
    access_tokens: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl OAuth2GithubClient for RecordingGithubClient {
    async fn get_authorize_url(&self, _state: &OAuth2WebflowStateId) -> String {
        String::new()
    }

    async fn exchange_code_for_token(
        &self,
        _code: &str,
        _state: &OAuth2WebflowStateId,
    ) -> Result<String, OAuth2GithubClientError> {
        Ok(String::new())
    }

    async fn get_external_login(
        &self,
        access_token: &str,
    ) -> Result<ExternalLogin, OAuth2GithubClientError> {
        self.access_tokens
            .lock()
            .unwrap()
            .push(access_token.to_string());
        Ok(ExternalLogin {
            external_id: "4242".to_string(),
            name: Some("Octo Cat".to_string()),
            email: Some("octocat@example.com".to_string()),
            verified_emails: vec!["octocat@example.com".to_string()],
        })
    }
}

struct Fixture {
    services: Services,
    github: Arc<RecordingGithubClient>,
    _tasks: JoinSet<anyhow::Result<()>>,
    _directory: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let db = DbSqliteConfig {
        database: directory
            .path()
            .join("registry.db")
            .to_string_lossy()
            .into(),
        max_connections: 4,
        foreign_keys: true,
    };
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(db.clone()),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let mut tasks = JoinSet::new();
    let mut services = Services::new_without_component_builtins(&config, &mut tasks)
        .await
        .unwrap();

    let pool = SqlitePool::configured(&db).await.unwrap();
    let github = Arc::new(RecordingGithubClient::default());
    let oauth2_service = OAuth2Service::new(
        github.clone(),
        services.account_service.clone(),
        services.token_service.clone(),
        Arc::new(DbOAuth2TokenRepo::new(pool.clone())),
        Arc::new(DbOAuth2WebflowStateRepo::new(pool)),
        &OAuth2Config {
            webflow_state_expiry: std::time::Duration::from_secs(600),
            cli_redirect: url::Url::parse("https://golem.cloud").unwrap(),
            allowed_redirect_domains: Vec::new(),
        },
    )
    .unwrap();
    services.login_system = LoginSystem::Enabled(LoginSystemEnabled {
        oauth2_service: Arc::new(oauth2_service),
    });

    Fixture {
        services,
        github,
        _tasks: tasks,
        _directory: directory,
    }
}

#[test]
#[timeout("120s")]
async fn login_oauth2_takes_the_access_token_from_the_request_body() {
    let fixture = fixture().await;
    let api = TestClient::new(make_open_api_service(&fixture.services));

    let response = api
        .post("/v1/login/oauth2")
        .body_json(&json!({ "provider": "github", "accessToken": "gho_body_token" }))
        .send()
        .await;

    response.assert_status_is_ok();
    assert_eq!(
        *fixture.github.access_tokens.lock().unwrap(),
        vec!["gho_body_token".to_string()]
    );
}

#[test]
#[timeout("120s")]
async fn login_oauth2_refuses_an_access_token_in_the_query() {
    let fixture = fixture().await;
    let api = TestClient::new(make_open_api_service(&fixture.services));

    let response = api
        .post("/v1/login/oauth2")
        .query("provider", &"github")
        .query("access-token", &"gho_query_token")
        .send()
        .await;

    response.assert_status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert!(fixture.github.access_tokens.lock().unwrap().is_empty());
}

/// Sends the body to the login endpoint and returns the error of the endpoint. The tracing
/// middleware records the message of this error.
async fn login_error(fixture: &Fixture, body: &'static str) -> poem::Error {
    let endpoint = make_open_api_service(&fixture.services).into_endpoint();
    let request = Request::builder()
        .method(Method::POST)
        .uri(Uri::from_static("/v1/login/oauth2"))
        .content_type("application/json")
        .body(body);
    match endpoint.call(request).await {
        Ok(_) => panic!("the endpoint accepted the body"),
        Err(error) => error,
    }
}

#[test]
#[timeout("120s")]
async fn login_oauth2_leaves_a_wrongly_typed_access_token_out_of_the_error() {
    let fixture = fixture().await;

    let error = login_error(
        &fixture,
        r#"{"provider":"github","accessToken":["gho_typed_token"]}"#,
    )
    .await;

    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    let message = error.to_string();
    assert!(
        message.contains(r#"{"provider":"github","accessToken":"<string>"}"#),
        "{message}"
    );
    assert!(!message.contains("gho_typed_token"), "{message}");
    assert!(fixture.github.access_tokens.lock().unwrap().is_empty());
}

#[test]
#[timeout("120s")]
async fn login_oauth2_leaves_a_top_level_array_out_of_the_error() {
    let fixture = fixture().await;

    let error = login_error(
        &fixture,
        r#"[{"provider":"github","accessToken":"gho_array_token"}]"#,
    )
    .await;

    assert_eq!(error.status(), StatusCode::BAD_REQUEST);
    assert!(!error.to_string().contains("gho_array_token"), "{error}");
    assert!(fixture.github.access_tokens.lock().unwrap().is_empty());
}
