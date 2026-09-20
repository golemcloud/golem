use super::*;
use crate::bootstrap::Services;
use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::card::owner::{EmptyOwnerPattern, EnvironmentOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    ClassPermissionPattern, EffectiveSurface, EnvironmentResourcePattern, EnvironmentVerb,
    NetworkResourcePattern, NetworkVerb, PermissionPattern, PortPattern,
};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::model::mcp_import::{McpImportAuthInput, McpImportDeployment};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService, RegistryServiceError,
};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::{PoolApi, sqlite::SqlitePool};
use golem_service_base::repo::{Blob, SqlDateTime};
use poem::listener::{Acceptor, Listener, RustlsCertificate, RustlsConfig};
use serde_json::{Value, json};
use sqlx::Row;
use std::time::Duration;
use test_r::{test, timeout};
use tokio::sync::Notify;
use tokio::task::JoinSet;

type ResponseStep = (u16, Value, Option<Arc<Notify>>);

struct Upstream {
    url: String,
    port: u16,
    replies: Arc<Mutex<VecDeque<ResponseStep>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    authorizations: Arc<Mutex<Vec<Option<String>>>>,
    refreshes: Arc<Mutex<usize>>,
    token_forms: Arc<Mutex<Vec<HashMap<String, String>>>>,
    entered: Arc<Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Upstream {
    async fn new() -> Self {
        Self::with_oauth(false).await
    }

    async fn with_oauth(require_auth: bool) -> Self {
        let tls = RustlsConfig::new().fallback(
            RustlsCertificate::new()
                .cert(include_bytes!("tls/server-cert.pem"))
                .key(include_bytes!("tls/server-key.pem")),
        );
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .rustls(tls)
            .into_acceptor()
            .await
            .unwrap();
        let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
        let replies = Arc::new(Mutex::new(VecDeque::<ResponseStep>::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let refreshes = Arc::new(Mutex::new(0));
        let token_forms = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let origin = format!("https://127.0.0.1:{port}");
        let handler = poem::endpoint::make({
            let (replies, requests, authorizations, refreshes, token_forms, entered) = (
                replies.clone(),
                requests.clone(),
                authorizations.clone(),
                refreshes.clone(),
                token_forms.clone(),
                entered.clone(),
            );
            let origin = origin.clone();
            move |mut request: poem::Request| {
                let (replies, requests, authorizations, refreshes, token_forms, entered) = (
                    replies.clone(),
                    requests.clone(),
                    authorizations.clone(),
                    refreshes.clone(),
                    token_forms.clone(),
                    entered.clone(),
                );
                let origin = origin.clone();
                async move {
                    let path = request.uri().path();
                    if path == "/.well-known/oauth-protected-resource/mcp" {
                        return poem::Response::builder()
                            .header("content-type", "application/json")
                            .body(json!({"resource":format!("{origin}/mcp"), "authorization_servers":[origin]}).to_string());
                    }
                    if path == "/.well-known/oauth-authorization-server" {
                        return poem::Response::builder()
                            .header("content-type", "application/json")
                            .body(json!({"issuer":origin, "authorization_endpoint":format!("{origin}/authorize"), "token_endpoint":format!("{origin}/token"), "response_types_supported":["code"], "code_challenge_methods_supported":["S256"], "authorization_response_iss_parameter_supported":true}).to_string());
                    }
                    if path == "/token" {
                        let body = request.take_body().into_bytes().await.unwrap();
                        let form: HashMap<String, String> =
                            url::form_urlencoded::parse(&body).into_owned().collect();
                        let refresh = form.get("grant_type").is_some_and(|v| v == "refresh_token");
                        if refresh {
                            *refreshes.lock().unwrap() += 1;
                        }
                        token_forms.lock().unwrap().push(form);
                        return poem::Response::builder()
                            .header("content-type", "application/json")
                            .body(if refresh {
                                json!({"access_token":"rotated", "refresh_token":"rotated-refresh", "token_type":"Bearer", "expires_in":3600}).to_string()
                            } else {
                                json!({"access_token":"issued", "refresh_token":"initial-refresh", "token_type":"Bearer", "expires_in":3600}).to_string()
                            });
                    }
                    if require_auth
                        && path == "/mcp"
                        && !request.headers().contains_key("authorization")
                    {
                        return poem::Response::builder()
                            .status(http::StatusCode::UNAUTHORIZED)
                            .header("www-authenticate", format!("Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\""))
                            .body("");
                    }
                    authorizations.lock().unwrap().push(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_owned),
                    );
                    let request: Value = request.take_body().into_json().await.unwrap();
                    requests.lock().unwrap().push(request.clone());
                    let (status, result, wait) = replies
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected upstream dispatch");
                    entered.notify_one();
                    if let Some(wait) = wait {
                        wait.notified().await;
                    }
                    let body = if let Some(error) = result.get("error") {
                        json!({"jsonrpc":"2.0", "id":request["id"], "error":error})
                    } else {
                        json!({"jsonrpc":"2.0", "id": request["id"], "result": result})
                    };
                    poem::Response::builder()
                        .status(http::StatusCode::from_u16(status).unwrap())
                        .header("content-type", "application/json")
                        .body(body.to_string())
                }
            }
        });
        let task = tokio::spawn(async move {
            poem::Server::new_with_acceptor(acceptor)
                .run(handler)
                .await
                .unwrap();
        });
        Self {
            url: format!("https://127.0.0.1:{port}/mcp"),
            port,
            replies,
            requests,
            authorizations,
            refreshes,
            token_forms,
            entered,
            task,
        }
    }

    fn reply(&self, status: u16, result: Value) {
        self.replies
            .lock()
            .unwrap()
            .push_back((status, result, None));
    }
    fn blocked(&self, result: Value) -> Arc<Notify> {
        let resume = Arc::new(Notify::new());
        self.replies
            .lock()
            .unwrap()
            .push_back((200, result, Some(resume.clone())));
        resume
    }
    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    fn refresh_count(&self) -> usize {
        *self.refreshes.lock().unwrap()
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Fixture {
    services: Services,
    pool: SqlitePool,
    source: McpImportSource,
    auth: AuthCtx,
    _tasks: JoinSet<anyhow::Result<()>>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(upstream: &Upstream) -> Self {
        Self::with_http_limit(upstream, 100).await
    }

    async fn with_http_limit(upstream: &Upstream, monthly_http_call_limit: u64) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = DbSqliteConfig {
            database: directory
                .path()
                .join("registry.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 4,
            foreign_keys: true,
        };
        let mut config = RegistryServiceConfig {
            db: DbConfig::Sqlite(db.clone()),
            login: LoginConfig::Disabled(Empty {}),
            blob_storage: BlobStorageConfig::default_in_memory(),
            component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
            ..Default::default()
        };
        config
            .initial_plans
            .get_mut("default")
            .unwrap()
            .monthly_http_call_limit = monthly_http_call_limit;
        let root = &config.initial_accounts["root"];
        let mut tasks = JoinSet::new();
        let services = Services::new(&config, &mut tasks).await.unwrap();
        services.mcp_oauth_service.trust_certificate(
            reqwest::Certificate::from_pem(include_bytes!("tls/ca.pem")).unwrap(),
        );
        let app = services
            .application_service
            .create(
                root.id,
                ApplicationCreation {
                    name: ApplicationName("imports".into()),
                },
                &AuthCtx::System,
            )
            .await
            .unwrap();
        let env = services
            .environment_service
            .create(
                app.id,
                EnvironmentCreation {
                    name: EnvironmentName("test".into()),
                    compatibility_check: false,
                    tool_compatibility_mode: Default::default(),
                    version_check: false,
                    security_overrides: false,
                },
                &AuthCtx::System,
            )
            .await
            .unwrap();
        let pool = SqlitePool::configured(&db).await.unwrap();
        for revision in [1_i64, 2] {
            pool.with_rw("mcp-resolver-test", "seed").execute(sqlx::query("INSERT INTO deployment_revisions (environment_id,revision_id,version,hash,created_at,created_by) VALUES ($1,$2,$3,$4,$5,$6)")
                .bind(env.id.0).bind(revision).bind(format!("v{revision}")).bind(vec![0_u8;32]).bind(SqlDateTime::now()).bind(root.id.0)).await.unwrap();
        }
        let recipient = RecipientPattern::Account {
            account: root.email.clone(),
        };
        let permission = PermissionPattern::Network(ClassPermissionPattern {
            verb: Some(NetworkVerb::Connect),
            owner: EmptyOwnerPattern,
            recipient: recipient.clone(),
            resource: NetworkResourcePattern::host_port(
                "127.0.0.1",
                PortPattern::single(upstream.port),
            ),
        });
        let auth = AuthCtx::agent_with_effective_surface(
            root.id,
            root.email.clone(),
            EffectiveSurface::from_grants(&[permission], &[], &[], &[], &recipient).unwrap(),
        );
        let source = McpImportSource {
            environment_id: env.id,
            deployment_revision: 1_i64.try_into().unwrap(),
            import_index: 0,
            upstream_tool_name: String::new(),
        };
        let fixture = Self {
            services,
            pool,
            source,
            auth,
            _tasks: tasks,
            _directory: directory,
        };
        fixture.import(1, 0, &upstream.url, None).await;
        fixture
    }

    async fn import(&self, revision: i64, index: u32, url: &str, token: Option<&str>) {
        let (import, credential) = McpImportDeployment {
            url: url.into(),
            auth: token.map(|token| McpImportAuthInput {
                bearer: Some(token.into()),
                basic: None,
            }),
            security_scheme: None,
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        }
        .into_parts(self.source.environment_id)
        .unwrap();
        self.pool.with_rw("mcp-resolver-test", "import").execute(sqlx::query("INSERT INTO deployment_mcp_imports (environment_id,deployment_revision_id,import_index,import_hash,import_config,inline_credential) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(self.source.environment_id.0).bind(revision).bind(i64::from(index)).bind(vec![0_u8;32]).bind(Blob::new(import)).bind(credential.map(Blob::new))).await.unwrap();
    }

    async fn oauth_import_and_grant(&self, upstream: &Upstream) -> uuid::Uuid {
        let issuer = format!("https://127.0.0.1:{}", upstream.port);
        let scheme = uuid::Uuid::new_v4();
        let owner: uuid::Uuid = self
            .pool
            .with_ro("mcp-resolver-test", "owner")
            .fetch_one(sqlx::query("SELECT account_id FROM applications LIMIT 1"))
            .await
            .unwrap()
            .try_get("account_id")
            .unwrap();
        let now = SqlDateTime::now();
        let mut db = self.pool.with_rw("mcp-resolver-test", "oauth");
        db.execute(sqlx::query("INSERT INTO security_schemes (security_scheme_id,environment_id,name,created_at,updated_at,modified_by,current_revision_id) VALUES ($1,$2,'oauth',$3,$3,$4,1)").bind(scheme).bind(self.source.environment_id.0).bind(now.clone()).bind(owner)).await.unwrap();
        db.execute(sqlx::query("INSERT INTO security_scheme_revisions (security_scheme_id,revision_id,provider_type,client_id,client_secret,redirect_url,scopes,custom_provider_name,custom_issuer_url,created_at,created_by,deleted) VALUES ($1,1,'custom','client','secret','\"https://callback.example/complete\"','[\"tools\"]','provider',$2,$3,$4,false)").bind(scheme).bind(&issuer).bind(now).bind(owner)).await.unwrap();
        db.execute(sqlx::query("DELETE FROM deployment_mcp_imports WHERE environment_id=$1 AND deployment_revision_id=1 AND import_index=0").bind(self.source.environment_id.0)).await.unwrap();
        drop(db);
        let (import, credential) = McpImportDeployment {
            url: upstream.url.clone(),
            auth: None,
            security_scheme: Some(golem_common::model::security_scheme::SecuritySchemeName(
                "oauth".into(),
            )),
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        }
        .into_parts(self.source.environment_id)
        .unwrap();
        self.pool.with_rw("mcp-resolver-test", "oauth-import").execute(sqlx::query("INSERT INTO deployment_mcp_imports (environment_id,deployment_revision_id,import_index,import_hash,import_config,inline_credential) VALUES ($1,1,0,$2,$3,$4)")
            .bind(self.source.environment_id.0).bind(vec![0_u8;32]).bind(Blob::new(import)).bind(credential.map(Blob::new))).await.unwrap();

        let consent = self
            .services
            .mcp_oauth_service
            .authorize(&self.source, &AuthCtx::System)
            .await
            .unwrap();
        let state = consent
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        self.services
            .mcp_oauth_service
            .complete_operator(
                &self.source,
                crate::services::mcp_oauth::McpOAuthCallback {
                    state,
                    issuer: Some(issuer),
                    code: Some("code".into()),
                    error: None,
                },
                &AuthCtx::System,
            )
            .await
            .unwrap();
        scheme
    }

    async fn oauth_generation(&self) -> uuid::Uuid {
        self.pool
            .with_ro("mcp-resolver-test", "generation")
            .fetch_one(
                sqlx::query("SELECT generation FROM mcp_oauth_grants WHERE environment_id=$1")
                    .bind(self.source.environment_id.0),
            )
            .await
            .unwrap()
            .try_get("generation")
            .unwrap()
    }

    fn resolver(&self, config: McpImportResolverConfig) -> McpImportResolver {
        McpImportResolver::new(self.services.mcp_oauth_service.clone(), config).unwrap()
    }

    async fn client(&mut self) -> GrpcRegistryService {
        let port = crate::grpc::start_grpc_server(
            &crate::config::GrpcApiConfig {
                port: 0,
                ..Default::default()
            },
            &self.services,
            &mut self._tasks,
        )
        .await
        .unwrap();
        GrpcRegistryService::new(&GrpcRegistryServiceConfig {
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        })
    }

    async fn state(&self) -> ToolDeploymentState {
        self.services
            .deployment_service
            .get_tool_deployment_state_at_revision(
                self.source.environment_id,
                self.source.deployment_revision,
            )
            .await
            .unwrap()
            .unwrap()
    }
}

fn tool(name: &str) -> Value {
    json!({"name":name, "inputSchema":{"type":"object", "properties":{}, "additionalProperties":false}})
}

fn declaration(url: &str) -> McpImportDeployment {
    McpImportDeployment {
        url: url.into(),
        auth: None,
        security_scheme: None,
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    }
}

#[test]
#[timeout("30s")]
async fn deployment_warnings_preserve_collision_indices_and_do_not_cache() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    upstream.reply(
        200,
        json!({"tools":[tool("native"),tool("shared"),tool("first")]}),
    );
    upstream.reply(200, json!({"tools":[tool("shared"),tool("last")]}));
    let resolver = fixture.resolver(Default::default());
    let warnings = resolver
        .deployment_warnings(
            fixture.source.environment_id,
            vec![declaration(&upstream.url); 2],
            vec!["native".into()],
            AuthCtx::System,
        )
        .await;
    let details = warnings
        .iter()
        .map(|warning| match warning {
            DeployValidationWarning::McpImportDiscovery(warning) => {
                (warning.import_index, warning.upstream_tool_name.as_deref())
            }
            _ => panic!("unexpected warning"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        details,
        [(Some(0), Some("native")), (Some(1), Some("shared"))]
    );
    assert!(
        warnings
            .iter()
            .all(|warning| warning.to_string().contains("precedence"))
    );
    assert_eq!(upstream.count(), 2);
    assert!(resolver.inner.cache.lock().unwrap().entries.is_empty());
}

#[test]
#[timeout("30s")]
async fn deployment_warnings_do_not_elevate_permissions_or_expose_inline_credentials() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    let import = McpImportDeployment {
        auth: Some(McpImportAuthInput {
            bearer: Some("private-inline-token".into()),
            basic: None,
        }),
        ..declaration(&upstream.url)
    };
    let warnings = resolver
        .deployment_warnings(
            fixture.source.environment_id,
            vec![import.clone()],
            vec![],
            fixture.auth.clone(),
        )
        .await;
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        upstream.count(),
        0,
        "runtime identity cannot authorize control-plane discovery"
    );
    assert!(
        !serde_json::to_string(&warnings)
            .unwrap()
            .contains("private-inline-token")
    );
    assert!(
        resolver
            .deployment_warnings(
                fixture.source.environment_id,
                vec![],
                vec![],
                fixture.auth.clone(),
            )
            .await
            .is_empty()
    );
    upstream.reply(503, json!({"private":"private-inline-token"}));
    let warnings = resolver
        .deployment_warnings(
            fixture.source.environment_id,
            vec![import],
            vec![],
            AuthCtx::System,
        )
        .await;
    assert!(
        matches!(&warnings[..], [DeployValidationWarning::McpImportDiscovery(w)]
        if w.import_index == Some(0) && w.upstream_tool_name.is_none())
    );
    assert!(
        !serde_json::to_string(&warnings)
            .unwrap()
            .contains("private-inline-token")
    );
    assert_eq!(upstream.count(), 1);
}

#[test]
#[timeout("30s")]
async fn deployment_without_oauth_consent_succeeds_with_discovery_warning() {
    use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
    use golem_common::model::diff::Hashable;
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    fixture
        .services
        .mcp_oauth_service
        .disconnect(&fixture.source, &AuthCtx::System)
        .await
        .unwrap();
    let mut db = fixture.pool.with_rw("mcp-deploy-test", "undeploy");
    db.execute(sqlx::query("DELETE FROM deployment_mcp_imports"))
        .await
        .unwrap();
    db.execute(sqlx::query("DELETE FROM deployment_revisions"))
        .await
        .unwrap();
    drop(db);
    let imports = vec![McpImportDeployment {
        security_scheme: Some(golem_common::model::security_scheme::SecuritySchemeName(
            "oauth".into(),
        )),
        ..declaration(&upstream.url)
    }];
    let plan = fixture
        .services
        .deployment_service
        .get_current_deployment_plan(fixture.source.environment_id, &AuthCtx::System)
        .await
        .unwrap();
    let mut target = plan.to_diffable();
    for ambient in &plan.ambient_tools {
        target.remote_tools.insert(
            ambient.name.to_string(),
            ambient
                .to_diffable(std::iter::empty(), &Default::default(), &Default::default())
                .into(),
        );
    }
    target.mcp_imports = imports
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, import)| {
            (
                index.to_string(),
                import
                    .into_parts(fixture.source.environment_id)
                    .unwrap()
                    .0
                    .into(),
            )
        })
        .collect();
    let before = upstream.count();
    let deployed = fixture
        .services
        .deployment_write_service
        .create_deployment(
            fixture.source.environment_id,
            DeploymentCreation {
                current_revision: plan.current_revision,
                expected_deployment_hash: target.hash().unwrap(),
                version: DeploymentVersion("without-consent".into()),
                agent_secret_defaults: vec![],
                quota_resource_defaults: vec![],
                retry_policy_defaults: vec![],
                publish_tools: vec![],
                remote_tools: vec![],
                mcp_imports: imports,
                publish_tool_middlewares: vec![],
                remote_tool_middlewares: vec![],
                universal_tool_middlewares: vec![],
                replace_incompatible_agent_secrets: false,
            },
            &AuthCtx::System,
        )
        .await
        .unwrap();
    assert!(
        matches!(&deployed.validation_warnings[..], [DeployValidationWarning::McpImportDiscovery(w)]
        if w.import_index == Some(0) && w.reason.contains("security scheme oauth"))
    );
    assert_eq!(upstream.count(), before, "no tools/list without consent");
    let state = fixture
        .services
        .deployment_service
        .get_tool_deployment_state_at_revision(fixture.source.environment_id, deployed.revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.mcp_imports.len(),
        1,
        "warning must not remove the declaration"
    );
}

#[test]
#[timeout("30s")]
async fn preview_before_deployment_paginates_merges_and_does_not_cache() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let mut db = fixture.pool.with_rw("mcp-preview-test", "undeploy");
    db.execute(sqlx::query("DELETE FROM deployment_mcp_imports"))
        .await
        .unwrap();
    db.execute(sqlx::query("DELETE FROM deployment_revisions"))
        .await
        .unwrap();
    drop(db);
    let resolver = fixture.resolver(Default::default());
    let import = McpImportDeployment {
        prefix: Some("p".into()),
        auth: Some(McpImportAuthInput {
            bearer: Some("preview-only".into()),
            basic: None,
        }),
        ..declaration(&upstream.url)
    };
    upstream.reply(
        200,
        json!({"tools":[tool("native"),tool("first")],"nextCursor":"second-page"}),
    );
    upstream.reply(200, json!({"tools":[tool("last")]}));
    upstream.reply(200, json!({"tools":[tool("first"),tool("other")]}));
    let preview = resolver
        .preview(
            fixture.source.environment_id,
            vec![import.clone(), import.clone()],
            vec!["p-native".into()],
            AuthCtx::System,
        )
        .await
        .unwrap();
    assert_eq!(
        preview
            .tools
            .iter()
            .map(|(index, t)| (*index, t.definition.name().unwrap()))
            .collect::<Vec<_>>(),
        [(0, "p-first"), (0, "p-last"), (1, "p-other")]
    );
    assert_eq!(preview.diagnostics.len(), 2);
    assert_eq!(upstream.count(), 3);
    assert_eq!(
        upstream.requests.lock().unwrap()[1]["params"]["cursor"],
        "second-page"
    );
    assert!(
        upstream
            .authorizations
            .lock()
            .unwrap()
            .iter()
            .all(|h| h.as_deref() == Some("Bearer preview-only"))
    );
    let expected =
        project_import(&[tool("other")], Some("p"), None, None, Default::default()).unwrap();
    assert_eq!(
        preview.tools[2].1.digest, expected.tools[0].digest,
        "aggregate budgets do not change projection identity"
    );
    assert!(resolver.inner.cache.lock().unwrap().entries.is_empty());
    upstream.reply(200, json!({"tools":[tool("changed")]}));
    let fresh = resolver
        .preview(
            fixture.source.environment_id,
            vec![import],
            vec![],
            AuthCtx::System,
        )
        .await
        .unwrap();
    assert_eq!(fresh.tools[0].1.definition.name().unwrap(), "p-changed");
    assert_eq!(upstream.count(), 4);
    assert!(
        fixture
            .services
            .deployment_service
            .get_tool_deployment_state_at_revision(
                fixture.source.environment_id,
                fixture.source.deployment_revision
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[test]
#[timeout("30s")]
async fn preview_authorizes_empty_requests_and_bounds_declarations_before_network() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    assert!(
        matches!(resolver.preview(fixture.source.environment_id, vec![], vec![], fixture.auth.clone()).await,
        Err(McpImportResolverError::OAuth(e)) if matches!(e.as_ref(), McpOAuthError::ImportNotFound))
    );
    assert!(
        matches!(resolver.preview(fixture.source.environment_id, vec![declaration(&upstream.url);129], vec![], AuthCtx::System).await,
        Err(McpImportResolverError::OAuth(e)) if matches!(e.as_ref(), McpOAuthError::Transport(TransportError::InvalidInput(_))))
    );
    assert_eq!(upstream.count(), 0);
}

#[test]
#[timeout("30s")]
async fn preview_rejects_whole_result_on_later_failure_or_aggregate_limit() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let mut config = McpImportResolverConfig::default();
    config.projection.max_tools = 1;
    let resolver = fixture.resolver(config);
    for status in [503, 200] {
        upstream.reply(200, json!({"tools":[tool("first")]}));
        upstream.reply(status, json!({"tools":[tool("second")]}));
        assert!(matches!(
            resolver
                .preview(
                    fixture.source.environment_id,
                    vec![declaration(&upstream.url); 2],
                    vec![],
                    AuthCtx::System
                )
                .await,
            Err(McpImportResolverError::SourceUnavailable {
                import_index: 1,
                ..
            })
        ));
    }
    assert!(resolver.inner.cache.lock().unwrap().entries.is_empty());
}

#[test]
#[timeout("30s")]
async fn preview_401_expires_used_grant_and_next_resolution_refreshes_once() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    let import = McpImportDeployment {
        security_scheme: Some(golem_common::model::security_scheme::SecuritySchemeName(
            "oauth".into(),
        )),
        ..declaration(&upstream.url)
    };
    let resolver = fixture.resolver(Default::default());
    let old = fixture.oauth_generation().await;
    upstream.reply(401, json!({}));
    assert!(
        resolver
            .preview(
                fixture.source.environment_id,
                vec![import.clone()],
                vec![],
                AuthCtx::System
            )
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), 1);
    assert_eq!(*upstream.refreshes.lock().unwrap(), 0);
    assert_ne!(fixture.oauth_generation().await, old);
    upstream.reply(200, json!({"tools":[tool("refreshed")]}));
    let preview = resolver
        .preview(
            fixture.source.environment_id,
            vec![import],
            vec![],
            AuthCtx::System,
        )
        .await
        .unwrap();
    assert_eq!(preview.tools[0].1.definition.name().unwrap(), "refreshed");
    assert_eq!(*upstream.refreshes.lock().unwrap(), 1);
    assert_eq!(
        upstream.authorizations.lock().unwrap()[1].as_deref(),
        Some("Bearer rotated")
    );
}

fn names(observation: &McpImportObservation) -> Vec<&str> {
    observation
        .tools
        .iter()
        .map(|tool| tool.definition.name().unwrap())
        .collect()
}

async fn wait_for_requests(upstream: &Upstream, expected: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while upstream.count() < expected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[test]
#[timeout("30s")]
async fn operator_inspection_requires_both_permissions_and_bills_environment_owner() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::with_http_limit(&upstream, 1).await;
    let resolver = fixture.resolver(Default::default());
    let operator_id = AccountId::new();
    let email = AccountEmail::new("collaborator@test.invalid");
    let recipient = RecipientPattern::Account {
        account: email.clone(),
    };
    let config = RegistryServiceConfig::default();
    let auth = |verbs: &[EnvironmentVerb]| {
        let grants: Vec<_> = verbs
            .iter()
            .map(|verb| {
                PermissionPattern::Environment(ClassPermissionPattern {
                    verb: Some(*verb),
                    owner: EnvironmentOwnerPattern::Environment {
                        account: config.initial_accounts["root"].email.clone(),
                        application: ApplicationName("imports".into()),
                        environment: EnvironmentName("test".into()),
                    },
                    recipient: recipient.clone(),
                    resource: EnvironmentResourcePattern::Any,
                })
            })
            .collect();
        AuthCtx::agent_with_effective_surface(
            operator_id,
            email.clone(),
            EffectiveSurface::from_grants(&grants, &[], &[], &[], &recipient).unwrap(),
        )
    };
    for verbs in [
        vec![],
        vec![EnvironmentVerb::ViewTools],
        vec![EnvironmentVerb::ViewTools, EnvironmentVerb::ViewDeployment],
        vec![EnvironmentVerb::View, EnvironmentVerb::ViewTools],
    ] {
        assert!(matches!(
            resolver.inspect(fixture.source.clone(), auth(&verbs), false).await,
            Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::ImportNotFound)
        ));
    }
    assert!(matches!(
        resolver.inspect(fixture.source.clone(), auth(&[EnvironmentVerb::View, EnvironmentVerb::ViewDeployment]), false).await,
        Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::Unauthorized(_))
    ));
    for verbs in [
        vec![EnvironmentVerb::View, EnvironmentVerb::ViewTools],
        vec![EnvironmentVerb::View, EnvironmentVerb::Deploy],
    ] {
        assert!(
            matches!(resolver.preview(fixture.source.environment_id, vec![], vec![], auth(&verbs)).await,
            Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::Unauthorized(_)))
        );
    }
    resolver
        .preview(
            fixture.source.environment_id,
            vec![],
            vec![],
            auth(&[
                EnvironmentVerb::View,
                EnvironmentVerb::Deploy,
                EnvironmentVerb::ViewTools,
            ]),
        )
        .await
        .unwrap();
    assert_eq!(upstream.count(), 0);
    let operator = auth(&[
        EnvironmentVerb::View,
        EnvironmentVerb::ViewTools,
        EnvironmentVerb::ViewDeployment,
    ]);
    upstream.reply(200, json!({"tools":[tool("owned")]}));
    assert_eq!(
        names(
            &resolver
                .inspect(fixture.source.clone(), operator.clone(), false)
                .await
                .unwrap()
        ),
        ["owned"]
    );
    assert_eq!(
        names(
            &resolver
                .inspect(fixture.source.clone(), operator.clone(), false)
                .await
                .unwrap()
        ),
        ["owned"]
    );
    assert_eq!(
        upstream.count(),
        1,
        "cache reads do not charge another HTTP call"
    );
    assert!(matches!(
        resolver.resolve_observation(fixture.source.clone(), operator.clone()).await,
        Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::OwnerMismatch)
    ));
    assert!(matches!(
        resolver.inspect(fixture.source.clone(), operator, true).await,
        Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::AccountUsage(AccountUsageError::LimitExceeded(_)))
    ));
    assert_eq!(upstream.count(), 1, "quota is checked before dispatch");
}

