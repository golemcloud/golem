use super::*;
use crate::repo::deployment::DbDeploymentRepo;
use crate::repo::environment::DbEnvironmentRepo;
use crate::repo::mcp_oauth::DbMcpOAuthGrantRepo;
use crate::repo::security_scheme::DbSecuritySchemeRepo;
use bytes::Bytes;
use golem_common::config::DbSqliteConfig;
use golem_common::model::account::AccountEmail;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    ClassPermissionPattern, EffectiveSurface, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentSecuritySchemeVerb, PermissionPattern,
};
use golem_common::model::mcp_import::{McpImportAuthInput, McpImportDeployment};
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::db::{self, PoolApi};
use golem_service_base::migration::{Migrations, MigrationsDir};
use golem_service_base::repo::{Blob, SqlDateTime};
use http::{Request, Response};
use http_body_util::Full;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use test_r::test;
use uuid::Uuid;

struct Fixture {
    service: McpOAuthService,
    pool: SqlitePool,
    source: McpImportSource,
    owner: AccountId,
    operator: AuthCtx,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = DbSqliteConfig {
            database: directory
                .path()
                .join("oauth.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 4,
            foreign_keys: true,
        };
        let migrations = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("db/migration");
        db::sqlite::migrate(&config, MigrationsDir::new(migrations).sqlite_migrations())
            .await
            .unwrap();
        let pool = SqlitePool::configured(&config).await.unwrap();
        let owner = AccountId::new();
        let operator_id = AccountId::new();
        let app = Uuid::new_v4();
        let env = Uuid::new_v4();
        let scheme = Uuid::new_v4();
        let now = SqlDateTime::now();
        let mut db = pool.with_rw("oauth-service-test", "seed");
        for id in [owner, operator_id] {
            db.execute(sqlx::query("INSERT INTO accounts (account_id,email,created_at,updated_at,modified_by,current_revision_id) VALUES ($1,$2,$3,$3,$1,1)").bind(id.0).bind(format!("{id}@test.invalid")).bind(now.clone())).await.unwrap();
        }
        db.execute(sqlx::query("INSERT INTO applications (application_id,name,account_id,created_at,updated_at,modified_by,current_revision_id) VALUES ($1,'app',$2,$3,$3,$2,1)").bind(app).bind(owner.0).bind(now.clone())).await.unwrap();
        db.execute(sqlx::query("INSERT INTO environments (environment_id,name,application_id,created_at,updated_at,modified_by,current_revision_id) VALUES ($1,'env',$2,$3,$3,$4,1)").bind(env).bind(app).bind(now.clone()).bind(owner.0)).await.unwrap();
        db.execute(sqlx::query("INSERT INTO environment_revisions (environment_id,revision_id,name,hash,created_at,created_by,deleted,compatibility_check,version_check,security_overrides) VALUES ($1,1,'env',$2,$3,$4,false,false,false,false)").bind(env).bind(vec![0_u8;32]).bind(now.clone()).bind(owner.0)).await.unwrap();
        db.execute(sqlx::query("INSERT INTO security_schemes (security_scheme_id,environment_id,name,created_at,updated_at,modified_by,current_revision_id) VALUES ($1,$2,'oauth',$3,$3,$4,1)").bind(scheme).bind(env).bind(now.clone()).bind(owner.0)).await.unwrap();
        db.execute(sqlx::query("INSERT INTO security_scheme_revisions (security_scheme_id,revision_id,provider_type,client_id,client_secret,redirect_url,scopes,custom_provider_name,custom_issuer_url,created_at,created_by,deleted) VALUES ($1,1,'custom','client','private-secret','\"https://callback.example/complete\"','[\"tools\"]','provider','https://issuer.example',$2,$3,false)").bind(scheme).bind(now.clone()).bind(owner.0)).await.unwrap();
        for revision in [1_i64, 2] {
            db.execute(sqlx::query("INSERT INTO deployment_revisions (environment_id,revision_id,version,hash,created_at,created_by) VALUES ($1,$2,$3,$4,$5,$6)").bind(env).bind(revision).bind(format!("v{revision}")).bind(vec![0_u8;32]).bind(now.clone()).bind(owner.0)).await.unwrap();
        }
        let recipient = RecipientPattern::Account {
            account: AccountEmail::new(format!("{operator_id}@test.invalid")),
        };
        let grant = PermissionPattern::EnvironmentSecurityScheme(ClassPermissionPattern {
            verb: Some(EnvironmentSecuritySchemeVerb::Update),
            owner: EnvironmentOwnerPattern::Environment {
                account: AccountEmail::new(format!("{owner}@test.invalid")),
                application: golem_common::model::application::ApplicationName("app".into()),
                environment: golem_common::model::environment::EnvironmentName("env".into()),
            },
            recipient: recipient.clone(),
            resource: EnvironmentSecuritySchemeResourcePattern::Any,
        });
        let operator = AuthCtx::agent_with_effective_surface(
            operator_id,
            AccountEmail::new(format!("{operator_id}@test.invalid")),
            EffectiveSurface::from_grants(&[grant], &[], &[], &[], &recipient).unwrap(),
        );
        let fixture = Self {
            service: McpOAuthService::new(
                Arc::new(DbEnvironmentRepo::new(pool.clone())),
                Arc::new(DbDeploymentRepo::new(pool.clone())),
                Arc::new(DbSecuritySchemeRepo::new(pool.clone())),
                Arc::new(DbMcpOAuthGrantRepo::new(pool.clone())),
                oauth::Limits {
                    timeout: Duration::from_secs(2),
                    ..Default::default()
                },
            ),
            pool,
            source: McpImportSource {
                environment_id: EnvironmentId(env),
                deployment_revision: 1_i64.try_into().unwrap(),
                import_index: 0,
                upstream_tool_name: "tool".into(),
            },
            owner,
            operator,
            _directory: directory,
        };
        fixture.insert_import(1, 0, Some("oauth"), None).await;
        fixture
    }