#[test]
#[timeout("30s")]
async fn operator_hidden_import_does_not_reveal_missing_security_scheme() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let (import, _) = McpImportDeployment {
        url: upstream.url.clone(),
        auth: None,
        security_scheme: Some(golem_common::model::security_scheme::SecuritySchemeName(
            "missing".into(),
        )),
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    }
    .into_parts(fixture.source.environment_id)
    .unwrap();
    let mut db = fixture.pool.with_rw("mcp-resolver-test", "missing-scheme");
    db.execute(
        sqlx::query("UPDATE deployment_mcp_imports SET import_config=$1 WHERE environment_id=$2 AND deployment_revision_id=1 AND import_index=0")
            .bind(Blob::new(import))
            .bind(fixture.source.environment_id.0),
    )
    .await
    .unwrap();
    drop(db);

    let account = AccountId::new();
    let email = AccountEmail::new("unauthorized@test.invalid");
    let recipient = RecipientPattern::Account {
        account: email.clone(),
    };
    let unauthorized = AuthCtx::agent_with_effective_surface(
        account,
        email,
        EffectiveSurface::from_grants(&[], &[], &[], &[], &recipient).unwrap(),
    );

    assert!(matches!(
        fixture
            .resolver(Default::default())
            .inspect(fixture.source.clone(), unauthorized, false)
            .await,
        Err(McpImportResolverError::OAuth(error))
            if matches!(error.as_ref(), McpOAuthError::ImportNotFound)
    ));
    assert!(matches!(
        fixture.resolver(Default::default()).inspect(fixture.source.clone(), AuthCtx::System, false).await,
        Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::SchemeNotFound)
    ));
    assert_eq!(upstream.count(), 0);
}