    async fn insert_import(
        &self,
        revision: i64,
        index: u32,
        scheme: Option<&str>,
        bearer: Option<&str>,
    ) {
        let deployment = McpImportDeployment {
            url: "https://resource.example/mcp".into(),
            auth: bearer.map(|token| McpImportAuthInput {
                bearer: Some(token.into()),
                basic: None,
            }),
            security_scheme: scheme.map(|name| SecuritySchemeName(name.into())),
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        };
        let (import, credential) = deployment.into_parts(self.source.environment_id).unwrap();
        self.pool.with_rw("oauth-service-test", "import").execute(sqlx::query("INSERT INTO deployment_mcp_imports (environment_id,deployment_revision_id,import_index,import_hash,import_config,inline_credential) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(self.source.environment_id.0).bind(revision).bind(i64::from(index)).bind(vec![0_u8;32]).bind(Blob::new(import)).bind(credential.map(Blob::new))).await.unwrap();
    }

    async fn begin(&self) -> Url {
        let mut sender = Queue::discovery();
        let url = self
            .service
            .begin(&self.source, &self.operator, &mut sender)
            .await
            .unwrap();
        assert_eq!(sender.requests.len(), 3);
        assert!(
            sender
                .requests
                .iter()
                .all(|r| !r.headers().contains_key("authorization"))
        );
        url
    }

    async fn grant(&self) -> McpOAuthGrantKey {
        let url = self.begin().await;
        let mut sender = Queue::token();
        self.service
            .complete(callback(&url), &self.operator, &mut sender)
            .await
            .unwrap();
        assert_eq!(sender.requests.len(), 1);
        let resolved = self.service.resolve(&self.source).await.unwrap();
        resolved.oauth().unwrap().1
    }

    async fn expire(&self, key: &McpOAuthGrantKey) {
        let mut tokens = self
            .service
            .grants
            .load(key)
            .await
            .unwrap()
            .unwrap()
            .tokens
            .unwrap();
        tokens.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        self.pool
            .with_rw("oauth-service-test", "expire")
            .execute(
                sqlx::query(
                    "UPDATE mcp_oauth_grants SET token_secrets = $1 WHERE environment_id = $2",
                )
                .bind(serde_json::to_vec(&tokens).unwrap())
                .bind(key.environment_id),
            )
            .await
            .unwrap();
    }

    async fn rotate_scheme(&self) {
        let mut db = self.pool.with_rw("oauth-service-test", "rotate");
        db.execute(sqlx::query("INSERT INTO security_scheme_revisions SELECT security_scheme_id, revision_id + 1, provider_type, client_id, client_secret, redirect_url, scopes, created_at, created_by, deleted, custom_provider_name, custom_issuer_url FROM security_scheme_revisions WHERE revision_id = 1")).await.unwrap();
        db.execute(sqlx::query(
            "UPDATE security_schemes SET current_revision_id = 2",
        ))
        .await
        .unwrap();
    }
}

struct Queue {
    requests: Vec<Request<Bytes>>,
    responses: VecDeque<Response<Full<Bytes>>>,
}

fn json_response(value: Value) -> Response<Full<Bytes>> {
    Response::builder()
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(value.to_string())))
        .unwrap()
}

impl Queue {
    fn discovery() -> Self {
        Self {
            requests: vec![],
            responses: VecDeque::from([
                json!({"tools":[]}),
                json!({"resource":"https://resource.example/mcp", "authorization_servers":["https://issuer.example"]}),
                json!({"issuer":"https://issuer.example", "authorization_endpoint":"https://issuer.example/authorize", "token_endpoint":"https://issuer.example/token", "response_types_supported":["code"], "code_challenge_methods_supported":["S256"], "authorization_response_iss_parameter_supported":true}),
            ].map(json_response)),
        }
    }

    fn token() -> Self {
        Self {
            requests: vec![],
            responses: VecDeque::from([
                json!({"access_token":"issued", "refresh_token":"rotating", "token_type":"Bearer", "expires_in":3600}),
            ].map(json_response)),
        }
    }

    fn empty() -> Self {
        Self {
            requests: vec![],
            responses: VecDeque::new(),
        }
    }
}

impl HttpSend for Queue {
    type Body = Full<Bytes>;
    type Error = McpOAuthError;

    async fn send(&mut self, request: Request<Bytes>) -> Result<Response<Self::Body>, Self::Error> {
        self.requests.push(request);
        self.responses
            .pop_front()
            .ok_or(TransportError::Network.into())
    }
}

fn callback(url: &Url) -> McpOAuthCallback {
    McpOAuthCallback {
        state: url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned(),
        issuer: Some("https://issuer.example".into()),
        code: Some("private-code".into()),
        error: None,
    }
}

#[test]
async fn consent_binds_owner_actor_pkce_and_saved_metadata() {
    let fixture = Fixture::new().await;
    let url = fixture.begin().await;
    let key = fixture
        .service
        .resolve(&fixture.source)
        .await
        .unwrap()
        .oauth()
        .unwrap()
        .1;
    assert_eq!(key.credential_owner_account_id, fixture.owner.0);
    assert_ne!(
        key.credential_owner_account_id,
        fixture.operator.access_account_id().0
    );
    let mut sender = Queue::token();
    fixture
        .service
        .complete(callback(&url), &fixture.operator, &mut sender)
        .await
        .unwrap();
    assert_eq!(sender.requests.len(), 1, "callback must not rediscover");
    let request = &sender.requests[0];
    assert_eq!(request.uri(), "https://issuer.example/token");
    let form: BTreeMap<_, _> = url::form_urlencoded::parse(request.body())
        .into_owned()
        .collect();
    assert_eq!(form["resource"], "https://resource.example/mcp");
    assert_eq!(form["code"], "private-code");
    let challenge = oauth2::PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
        form["code_verifier"].clone(),
    ));
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "code_challenge")
            .unwrap()
            .1,
        challenge.as_str()
    );
    let stored = fixture
        .service
        .grants
        .load(&key)
        .await
        .unwrap()
        .unwrap()
        .tokens
        .unwrap();
    assert_eq!(
        stored.session.authorized_by,
        fixture.operator.actor_account_id().0
    );
    assert!(
        stored
            .session
            .server
            .authorization_response_iss_parameter_supported
    );
    assert_eq!(stored.scopes, vec!["tools"]);
    assert!(matches!(
        fixture
            .service
            .complete(callback(&url), &fixture.operator, &mut sender)
            .await,
        Err(McpOAuthError::InvalidCallback)
    ));
    assert_eq!(sender.requests.len(), 1);
    let credential = fixture
        .service
        .credential(&fixture.source, fixture.owner, &mut sender)
        .await
        .unwrap();
    assert!(
        matches!(credential.credential, Some(McpImportCredential::Bearer { token }) if token == "issued")
    );
    assert_eq!(
        sender.requests.len(),
        1,
        "unexpired access must not call OAuth"
    );
}