#[test]
#[timeout("30s")]
async fn preview_http_route_authenticates_and_preserves_indexed_errors_and_public_shape() {
    use poem::http::StatusCode;
    use poem::test::TestClient;

    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let api = TestClient::new(crate::api::make_open_api_service(&fixture.services));
    let path = format!(
        "/v1/envs/{}/mcp-imports/resolve",
        fixture.source.environment_id
    );
    let config = RegistryServiceConfig::default();
    let bearer = format!(
        "Bearer {}",
        config.initial_accounts["root"]
            .token
            .as_ref()
            .unwrap()
            .secret()
    );
    let request = json!({"imports":[declaration(&upstream.url)],"nativeToolNames":[]});
    api.post(&path)
        .body_json(&request)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    api.post(path.replace(
        &fixture.source.environment_id.to_string(),
        &uuid::Uuid::new_v4().to_string(),
    ))
    .header("Authorization", &bearer)
    .body_json(&request)
    .send()
    .await
    .assert_status(StatusCode::NOT_FOUND);
    assert_eq!(upstream.count(), 0);
    upstream.reply(200, json!({"tools":[tool("First_Name")]}));
    let response = api
        .post(&path)
        .header("Authorization", &bearer)
        .body_json(&request)
        .send()
        .await;
    response.assert_status_is_ok();
    let body: Value = response.0.into_body().into_json().await.unwrap();
    assert_eq!(body.as_object().unwrap().len(), 3);
    assert_eq!(body["protocolVersions"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["importIndex"], 0);
    assert_eq!(body["tools"][0]["upstreamName"], "First_Name");
    assert_eq!(body["tools"][0].as_object().unwrap().len(), 4);
    let definition: golem_common::schema::tool::Tool =
        serde_json::from_value(body["tools"][0]["definition"].clone()).unwrap();
    assert_eq!(definition.name().unwrap(), "first-name");
    assert_eq!(body["diagnostics"], json!([]));
    upstream.reply(200, json!({"tools":[]}));
    let invalid = McpImportDeployment {
        url: "invalid".into(),
        ..declaration(&upstream.url)
    };
    let response = api
        .post(&path)
        .header("Authorization", &bearer)
        .body_json(&json!({"imports":[declaration(&upstream.url),invalid],"nativeToolNames":[]}))
        .send()
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
    let body: Value = response.0.into_body().into_json().await.unwrap();
    assert!(
        body["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("MCP import 1: ")
    );
    assert_eq!(
        body["code"],
        golem_common::model::api::error_code::INVALID_OAUTH_SESSION
    );
}

#[test]
#[timeout("30s")]
async fn preview_http_route_rejects_oversized_duplicate_native_names() {
    use poem::http::StatusCode;
    use poem::test::TestClient;

    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let api = TestClient::new(crate::api::make_open_api_service(&fixture.services));
    let path = format!(
        "/v1/envs/{}/mcp-imports/resolve",
        fixture.source.environment_id
    );
    let config = RegistryServiceConfig::default();
    let bearer = format!(
        "Bearer {}",
        config.initial_accounts["root"]
            .token
            .as_ref()
            .unwrap()
            .secret()
    );
    for duplicate_names in [
        vec!["native".to_string(); 12_000],
        vec!["native".repeat(16_384); 96],
    ] {
        api.post(&path)
            .header("Authorization", &bearer)
            .body_json(&json!({"imports":[], "nativeToolNames":duplicate_names}))
            .send()
            .await
            .assert_status(StatusCode::BAD_REQUEST);
    }
    api.post(&path)
        .header("Authorization", &bearer)
        .body_json(&json!({"imports":[], "nativeToolNames":["native","native"]}))
        .send()
        .await
        .assert_status_is_ok();
    assert_eq!(upstream.count(), 0);
}

#[test]
#[timeout("30s")]
async fn operator_tools_routes_pin_import_refresh_and_expose_only_public_metadata() {
    use poem::http::StatusCode;
    use poem::test::TestClient;

    let upstream = Upstream::new().await;
    let fixture = Fixture::with_http_limit(&upstream, 3).await;
    fixture
        .import(2, 1, &upstream.url, Some("private-import-token"))
        .await;
    let api = TestClient::new(crate::api::make_open_api_service(&fixture.services));
    let path = format!(
        "/v1/envs/{}/deployments/2/mcp-imports/1",
        fixture.source.environment_id
    );
    let config = RegistryServiceConfig::default();
    let bearer = format!(
        "Bearer {}",
        config.initial_accounts["root"]
            .token
            .as_ref()
            .unwrap()
            .secret()
    );
    api.get(format!("{path}/tools"))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    api.post(format!("{path}/refresh"))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    api.get(format!(
        "{}/tools",
        path.replace("/deployments/2/", "/deployments/1/")
    ))
    .header("Authorization", &bearer)
    .send()
    .await
    .assert_status(StatusCode::NOT_FOUND);
    assert_eq!(upstream.count(), 0);

    upstream.reply(200, json!({"tools":[tool("Original_Name"), {"name":"invalid", "inputSchema":{"type":"string"}}]}));
    let response = api
        .get(format!("{path}/tools"))
        .header("Authorization", &bearer)
        .send()
        .await;
    response.assert_status_is_ok();
    let body: Value = response.0.into_body().into_json().await.unwrap();
    assert_eq!(
        body["environmentId"],
        fixture.source.environment_id.to_string()
    );
    assert_eq!(body["deploymentRevision"], 2);
    assert_eq!(body["importIndex"], 1);
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["upstreamName"], "Original_Name");
    assert_eq!(body["tools"][0].as_object().unwrap().len(), 3);
    assert!(body["tools"][0]["definition"].is_object());
    assert!(!body["tools"][0]["digest"].as_str().unwrap().is_empty());
    assert_eq!(body["diagnostics"][0]["upstreamName"], "invalid");
    assert!(!body.to_string().contains("private-import-token"));
    assert_eq!(
        upstream.authorizations.lock().unwrap().as_slice(),
        &[Some("Bearer private-import-token".into())]
    );
    api.get(format!("{path}/tools"))
        .header("Authorization", &bearer)
        .send()
        .await
        .assert_json(body)
        .await;
    assert_eq!(upstream.count(), 1);

    upstream.reply(200, json!({"tools":[]}));
    let refreshed = api
        .post(format!("{path}/refresh"))
        .header("Authorization", &bearer)
        .send()
        .await;
    refreshed.assert_status_is_ok();
    let body: Value = refreshed.0.into_body().into_json().await.unwrap();
    assert_eq!(body["tools"], json!([]));
    assert_eq!(body["diagnostics"], json!([]));
    upstream.reply(503, json!({}));
    let failed = api
        .post(format!("{path}/refresh"))
        .header("Authorization", &bearer)
        .send()
        .await;
    failed.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
    let error: Value = failed.0.into_body().into_json().await.unwrap();
    assert_eq!(error["code"], "MCP_IMPORT_UPSTREAM_UNAVAILABLE");
    api.get(format!("{path}/tools"))
        .header("Authorization", &bearer)
        .send()
        .await
        .assert_json(body)
        .await;
    api.post(format!("{path}/refresh"))
        .header("Authorization", &bearer)
        .send()
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(upstream.count(), 3);
}

#[test]
#[timeout("30s")]
async fn operator_oauth_inspection_and_runtime_views_are_isolated_and_revocation_is_checked() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("runtime")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["runtime"]
    );
    upstream.reply(200, json!({"tools":[tool("operator")]}));
    assert_eq!(
        names(
            &resolver
                .inspect(fixture.source.clone(), AuthCtx::System, false)
                .await
                .unwrap()
        ),
        ["operator"]
    );
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["runtime"]
    );
    assert_eq!(
        names(
            &resolver
                .inspect(fixture.source.clone(), AuthCtx::System, false)
                .await
                .unwrap()
        ),
        ["operator"]
    );
    assert_eq!(upstream.count(), 2);
    assert_eq!(
        upstream.authorizations.lock().unwrap().as_slice(),
        &[Some("Bearer issued".into()), Some("Bearer issued".into())]
    );
    fixture
        .services
        .mcp_oauth_service
        .disconnect(&fixture.source, &AuthCtx::System)
        .await
        .unwrap();
    assert!(matches!(
        resolver.inspect(fixture.source.clone(), AuthCtx::System, false).await,
        Err(McpImportResolverError::OAuth(error)) if matches!(error.as_ref(), McpOAuthError::AuthorizationRequired(_))
    ));
    assert_eq!(
        upstream.count(),
        2,
        "revoked grant cannot serve cached metadata or dispatch"
    );
}