#[test]
async fn callback_errors_validate_issuer_and_never_exchange_or_leak() {
    let fixture = Fixture::new().await;
    for issuer in [
        None,
        Some("https://wrong.example".into()),
        Some("https://issuer.example".into()),
    ] {
        let url = fixture.begin().await;
        let mut data = callback(&url);
        data.issuer = issuer.clone();
        data.code = None;
        data.error = Some("private-provider-error".into());
        let mut sender = Queue::empty();
        let error = fixture
            .service
            .complete(data, &fixture.operator, &mut sender)
            .await
            .unwrap_err();
        if issuer.as_deref() == Some("https://issuer.example") {
            assert!(matches!(error, McpOAuthError::ConsentDenied));
        } else {
            assert!(matches!(
                error,
                McpOAuthError::Transport(TransportError::Configuration(_))
            ));
        }
        assert!(!error.to_safe_string().contains("private-provider-error"));
        assert!(sender.requests.is_empty());
    }
}

#[test]
async fn credential_is_exact_deployment_scoped_and_not_an_admin_view() {
    let fixture = Fixture::new().await;
    fixture
        .insert_import(2, 0, None, Some("second-deployment"))
        .await;
    fixture.insert_import(1, 1, None, None).await;
    let mut sender = Queue::empty();
    assert!(matches!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut sender)
            .await,
        Err(McpOAuthError::AuthorizationRequired(_))
    ));
    let mut source = fixture.source.clone();
    source.deployment_revision = 2_i64.try_into().unwrap();
    assert!(
        matches!(fixture.service.credential(&source, fixture.owner, &mut sender).await.unwrap().credential, Some(McpImportCredential::Bearer { token }) if token == "second-deployment")
    );
    source = fixture.source.clone();
    source.import_index = 1;
    assert!(
        fixture
            .service
            .credential(&source, fixture.owner, &mut sender)
            .await
            .unwrap()
            .credential
            .is_none()
    );
    source.import_index = 9;
    assert!(matches!(
        fixture
            .service
            .credential(&source, fixture.owner, &mut sender)
            .await,
        Err(McpOAuthError::ImportNotFound)
    ));
    assert!(matches!(
        fixture
            .service
            .credential(
                &fixture.source,
                fixture.operator.access_account_id(),
                &mut sender
            )
            .await,
        Err(McpOAuthError::OwnerMismatch)
    ));
    assert!(sender.requests.is_empty());
}