#[test]
#[timeout("30s")]
async fn periodic_refresh_replaces_removes_and_retains_last_success_on_failure() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let config = McpImportResolverConfig {
        refresh_interval: Duration::from_secs(1),
        cache_ttl: Duration::from_secs(60),
        ..Default::default()
    };
    let resolver = Arc::new(fixture.resolver(config));
    let mut tasks = JoinSet::new();
    McpImportResolver::start_background_tasks(&resolver, &mut tasks);

    upstream.reply(200, json!({"tools":[tool("initial")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["initial"]
    );

    upstream.reply(200, json!({"tools":[tool("changed")]}));
    wait_for_requests(&upstream, 2).await;
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["changed"]
    );

    upstream.reply(503, json!({}));
    wait_for_requests(&upstream, 3).await;
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["changed"]
    );

    upstream.reply(200, json!({"tools":[]}));
    wait_for_requests(&upstream, 4).await;
    assert!(
        resolver
            .resolve_observation(fixture.source.clone(), fixture.auth.clone())
            .await
            .unwrap()
            .tools
            .is_empty()
    );
}

#[test]
#[timeout("30s")]
async fn active_refresh_does_not_revive_evicted_historical_views() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture.import(2, 0, &upstream.url, None).await;
    let resolver = fixture.resolver(McpImportResolverConfig {
        cache_entries: 1,
        ..Default::default()
    });
    resolver.refresh_active_entries().await;
    assert_eq!(upstream.count(), 0, "cold views are fetched only on demand");

    upstream.reply(200, json!({"tools":[tool("historical")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    upstream.reply(200, json!({"tools":[tool("historical-updated")]}));
    resolver.refresh_active_entries().await;
    assert_eq!(upstream.count(), 2);
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["historical-updated"]
    );

    let later = McpImportSource {
        deployment_revision: 2_u64.try_into().unwrap(),
        ..fixture.source.clone()
    };
    upstream.reply(200, json!({"tools":[tool("later")]}));
    resolver
        .resolve_observation(later.clone(), fixture.auth.clone())
        .await
        .unwrap();
    upstream.reply(200, json!({"tools":[tool("later-updated")]}));
    resolver.refresh_active_entries().await;
    assert_eq!(
        upstream.count(),
        4,
        "evicted historical view must not refresh"
    );
    let cached = resolver.inner.cache.lock().unwrap();
    assert_eq!(cached.entries.len(), 1);
    assert_eq!(cached.entries.keys().next().unwrap().deployment_revision, 2);
}

#[test]
#[timeout("30s")]
async fn stale_periodic_scan_does_not_revive_view_evicted_by_foreground_demand() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture.import(2, 0, &upstream.url, None).await;
    let resolver = fixture.resolver(McpImportResolverConfig {
        cache_entries: 1,
        ..Default::default()
    });

    upstream.reply(200, json!({"tools":[tool("historical")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    let (original_key, inherited_last_used) = {
        let cache = resolver.inner.cache.lock().unwrap();
        let (key, entry) = cache.entries.iter().next().unwrap();
        (key.clone(), entry.last_used)
    };

    let later = McpImportSource {
        deployment_revision: 2_u64.try_into().unwrap(),
        ..fixture.source.clone()
    };
    upstream.reply(200, json!({"tools":[tool("later")]}));
    resolver
        .resolve_observation(later, fixture.auth.clone())
        .await
        .unwrap();

    let attempts = upstream.count();
    let result = resolver
        .resolve(
            fixture.source.clone(),
            McpImportAuth::Runtime(fixture.auth.clone()),
            true,
            false,
            Some((original_key, inherited_last_used)),
            tokio::time::Instant::now() + resolver.inner.config.operation_timeout,
        )
        .await;
    assert!(result.is_err());
    assert_eq!(upstream.count(), attempts, "obsolete work must not fetch");

    let cache = resolver.inner.cache.lock().unwrap();
    assert_eq!(cache.entries.len(), 1);
    assert_eq!(
        cache.entries.keys().next().unwrap().deployment_revision,
        2,
        "stale periodic work must not revive an evicted historical view"
    );
    assert_eq!(
        names(
            cache
                .entries
                .values()
                .next()
                .unwrap()
                .success
                .as_ref()
                .unwrap()
        ),
        ["later"]
    );
}

#[test]
#[timeout("30s")]
async fn active_refresh_rechecks_revoked_oauth_before_using_cached_metadata() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("private")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    let before = upstream.count();
    fixture
        .services
        .mcp_oauth_service
        .disconnect(&fixture.source, &AuthCtx::System)
        .await
        .unwrap();
    resolver.refresh_active_entries().await;
    assert_eq!(
        upstream.count(),
        before,
        "revoked grant must not reach upstream"
    );
    assert!(resolver.inner.cache.lock().unwrap().entries.is_empty());
    assert!(
        resolver
            .resolve_observation(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), before);
}

#[test]
#[timeout("30s")]
async fn active_refresh_stops_after_recent_demand_window() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(McpImportResolverConfig {
        cache_ttl: Duration::from_millis(500),
        ..Default::default()
    });
    upstream.reply(200, json!({"tools":[tool("initial")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    upstream.reply(200, json!({"tools":[tool("refreshed")]}));
    resolver.refresh_active_entries().await;
    assert_eq!(upstream.count(), 2);

    tokio::time::sleep(Duration::from_millis(450)).await;
    resolver.refresh_active_entries().await;
    assert_eq!(
        upstream.count(),
        2,
        "background refresh completion must not renew demand activity"
    );
}

#[test]
#[timeout("30s")]
async fn background_replacement_preserves_foreground_demand_after_scan() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let config = McpImportResolverConfig::default();
    let resolver = fixture.resolver(config);
    upstream.reply(200, json!({"tools":[tool("initial")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();

    let inherited_last_used = Instant::now() - Duration::from_secs(10);
    let foreground_last_used = Instant::now();
    {
        let mut cache = resolver.inner.cache.lock().unwrap();
        let entry = cache.entries.values_mut().next().unwrap();
        entry.last_used = foreground_last_used;
    }
    let context = fixture
        .services
        .mcp_oauth_service
        .import_context(
            &fixture.source,
            McpImportAuth::Runtime(fixture.auth.clone()),
            config.transport,
        )
        .await
        .unwrap();
    let key = CacheKey {
        environment_id: fixture.source.environment_id,
        deployment_revision: fixture.source.deployment_revision.into(),
        import_index: fixture.source.import_index,
        auth: McpImportAuth::Runtime(fixture.auth.clone()),
        credential: context.identity.clone(),
    };
    let resume = upstream.blocked(json!({"tools":[tool("background")]}));
    let _ = resolver.receiver_or_start(
        key.clone(),
        fixture.source.clone(),
        context,
        true,
        false,
        Some((key.clone(), inherited_last_used)),
        tokio::time::Instant::now() + config.operation_timeout,
    );
    assert_eq!(
        resolver
            .inner
            .cache
            .lock()
            .unwrap()
            .entries
            .get(&key)
            .unwrap()
            .last_used,
        foreground_last_used
    );
    resume.notify_one();
}

#[test]
#[timeout("30s")]
async fn empty_misses_refresh_replacement_and_failed_page_are_not_conflated() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[]}));
    for _ in 0..2 {
        assert!(
            resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
                .tools
                .is_empty()
        );
    }
    assert_eq!(upstream.count(), 1);
    upstream.reply(200, json!({"tools":[tool("First_Name")]}));
    assert_eq!(
        names(
            &resolver
                .refresh(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["first-name"]
    );
    upstream.reply(
        200,
        json!({"tools":[tool("partial")], "nextCursor":"second"}),
    );
    upstream.reply(503, json!({}));
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["first-name"]
    );
    assert_eq!(upstream.count(), 4);
    upstream.reply(
        200,
        json!({"error":{"code":-32603,"message":"upstream internal error"}}),
    );
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["first-name"]
    );
    assert_eq!(upstream.count(), 5);
    upstream.reply(
        200,
        json!({"tools":[{"name":"First_Name", "inputSchema":{"type":"array"}}, tool("New")]}),
    );
    let replaced = resolver
        .refresh(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    assert_eq!(names(&replaced), ["new"]);
    assert_eq!(replaced.diagnostics.len(), 1);
    upstream.reply(200, json!({"tools":[]}));
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .unwrap()
            .tools
            .is_empty()
    );
}

#[test]
#[timeout("30s")]
async fn concurrent_cold_misses_coalesce_and_caller_cancellation_does_not_strand_fill() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = Arc::new(fixture.resolver(Default::default()));
    let resume = upstream.blocked(json!({"tools":[tool("shared")]}));
    let owner = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    owner.abort();
    let mut waiters = JoinSet::new();
    for _ in 0..8 {
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        waiters.spawn(async move { resolver.resolve_observation(source, auth).await });
    }
    resume.notify_one();
    while let Some(result) = waiters.join_next().await {
        assert_eq!(names(&result.unwrap().unwrap()), ["shared"]);
    }
    assert_eq!(upstream.count(), 1);
    upstream.reply(503, json!({}));
    let mut other = fixture.source.clone();
    other.deployment_revision = 2_i64.try_into().unwrap();
    fixture.import(2, 0, &upstream.url, None).await;
    for _ in 0..2 {
        assert!(
            resolver
                .resolve_observation(other.clone(), fixture.auth.clone())
                .await
                .is_err()
        );
    }
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn bridge_feedback_invalidates_inline_cache_after_authorization() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture
        .pool
        .with_rw("mcp-resolver-test", "replace-inline")
        .execute(
            sqlx::query("DELETE FROM deployment_mcp_imports WHERE environment_id=$1")
                .bind(fixture.source.environment_id.0),
        )
        .await
        .unwrap();
    fixture
        .import(1, 0, &upstream.url, Some("inline-token"))
        .await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("cached")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();

    let mut invocation_source = fixture.source.clone();
    invocation_source.upstream_tool_name = "invoked-name".into();
    resolver
        .report_resource_unauthorized(&invocation_source, fixture.auth.clone(), None)
        .await
        .unwrap();
    upstream.reply(200, json!({"tools":[tool("refetched")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["refetched"]
    );
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn denied_bridge_feedback_does_not_invalidate_cache() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("cached")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    assert!(
        resolver
            .report_resource_unauthorized(&fixture.source, AuthCtx::System, None)
            .await
            .is_err()
    );
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["cached"]
    );
    assert_eq!(upstream.count(), 1);
}

#[test]
#[timeout("30s")]
async fn feedback_during_pending_fill_cannot_overwrite_replacement() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = Arc::new(fixture.resolver(Default::default()));
    let resume = upstream.blocked(json!({"tools":[tool("stale")]}));
    let first = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    resolver
        .report_resource_unauthorized(&fixture.source, fixture.auth.clone(), None)
        .await
        .unwrap();
    upstream.reply(200, json!({"tools":[tool("replacement")]}));
    let replacement = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    assert_eq!(names(&replacement.await.unwrap().unwrap()), ["replacement"]);
    assert!(!first.is_finished());
    resume.notify_one();
    assert_eq!(names(&first.await.unwrap().unwrap()), ["stale"]);
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["replacement"]
    );
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn manual_refresh_does_not_join_invalidated_pending_fill() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = Arc::new(fixture.resolver(Default::default()));
    let resume = upstream.blocked(json!({"tools":[tool("stale")]}));
    let first = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    resolver
        .report_resource_unauthorized(&fixture.source, fixture.auth.clone(), None)
        .await
        .unwrap();

    upstream.reply(200, json!({"tools":[tool("refreshed")]}));
    let refresh = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.refresh(source, auth).await }
    });
    upstream.entered.notified().await;
    assert_eq!(names(&refresh.await.unwrap().unwrap()), ["refreshed"]);
    assert!(!first.is_finished());
    resume.notify_one();
    assert_eq!(names(&first.await.unwrap().unwrap()), ["stale"]);
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["refreshed"]
    );
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn invalidated_pending_fill_can_be_replaced_when_cache_is_at_capacity() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = Arc::new(fixture.resolver(McpImportResolverConfig {
        cache_entries: 1,
        ..Default::default()
    }));
    let resume = upstream.blocked(json!({"tools":[tool("stale")]}));
    let first = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    resolver
        .report_resource_unauthorized(&fixture.source, fixture.auth.clone(), None)
        .await
        .unwrap();

    upstream.reply(200, json!({"tools":[tool("refreshed")]}));
    let refreshed = resolver
        .refresh(fixture.source.clone(), fixture.auth.clone())
        .await;
    assert_eq!(names(&refreshed.unwrap()), ["refreshed"]);

    resume.notify_one();
    first.await.unwrap().unwrap();
}

#[test]
#[timeout("30s")]
async fn registry_rpc_resolves_cold_cached_and_refreshed_observations_with_typed_errors() {
    let upstream = Upstream::new().await;
    let mut fixture = Fixture::with_http_limit(&upstream, 2).await;
    let client = fixture.client().await;
    upstream.reply(200, json!({"tools":[tool("First_Name")]}));
    for _ in 0..2 {
        let observation = client
            .resolve_mcp_import(&fixture.source, &fixture.auth, false)
            .await
            .unwrap();
        assert_eq!(names(&observation), ["first-name"]);
        assert_eq!(observation.source, fixture.source);
    }
    assert!(matches!(
        client
            .resolve_mcp_import(&fixture.source, &AuthCtx::System, false)
            .await,
        Err(RegistryServiceError::Unauthorized(_))
    ));
    let missing = McpImportSource {
        import_index: 3,
        ..fixture.source.clone()
    };
    assert!(matches!(
        client
            .resolve_mcp_import(&missing, &fixture.auth, false)
            .await,
        Err(RegistryServiceError::NotFound(_))
    ));
    assert_eq!(upstream.count(), 1);
    upstream.reply(200, json!({"tools":[]}));
    assert!(
        client
            .resolve_mcp_import(&fixture.source, &fixture.auth, true)
            .await
            .unwrap()
            .tools
            .is_empty()
    );
    assert!(matches!(
        client
            .resolve_mcp_import(&fixture.source, &fixture.auth, true)
            .await,
        Err(RegistryServiceError::LimitExceeded(_))
    ));
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn registry_rpc_preserves_supported_deep_projection() {
    let upstream = Upstream::new().await;
    let mut fixture = Fixture::new(&upstream).await;
    let client = fixture.client().await;
    let mut schema = json!({"type":"string"});
    for _ in 0..60 {
        schema = json!({"type":"object", "properties":{"nested":schema}, "required":["nested"], "additionalProperties":false});
    }
    upstream.reply(
        200,
        json!({"tools":[{"name":"deep", "inputSchema":schema}]}),
    );
    let observation = client
        .resolve_mcp_import(&fixture.source, &fixture.auth, false)
        .await
        .unwrap();
    assert_eq!(names(&observation), ["deep"]);
    assert!(observation.diagnostics.is_empty());
    assert_eq!(upstream.count(), 1);
}

#[test]
#[timeout("30s")]
async fn native_wins_without_fetch_and_distinct_effective_surfaces_do_not_share_results() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    let mut state = fixture.state().await;
    let projected = ProjectedTool::new(&tool("fixed"), "fixed", Default::default()).unwrap();
    let AuthCtx::Agent(agent) = &fixture.auth else {
        unreachable!()
    };
    let name = ToolName::try_from("fixed".to_owned()).unwrap();
    let registered = RegisteredTool {
        deployment_revision: state.deployment_revision,
        release_id: None,
        definition: serde_json::from_value(serde_json::to_value(projected.definition).unwrap())
            .unwrap(),
        provision: Default::default(),
        source: golem_common::model::tool::ToolSource::Host {
            host_tool_id: "fixture".to_owned().try_into().unwrap(),
            implementation_version: "1".into(),
        },
        owner_account_id: agent.account_id,
        owner_account_email: agent.account_email.clone(),
        metadata_version: "1".into(),
        metadata_digest: golem_common::model::diff::Hash::empty(),
        component_bindings: Default::default(),
    };
    state
        .registered_tools
        .insert(name.clone(), registered.clone());
    let selected = resolver
        .lookup_tool(
            fixture.source.environment_id,
            &state,
            &name,
            fixture.auth.clone(),
        )
        .await
        .unwrap();
    assert!(matches!(selected, Some(ResolvedTool::Native(value)) if *value == registered));
    assert_eq!(upstream.count(), 0);
    upstream.reply(200, json!({"tools":[tool("fixed"), tool("narrow")]}));
    let all = resolver
        .list_tools(fixture.source.environment_id, &state, fixture.auth.clone())
        .await
        .unwrap();
    assert_eq!(all.imported.len(), 1);
    assert_eq!(all.imported[0].tool.definition.name(), Some("narrow"));
    assert_eq!(all.diagnostics.len(), 1);
    let recipient = RecipientPattern::Account {
        account: agent.account_email.clone(),
    };
    let permission = PermissionPattern::Network(ClassPermissionPattern {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        recipient: recipient.clone(),
        resource: NetworkResourcePattern::host_port("127.0.0.1", PortPattern::Any),
    });
    let broad = AuthCtx::agent_with_effective_surface(
        agent.account_id,
        agent.account_email.clone(),
        EffectiveSurface::from_grants(&[permission], &[], &[], &[], &recipient).unwrap(),
    );
    upstream.reply(200, json!({"tools":[tool("broad")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), broad)
                .await
                .unwrap()
        ),
        ["broad"]
    );
    assert_eq!(upstream.count(), 2);
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["fixed", "narrow"]
    );
}