#[test]
async fn ambiguous_exchange_and_refresh_are_not_repeated() {
    let fixture = Fixture::new().await;
    let url = fixture.begin().await;
    let mut lost_response = Queue::empty();
    assert!(matches!(
        fixture
            .service
            .complete(callback(&url), &fixture.operator, &mut lost_response)
            .await,
        Err(McpOAuthError::Transport(TransportError::Network))
    ));
    assert!(matches!(
        fixture
            .service
            .complete(callback(&url), &fixture.operator, &mut lost_response)
            .await,
        Err(McpOAuthError::InvalidCallback)
    ));
    assert_eq!(lost_response.requests.len(), 1);
    let key = fixture.grant().await;
    fixture.expire(&key).await;
    let mut lost_response = Queue::empty();
    assert!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut lost_response)
            .await
            .is_err()
    );
    assert!(matches!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut lost_response)
            .await,
        Err(McpOAuthError::AuthorizationRequired(_))
    ));
    assert_eq!(lost_response.requests.len(), 1);
    assert_eq!(
        fixture
            .service
            .grants
            .load(&key)
            .await
            .unwrap()
            .unwrap()
            .status,
        McpOAuthGrantStatus::ReauthorizationRequired
    );
}

struct BlockedToken {
    entered: Arc<tokio::sync::Notify>,
    resume: Arc<tokio::sync::Notify>,
    inner: Queue,
}

impl HttpSend for BlockedToken {
    type Body = Full<Bytes>;
    type Error = McpOAuthError;

    async fn send(&mut self, request: Request<Bytes>) -> Result<Response<Self::Body>, Self::Error> {
        let response = self.inner.send(request).await?;
        self.entered.notify_one();
        self.resume.notified().await;
        Ok(response)
    }
}

#[test]
#[test_r::timeout("20s")]
async fn concurrent_refresh_waits_and_disconnect_fences_completion() {
    for change in ["none", "disconnect", "rotate-scheme"] {
        let fixture = Fixture::new().await;
        let key = fixture.grant().await;
        fixture.expire(&key).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let mut sender = BlockedToken {
            entered: entered.clone(),
            resume: resume.clone(),
            inner: Queue::token(),
        };
        let refresh = fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut sender);
        let concurrent = async {
            entered.notified().await;
            let mut waiter = Queue::empty();
            if change == "disconnect" {
                fixture
                    .service
                    .disconnect(&fixture.source, &fixture.operator)
                    .await
                    .unwrap();
            } else if change == "rotate-scheme" {
                fixture.rotate_scheme().await;
            }
            resume.notify_one();
            let result = fixture
                .service
                .credential(&fixture.source, fixture.owner, &mut waiter)
                .await;
            assert!(waiter.requests.is_empty());
            result
        };
        let (refreshed, waited) = tokio::join!(refresh, concurrent);
        if change != "none" {
            assert!(matches!(refreshed, Err(McpOAuthError::ContextChanged)));
            assert!(matches!(
                waited,
                Err(McpOAuthError::AuthorizationRequired(_))
            ));
        } else {
            let refreshed = refreshed.unwrap().oauth_grant.unwrap();
            let waited = waited.unwrap().oauth_grant.unwrap();
            assert_eq!(refreshed.generation, waited.generation);
        }
        assert_eq!(sender.inner.requests.len(), 1);
    }
}

#[test]
async fn abandoned_refresh_has_bounded_wait_and_no_lease_retry() {
    let fixture = Fixture::new().await;
    let key = fixture.grant().await;
    let grant = fixture.service.grants.load(&key).await.unwrap().unwrap();
    fixture
        .service
        .grants
        .claim_refresh(&key, grant.generation)
        .await
        .unwrap()
        .unwrap();
    let mut sender = Queue::empty();
    assert!(matches!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut sender)
            .await,
        Err(McpOAuthError::RefreshUnresolved(name)) if name.0 == "oauth"
    ));
    assert!(sender.requests.is_empty());
    assert_eq!(
        fixture
            .service
            .grants
            .load(&key)
            .await
            .unwrap()
            .unwrap()
            .status,
        McpOAuthGrantStatus::Refreshing
    );
}