#[test]
fn resolver_errors_preserve_quota_category_and_hide_internal_details() {
    use crate::services::account_usage::error::{AccountUsageError, LimitExceededError};
    let error = McpImportResolverError::from(McpOAuthError::AccountUsage(
        AccountUsageError::LimitExceeded(LimitExceededError {
            limit_name: "monthly_http_call_limit".into(),
            limit_value: 1,
            current_value: 2,
        }),
    ));
    assert!(
        matches!(&error, McpImportResolverError::OAuth(inner) if matches!(inner.as_ref(), McpOAuthError::AccountUsage(AccountUsageError::LimitExceeded(_))))
    );
    assert!(!error.allows_stale());
    let internal = McpImportResolverError::from(McpOAuthError::InternalError(anyhow::anyhow!(
        "private-upstream-token"
    )));
    assert!(!internal.to_string().contains("private-upstream-token"));
    assert!(internal.allows_stale());
    assert!(
        McpImportResolverError::from(McpOAuthError::AccountUsage(
            AccountUsageError::InternalError(anyhow::anyhow!("accounting unavailable"))
        ))
        .allows_stale()
    );
}

#[test]
#[timeout("30s")]
async fn denied_cache_access_and_resource_401_do_not_fall_back_to_success() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("old")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    assert!(
        resolver
            .resolve_observation(fixture.source.clone(), AuthCtx::System)
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), 1);
    upstream.reply(401, json!({}));
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert!(
        resolver
            .resolve_observation(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn oauth_generation_keys_cache_and_resource_401_refreshes_once_without_same_call_retry() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    {
        let forms = upstream.token_forms.lock().unwrap();
        let initial_form = &forms[0];
        assert_eq!(
            initial_form.get("grant_type").unwrap(),
            "authorization_code"
        );
        assert_eq!(initial_form.get("code").unwrap(), "code");
    }
    let resolver = fixture.resolver(Default::default());
    let issued_generation = fixture.oauth_generation().await;

    upstream.reply(200, json!({"tools":[tool("issued")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["issued"]
    );
    assert_eq!(upstream.count(), 1);
    assert_eq!(
        upstream.authorizations.lock().unwrap().as_slice(),
        [Some("Bearer issued".into())]
    );

    // A warm metadata hit needs neither the resource nor the provider.
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["issued"]
    );
    assert_eq!(upstream.count(), 1);
    assert_eq!(upstream.refresh_count(), 0);

    upstream.reply(401, json!({}));
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), 2, "the rejected operation is not retried");
    assert_eq!(upstream.refresh_count(), 0);
    let expired_generation = fixture.oauth_generation().await;
    assert_ne!(expired_generation, issued_generation);

    upstream.reply(200, json!({"tools":[tool("rotated")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["rotated"]
    );
    assert_eq!(upstream.refresh_count(), 1);
    {
        let forms = upstream.token_forms.lock().unwrap();
        assert_eq!(forms.len(), 2);
        assert_eq!(forms[1].get("grant_type").unwrap(), "refresh_token");
        assert_eq!(forms[1].get("refresh_token").unwrap(), "initial-refresh");
    }
    assert_eq!(upstream.count(), 3);
    assert_eq!(
        upstream.authorizations.lock().unwrap().last().unwrap(),
        &Some("Bearer rotated".into())
    );
    let rotated_generation = fixture.oauth_generation().await;
    assert_ne!(rotated_generation, expired_generation);

    resolver
        .report_resource_unauthorized(
            &fixture.source,
            fixture.auth.clone(),
            Some(issued_generation),
        )
        .await
        .unwrap();
    assert_eq!(fixture.oauth_generation().await, rotated_generation);
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["rotated"]
    );
    assert_eq!(upstream.refresh_count(), 1);
    assert_eq!(upstream.count(), 3);
}

#[test]
#[timeout("30s")]
async fn active_refresh_keeps_current_oauth_generation_with_failed_old_sibling() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    let resolver = fixture.resolver(Default::default());

    upstream.reply(200, json!({"tools":[tool("issued")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    upstream.reply(401, json!({}));
    assert!(
        resolver
            .refresh(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    upstream.reply(200, json!({"tools":[tool("rotated")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();

    assert_eq!(resolver.inner.cache.lock().unwrap().entries.len(), 2);
    upstream.reply(200, json!({"tools":[tool("periodic")]}));
    resolver.refresh_active_entries().await;
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["periodic"]
    );
    assert_eq!(
        upstream.count(),
        4,
        "the current successful generation must remain cached"
    );
}

#[test]
#[timeout("30s")]
async fn active_refresh_rotates_oauth_generation_and_removes_successful_old_sibling() {
    let upstream = Upstream::with_oauth(true).await;
    let fixture = Fixture::new(&upstream).await;
    fixture.oauth_import_and_grant(&upstream).await;
    let resolver = fixture.resolver(Default::default());

    upstream.reply(200, json!({"tools":[tool("generation-one")]}));
    resolver
        .resolve_observation(fixture.source.clone(), fixture.auth.clone())
        .await
        .unwrap();
    let generation_one = fixture.oauth_generation().await;
    fixture
        .services
        .mcp_oauth_service
        .report_resource_unauthorized(&fixture.source, fixture.auth.clone(), Some(generation_one))
        .await
        .unwrap();

    upstream.reply(200, json!({"tools":[tool("generation-two")]}));
    resolver.refresh_active_entries().await;
    let generation_two = fixture.oauth_generation().await;
    assert_ne!(generation_two, generation_one);
    let cache = resolver.inner.cache.lock().unwrap();
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.entries.keys().all(|key| {
        matches!(key.credential, McpCredentialIdentity::OAuth { generation, .. } if generation == generation_two)
    }));
    assert_eq!(
        names(
            cache
                .entries
                .values()
                .next()
                .unwrap()
                .success
                .as_ref()
                .unwrap()
        ),
        ["generation-two"]
    );
}

#[test]
#[timeout("30s")]
async fn lookup_stops_at_first_winner_and_does_not_promote_past_unavailable_import() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture
        .import(1, 1, &upstream.url, Some("second-context"))
        .await;
    let state = fixture.state().await;
    let resolver = fixture.resolver(Default::default());
    upstream.reply(200, json!({"tools":[tool("winner")]}));
    let selected = resolver
        .lookup_tool(
            fixture.source.environment_id,
            &state,
            &ToolName::try_from("winner".to_owned()).unwrap(),
            fixture.auth.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(selected, ResolvedTool::Imported(tool) if tool.source.import_index == 0));
    assert_eq!(upstream.count(), 1);
    let cold = fixture.resolver(Default::default());
    upstream.reply(503, json!({}));
    assert!(
        cold.lookup_tool(
            fixture.source.environment_id,
            &state,
            &ToolName::try_from("winner".to_owned()).unwrap(),
            fixture.auth.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(upstream.count(), 2);
    upstream.reply(200, json!({"tools":[tool("winner"), tool("only-second")]}));
    let all = resolver
        .list_tools(fixture.source.environment_id, &state, fixture.auth.clone())
        .await
        .unwrap();
    assert_eq!(
        all.imported
            .iter()
            .map(|tool| (
                tool.source.import_index,
                tool.tool.definition.name().unwrap()
            ))
            .collect::<Vec<_>>(),
        [(0, "winner"), (1, "only-second")]
    );
    assert_eq!(all.diagnostics.len(), 1);
}

#[test]
#[timeout("30s")]
async fn saturated_cache_preserves_pending_owner_and_eviction_refetches() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture
        .import(1, 1, &upstream.url, Some("other-context"))
        .await;
    let resolver = Arc::new(fixture.resolver(McpImportResolverConfig {
        cache_entries: 1,
        ..Default::default()
    }));
    let resume = upstream.blocked(json!({"tools":[]}));
    let owner = tokio::spawn({
        let resolver = resolver.clone();
        let source = fixture.source.clone();
        let auth = fixture.auth.clone();
        async move { resolver.resolve_observation(source, auth).await }
    });
    upstream.entered.notified().await;
    let other = McpImportSource {
        import_index: 1,
        ..fixture.source.clone()
    };
    assert!(
        resolver
            .resolve_observation(other.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    assert_eq!(upstream.count(), 1);
    resume.notify_one();
    owner.await.unwrap().unwrap();
    upstream.reply(200, json!({"tools":[]}));
    resolver
        .resolve_observation(other, fixture.auth.clone())
        .await
        .unwrap();
    upstream.reply(200, json!({"tools":[tool("refetched")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["refetched"]
    );
    assert_eq!(upstream.count(), 3);
}

#[test]
#[timeout("30s")]
async fn detached_fill_obeys_operation_deadline_and_releases_capacity() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    let mut config = McpImportResolverConfig {
        operation_timeout: Duration::from_millis(250),
        failure_ttl: Duration::from_millis(10),
        fetch_concurrency: 1,
        ..Default::default()
    };
    config.transport.timeout = config.operation_timeout;
    let resolver = fixture.resolver(config);
    let _never_resume = upstream.blocked(json!({"tools":[]}));
    assert!(
        resolver
            .resolve_observation(fixture.source.clone(), fixture.auth.clone())
            .await
            .is_err()
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    upstream.reply(200, json!({"tools":[tool("recovered")]}));
    assert_eq!(
        names(
            &resolver
                .resolve_observation(fixture.source.clone(), fixture.auth.clone())
                .await
                .unwrap()
        ),
        ["recovered"]
    );
    assert_eq!(upstream.count(), 2);
}

#[test]
#[timeout("30s")]
async fn listing_waits_for_siblings_and_reports_first_failure_in_declaration_order() {
    let upstream = Upstream::new().await;
    let fixture = Fixture::new(&upstream).await;
    fixture
        .import(1, 1, "http://denied.invalid/mcp", None)
        .await;
    let resolver = fixture.resolver(Default::default());
    let state = fixture.state().await;
    let auth = fixture.auth.clone();
    let environment_id = fixture.source.environment_id;
    let resume = upstream.blocked(json!({"error":{"code":-32603,"message":"first import failed"}}));
    let listing =
        tokio::spawn(async move { resolver.list_tools(environment_id, &state, auth).await });
    tokio::time::timeout(Duration::from_secs(5), upstream.entered.notified())
        .await
        .unwrap();
    assert!(!listing.is_finished());
    resume.notify_one();
    assert!(matches!(
        listing.await.unwrap(),
        Err(McpImportResolverError::SourceUnavailable {
            import_index: 0,
            ..
        })
    ));
    assert_eq!(upstream.count(), 1);
}