#[test]
async fn unexpired_credential_is_not_subject_to_refresh_wait_timeout() {
    let mut fixture = Fixture::new().await;
    fixture.grant().await;
    fixture.service.limits.timeout = Duration::from_nanos(1);

    let mut sender = Queue::empty();
    let credential = fixture
        .service
        .credential(&fixture.source, fixture.owner, &mut sender)
        .await
        .expect("an already-stored unexpired credential requires no refresh wait");

    assert!(matches!(
        credential.credential,
        Some(McpImportCredential::Bearer { token }) if token == "issued"
    ));
    assert!(sender.requests.is_empty());
}

#[test]
#[test_r::timeout("20s")]
async fn cancelled_refresh_is_not_reused_and_explicit_reauthorization_recovers() {
    let fixture = Fixture::new().await;
    let key = fixture.grant().await;
    fixture.expire(&key).await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let mut sender = BlockedToken {
        entered: entered.clone(),
        resume: Arc::new(tokio::sync::Notify::new()),
        inner: Queue::token(),
    };
    {
        let pending = fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut sender);
        tokio::pin!(pending);
        tokio::select! {
            _ = entered.notified() => (),
            _ = &mut pending => panic!("token response must still be pending"),
        }
    }
    assert_eq!(sender.inner.requests.len(), 1);
    let mut waiter = Queue::empty();
    assert!(matches!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut waiter)
            .await,
        Err(McpOAuthError::RefreshUnresolved(_))
    ));
    assert!(waiter.requests.is_empty());
    fixture.grant().await;
    assert!(
        fixture
            .service
            .credential(&fixture.source, fixture.owner, &mut waiter)
            .await
            .is_ok()
    );
    assert!(waiter.requests.is_empty());
}

#[test]
async fn consent_rechecks_operator_and_scheme_before_exchange() {
    for change in ["authority", "actor", "scheme"] {
        let fixture = Fixture::new().await;
        let url = fixture.begin().await;
        let mut auth = fixture.operator.clone();
        match change {
            "authority" => {
                let AuthCtx::Agent(agent) = &mut auth else {
                    unreachable!()
                };
                agent.effective_surface = EffectiveSurface {
                    source_card_ids: vec![],
                    lower: vec![],
                    upper: vec![],
                };
            }
            "actor" => auth = AuthCtx::System,
            "scheme" => fixture.rotate_scheme().await,
            _ => unreachable!(),
        }
        let mut sender = Queue::empty();
        let error = fixture
            .service
            .complete(callback(&url), &auth, &mut sender)
            .await
            .unwrap_err();
        assert!(matches!(
            (change, error),
            ("authority", McpOAuthError::Unauthorized(_))
                | ("actor", McpOAuthError::InvalidCallback)
                | ("scheme", McpOAuthError::ContextChanged)
        ));
        assert!(sender.requests.is_empty());
        if change == "authority" {
            assert!(matches!(
                fixture
                    .service
                    .begin(&fixture.source, &auth, &mut sender)
                    .await,
                Err(McpOAuthError::Unauthorized(_))
            ));
            assert!(matches!(
                fixture.service.disconnect(&fixture.source, &auth).await,
                Err(McpOAuthError::Unauthorized(_))
            ));
            assert!(sender.requests.is_empty());
        }
    }
}

#[test]
async fn refreshed_tokens_preserve_omitted_refresh_and_scopes_but_replace_present_values() {
    let fixture = Fixture::new().await;
    let key = fixture.grant().await;
    let previous = fixture
        .service
        .grants
        .load(&key)
        .await
        .unwrap()
        .unwrap()
        .tokens
        .unwrap();
    let now = Utc::now();
    let response = serde_json::from_value(
        json!({"access_token":"new", "token_type":"Bearer", "expires_in":50}),
    )
    .unwrap();
    let next = tokens(response, now, previous.session.clone(), Some(&previous)).unwrap();
    assert_eq!(next.access_token, "new");
    assert_eq!(next.refresh_token.as_deref(), Some("rotating"));
    assert_eq!(next.scopes, vec!["tools"]);
    assert_eq!(next.expires_at, Some(now + chrono::Duration::seconds(50)));
    let response = serde_json::from_value(json!({"access_token":"third", "token_type":"Bearer", "refresh_token":"replacement", "scope":"narrower"})).unwrap();
    let next = tokens(response, now, previous.session.clone(), Some(&previous)).unwrap();
    assert_eq!(next.refresh_token.as_deref(), Some("replacement"));
    assert_eq!(next.scopes, vec!["narrower"]);
    assert_eq!(next.expires_at, None);
    for expiry in [0_u64, u64::MAX] {
        let response = serde_json::from_value(
            json!({"access_token":"invalid", "token_type":"Bearer", "expires_in":expiry}),
        )
        .unwrap();
        assert!(tokens(response, now, previous.session.clone(), Some(&previous)).is_err());
    }
}

#[test]
async fn consent_probes_deployed_resource_and_uses_its_challenge_metadata_url() {
    let fixture = Fixture::new().await;
    let mut sender = Queue::discovery();
    let probe = sender.responses.front_mut().unwrap();
    *probe.status_mut() = http::StatusCode::UNAUTHORIZED;
    probe.headers_mut().insert(
        http::header::WWW_AUTHENTICATE,
        http::HeaderValue::from_static(
            "Bearer resource_metadata=\"https://resource.example/custom-discovery\"",
        ),
    );
    fixture
        .service
        .begin(&fixture.source, &fixture.operator, &mut sender)
        .await
        .unwrap();
    assert_eq!(sender.requests.len(), 3);
    assert_eq!(sender.requests[0].uri(), "https://resource.example/mcp");
    assert_eq!(sender.requests[0].headers()["Mcp-Method"], "tools/list");
    assert_eq!(
        sender.requests[1].uri(),
        "https://resource.example/custom-discovery"
    );
    assert_eq!(sender.requests[1].method(), http::Method::GET);
    assert!(
        sender
            .requests
            .iter()
            .all(|r| !r.headers().contains_key("authorization"))
    );
}

#[test]
async fn failed_probe_does_not_discover_or_supersede_an_existing_grant() {
    let fixture = Fixture::new().await;
    let key = fixture.grant().await;
    let original = fixture.service.grants.load(&key).await.unwrap().unwrap();
    let mut sender = Queue::discovery();
    *sender.responses.front_mut().unwrap().status_mut() = http::StatusCode::SERVICE_UNAVAILABLE;
    assert!(matches!(
        fixture
            .service
            .begin(&fixture.source, &fixture.operator, &mut sender)
            .await,
        Err(McpOAuthError::Transport(TransportError::HttpStatus(503)))
    ));
    assert_eq!(sender.requests.len(), 1);
    let stored = fixture.service.grants.load(&key).await.unwrap().unwrap();
    assert_eq!(stored.generation, original.generation);
    assert_eq!(stored.status, McpOAuthGrantStatus::Granted);
}

#[test]
async fn invalid_oauth_resource_or_protocol_is_rejected_before_probe() {
    let fixture = Fixture::new().await;
    for (url, version) in [
        ("http://resource.example/mcp", None),
        (" https://resource.example/mcp ", None),
        ("https://resource.example/mcp", Some("1999-01-01")),
    ] {
        let mut import = fixture
            .service
            .resolve(&fixture.source)
            .await
            .unwrap()
            .import;
        import.url = url.into();
        import.version = version.map(str::to_owned);
        fixture.pool.with_rw("oauth-service-test", "invalid-import").execute(
            sqlx::query("UPDATE deployment_mcp_imports SET import_config = $1 WHERE environment_id = $2")
                .bind(Blob::new(import)).bind(fixture.source.environment_id.0)
        ).await.unwrap();
        let mut sender = Queue::empty();
        assert!(matches!(
            fixture
                .service
                .begin(&fixture.source, &fixture.operator, &mut sender)
                .await,
            Err(McpOAuthError::Transport(TransportError::Configuration(_)))
        ));
        assert!(sender.requests.is_empty());
    }
}
