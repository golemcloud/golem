// The platform TLS verifier honours SSL_CERT_FILE on Linux.
#![cfg(target_os = "linux")]

use crate::app::{TestContext, cmd, flag};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use golem_cli::{fs, versions};
use indoc::{formatdoc, indoc};
use poem::listener::{Acceptor, Listener, RustlsCertificate, RustlsConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use url::Url;

#[derive(Default)]
struct Calls {
    paths: Vec<String>,
    mcp_authorizations: Vec<Option<String>>,
    token_forms: Vec<HashMap<String, String>>,
    token_authorizations: Vec<Option<String>>,
    tool_calls: usize,
    code_grants: usize,
    reject_initial_token: bool,
    rejected_calls: Vec<(Value, usize)>,
}

#[test]
#[timeout("15 minutes")]
async fn rust_mcp_oauth_consent_invokes_over_tls_and_replays_offline() {
    let tls = RustlsConfig::new().fallback(
        RustlsCertificate::new()
            .cert(include_bytes!(
                "../../../../golem-registry-service/src/services/mcp_import/tls/server-cert.pem"
            ))
            .key(include_bytes!(
                "../../../../golem-registry-service/src/services/mcp_import/tls/server-key.pem"
            )),
    );
    let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
        .rustls(tls)
        .into_acceptor()
        .await
        .unwrap();
    let port = acceptor.local_addr()[0].as_socket_addr().unwrap().port();
    let origin = format!("https://127.0.0.1:{port}");
    let calls = Arc::new(Mutex::new(Calls::default()));
    let handler = poem::endpoint::make({
        let calls = calls.clone();
        let origin = origin.clone();
        move |mut request: poem::Request| {
            let calls = calls.clone();
            let origin = origin.clone();
            async move {
                let path = request.uri().path().to_string();
                calls.lock().unwrap().paths.push(path.clone());
                match path.as_str() {
                    "/.well-known/oauth-protected-resource/mcp" => poem::Response::builder()
                        .header("content-type", "application/json")
                        .body(json!({"resource":format!("{origin}/mcp"),"authorization_servers":[origin]}).to_string()),
                    "/.well-known/oauth-authorization-server" => poem::Response::builder()
                        .header("content-type", "application/json")
                        .body(json!({"issuer":origin,"authorization_endpoint":format!("{origin}/authorize"),"token_endpoint":format!("{origin}/token"),"response_types_supported":["code"],"code_challenge_methods_supported":["S256"],"authorization_response_iss_parameter_supported":true}).to_string()),
                    "/token" => {
                        let authorization = request.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
                        let body = request.take_body().into_bytes().await.unwrap();
                        let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
                        let mut calls = calls.lock().unwrap();
                        let token = match form.get("grant_type").map(String::as_str) {
                            Some("authorization_code") => {
                                calls.code_grants += 1;
                                if calls.code_grants == 1 { "consented-token" } else { "reconsented-token" }
                            }
                            Some("refresh_token") => "rotated-token",
                            _ => return poem::Response::builder().status(poem::http::StatusCode::BAD_REQUEST).body(""),
                        };
                        calls.token_authorizations.push(authorization);
                        calls.token_forms.push(form);
                        poem::Response::builder().header("content-type", "application/json").body(
                            json!({"access_token":token,"refresh_token":"refresh-token","token_type":"Bearer","expires_in":3600}).to_string(),
                        )
                    }
                    "/mcp" => {
                        let authorization = request.headers().get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
                        let body: Value = request.take_body().into_json().await.unwrap();
                        let mut calls = calls.lock().unwrap();
                        calls.mcp_authorizations.push(authorization.clone());
                        if authorization.as_deref() == Some("Bearer consented-token") && calls.reject_initial_token && body["method"] == "tools/call" {
                            let token_requests = calls.token_forms.len();
                            calls.rejected_calls.push((body, token_requests));
                            return poem::Response::builder().status(poem::http::StatusCode::UNAUTHORIZED).body("");
                        }
                        if !matches!(authorization.as_deref(), Some("Bearer consented-token" | "Bearer rotated-token" | "Bearer reconsented-token")) {
                            return poem::Response::builder()
                                .status(poem::http::StatusCode::UNAUTHORIZED)
                                .header("www-authenticate", format!("Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource/mcp\""))
                                .body("");
                        }
                        let result = if body["method"] == "tools/list" {
                            json!({"tools":[{"name":"lookup","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false},"outputSchema":{"type":"object","properties":{"answer":{"type":"string"},"score":{"type":"integer"}},"required":["answer","score"],"additionalProperties":false}}]})
                        } else {
                            if body["method"] != "tools/call" || body["params"]["name"] != "lookup" || body["params"]["arguments"]["query"] != "asymmetric" {
                                return poem::Response::builder().status(poem::http::StatusCode::BAD_REQUEST).body("");
                            }
                            calls.tool_calls += 1;
                            let score = match authorization.as_deref() {
                                Some("Bearer rotated-token") => 31,
                                Some("Bearer reconsented-token") => 37,
                                _ => 29,
                            };
                            json!({"structuredContent":{"answer":"oauth-answer","score":score},"content":[{"type":"text","text":"oauth-stdout"}]})
                        };
                        poem::Response::builder().header("content-type", "application/json").body(json!({"jsonrpc":"2.0","id":body["id"],"result":result}).to_string())
                    }
                    _ => poem::Response::builder().status(poem::http::StatusCode::NOT_FOUND).body(""),
                }
            }
        }
    });
    let provider = tokio::spawn(async move {
        poem::Server::new_with_acceptor(acceptor)
            .run(handler)
            .await
            .unwrap();
    });

    let trust = tempfile::NamedTempFile::new().unwrap();
    let system_ca_path = std::env::var_os("SSL_CERT_FILE").unwrap_or_else(|| {
        [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/pki/tls/certs/ca-bundle.crt",
        ]
        .into_iter()
        .find(|path| std::path::Path::new(path).is_file())
        .expect("a system CA bundle or SSL_CERT_FILE is required")
        .into()
    });
    let system_ca = std::fs::read(system_ca_path).unwrap();
    let fixture_ca =
        include_bytes!("../../../../golem-registry-service/src/services/mcp_import/tls/ca.pem");
    std::fs::write(trust.path(), [system_ca.as_slice(), fixture_ca].concat()).unwrap();

    let mut ctx = TestContext::new();
    ctx.add_env_var("SSL_CERT_FILE", trust.path().to_str().unwrap());
    ctx.start_server().await;
    let created = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            "oauth-client",
            flag::TEMPLATE,
            "rust",
            flag::COMPONENT_NAME,
            "oauth-client:consumer",
        ])
        .await;
    assert!(created.success_or_dump());
    ctx.cd("oauth-client");
    let scheme = ctx
        .cli([
            "api",
            "security-scheme",
            "create",
            "test-oauth",
            "--provider-type",
            "custom",
            "--custom-provider-name",
            "Test OAuth",
            "--custom-issuer-url",
            &origin,
            "--client-id",
            "client",
            "--client-secret",
            "secret",
            "--scope",
            "tools",
            "--redirect-url",
            "https://callback.example/complete",
        ])
        .await;
    assert!(scheme.success_or_dump());
    fs::write_str(
        ctx.cwd_path_join("golem.yaml"),
        formatdoc! {r#"
        manifestVersion: {version}
        app: oauth-client
        environments:
          local:
            server: local
            componentPresets: debug
        components:
          oauth-client:consumer:
            dir: .
            templates: rust
            dependencies:
              tools: [oauth-lookup]
        mcp:
          imports:
            local:
              - url: {origin}/mcp
                securityScheme: test-oauth
                prefix: oauth
        bridge:
          rust:
            internal:
              tools: [oauth-lookup]
    "#, version = versions::sdk::MANIFEST},
    )
    .unwrap();
    let authorized = ctx
        .cli(["api", "mcp-import", "authorize", "0", "--manifest"])
        .await;
    assert!(authorized.success_or_dump());
    assert!(authorized.stdout_contains("Authorization URL"));
    assert!(authorized.stdout_contains("mcp-import complete 0 --manifest"));
    let authorization_url = authorized
        .stdout()
        .flat_map(|line| line.split_whitespace())
        .find(|part| part.starts_with("https://127.0.0.1:") && part.contains("/authorize?"))
        .unwrap();
    let authorization_url = Url::parse(authorization_url).unwrap();
    let query: HashMap<_, _> = authorization_url.query_pairs().into_owned().collect();
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], "client");
    assert_eq!(query["redirect_uri"], "https://callback.example/complete");
    assert_eq!(query["scope"], "tools");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["resource"], format!("{origin}/mcp"));
    let state = &query["state"];
    assert!(!state.is_empty());
    let callback = format!(
        "https://callback.example/complete?code=accepted&state={state}&iss={}",
        url::form_urlencoded::byte_serialize(origin.as_bytes()).collect::<String>()
    );
    let completed = ctx
        .cli([
            "api",
            "mcp-import",
            "complete",
            "0",
            "--manifest",
            &callback,
        ])
        .await;
    assert!(completed.success_or_dump());
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.token_forms.len(), 1);
        assert_eq!(
            calls.token_authorizations,
            [Some("Basic Y2xpZW50OnNlY3JldA==".into())]
        );
        let form = &calls.token_forms[0];
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "accepted");
        assert_eq!(form["redirect_uri"], "https://callback.example/complete");
        assert_eq!(form["resource"], format!("{origin}/mcp"));
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
            query["code_challenge"]
        );
        assert!(
            calls
                .paths
                .iter()
                .any(|path| path == "/.well-known/oauth-protected-resource/mcp")
        );
        assert!(
            calls
                .paths
                .iter()
                .any(|path| path == "/.well-known/oauth-authorization-server")
        );
    }

    fs::write_str(ctx.cwd_path_join("src/counter_agent.rs"), indoc! {r#"
        use oauth_lookup_tool_guest_client::OauthLookupClient;
        use golem_rust::{agent_definition, agent_implementation};
        #[agent_definition]
        pub trait OAuthConsumer { fn new(name: String) -> Self; async fn run(&mut self) -> String; fn status(&self) -> String; }
        struct Consumer { result: String }
        #[agent_implementation]
        impl OAuthConsumer for Consumer {
            fn new(_name: String) -> Self { Self { result: String::new() } }
            async fn run(&mut self) -> String {
                let outcome = match OauthLookupClient::new().oauth_lookup("asymmetric".into()).unwrap().collect().await {
                    Ok((result, stdout)) => format!("{}:{}:{}", result.structured.answer, result.structured.score, String::from_utf8(stdout).unwrap()),
                    Err(error) => format!("error:{error:?}"),
                };
                self.result.push_str(&outcome);
                self.result.push('|');
                outcome
            }
            fn status(&self) -> String { self.result.clone() }
        }
    "#}).unwrap();
    let manifest = ctx.cwd_path_join("Cargo.toml");
    let source = fs::read_to_string(&manifest).unwrap();
    fs::write_str(&manifest, source.replace("[dependencies]", "[dependencies]\noauth-lookup-tool-guest-client = { path = \"golem-temp/bridge-sdk/rust/internal/oauth-lookup-tool-guest-client\" }")).unwrap();
    assert!(ctx.cli([flag::YES, cmd::BUILD]).await.success_or_dump());
    let deployed = ctx.cli([flag::YES, cmd::DEPLOY]).await;
    assert!(deployed.success_or_dump());
    assert!(!deployed.stderr_contains("Deployment discovery unavailable"));
    let agent = "OAuthConsumer(\"replay\")";
    let invoked = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(invoked.success_or_dump());
    assert!(invoked.stdout_contains("oauth-answer:29:oauth-stdout"));
    {
        let mut calls = calls.lock().unwrap();
        assert_eq!(calls.tool_calls, 1);
        assert!(calls.mcp_authorizations.len() >= 3);
        assert_eq!(calls.mcp_authorizations[0], None);
        assert!(
            calls.mcp_authorizations[1..]
                .iter()
                .all(|auth| auth.as_deref() == Some("Bearer consented-token"))
        );
        assert_eq!(calls.token_forms.len(), 1);
        calls.reject_initial_token = true;
    }

    let rejected = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(rejected.success_or_dump());
    assert!(
        rejected.stdout_contains("Rpc(RemoteInternal("),
        "{}",
        rejected.stdout_text()
    );
    assert!(rejected.stdout_contains("MCP upstream authorization required (HTTP 401)"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.rejected_calls.len(), 1);
        assert_eq!(calls.rejected_calls[0].0["method"], "tools/call");
        assert_eq!(calls.tool_calls, 1);
        assert_eq!(
            calls.rejected_calls[0].1, 1,
            "the rejected call must use the original token"
        );
    }
    let refreshed = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(refreshed.success_or_dump());
    assert!(refreshed.stdout_contains("oauth-answer:31:oauth-stdout"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.rejected_calls.len(), 1);
        assert_eq!(calls.tool_calls, 2);
        assert_eq!(calls.token_forms.len(), 2);
        assert_eq!(
            calls.token_authorizations[1].as_deref(),
            Some("Basic Y2xpZW50OnNlY3JldA==")
        );
        assert_eq!(calls.token_forms[1]["grant_type"], "refresh_token");
        assert_eq!(calls.token_forms[1]["refresh_token"], "refresh-token");
        assert_eq!(calls.token_forms[1]["resource"], format!("{origin}/mcp"));
        assert!(
            calls
                .mcp_authorizations
                .iter()
                .any(|auth| auth.as_deref() == Some("Bearer rotated-token"))
        );
    }

    let disconnected = ctx
        .cli(["api", "mcp-import", "disconnect", "0", "--manifest"])
        .await;
    assert!(disconnected.success_or_dump());
    let (tool_calls_after_disconnect, token_exchanges_after_disconnect) = {
        let calls = calls.lock().unwrap();
        (calls.tool_calls, calls.token_forms.len())
    };
    let revoked = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(revoked.success_or_dump());
    assert!(
        revoked.stdout_contains("Rpc(RemoteInternal("),
        "{}",
        revoked.stdout_text()
    );
    assert!(
        revoked.stdout_contains("test-oauth"),
        "{}",
        revoked.stdout_text()
    );
    assert!(
        revoked.stdout_contains("Authorize the MCP import"),
        "{}",
        revoked.stdout_text()
    );
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.tool_calls, tool_calls_after_disconnect);
        assert_eq!(calls.token_forms.len(), token_exchanges_after_disconnect);
    }

    let reauthorized = ctx
        .cli(["api", "mcp-import", "authorize", "0", "--manifest"])
        .await;
    assert!(reauthorized.success_or_dump());
    let reauthorization_url = reauthorized
        .stdout()
        .flat_map(|line| line.split_whitespace())
        .find(|part| part.starts_with("https://127.0.0.1:") && part.contains("/authorize?"))
        .unwrap();
    let reauthorization_url = Url::parse(reauthorization_url).unwrap();
    let reauthorization_query: HashMap<_, _> =
        reauthorization_url.query_pairs().into_owned().collect();
    assert_eq!(reauthorization_query["code_challenge_method"], "S256");
    assert_ne!(reauthorization_query["state"], query["state"]);
    assert_ne!(
        reauthorization_query["code_challenge"],
        query["code_challenge"]
    );
    let reauthorization_callback = format!(
        "https://callback.example/complete?code=reaccepted&state={}&iss={}",
        reauthorization_query["state"],
        url::form_urlencoded::byte_serialize(origin.as_bytes()).collect::<String>()
    );
    let recompleted = ctx
        .cli([
            "api",
            "mcp-import",
            "complete",
            "0",
            "--manifest",
            &reauthorization_callback,
        ])
        .await;
    assert!(recompleted.success_or_dump());
    {
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.token_forms.len(),
            token_exchanges_after_disconnect + 1
        );
        let form = calls.token_forms.last().unwrap();
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "reaccepted");
        assert_eq!(
            URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes())),
            reauthorization_query["code_challenge"]
        );
    }
    let reinvoked = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "run"])
        .await;
    assert!(reinvoked.success_or_dump());
    assert!(reinvoked.stdout_contains("oauth-answer:37:oauth-stdout"));
    {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.tool_calls, tool_calls_after_disconnect + 1);
        assert_eq!(
            calls.token_forms.len(),
            token_exchanges_after_disconnect + 1
        );
    }

    provider.abort();
    let _ = provider.await;
    ctx.server_process.take().unwrap().kill().await.unwrap();
    ctx.startup_ports = None;
    ctx.start_server().await;
    let replayed = ctx
        .cli([flag::YES, cmd::AGENT, cmd::INVOKE, agent, "status"])
        .await;
    assert!(replayed.success_or_dump());
    assert!(replayed.stdout_contains("oauth-answer:29:oauth-stdout"));
    assert!(
        replayed.stdout_contains("Rpc(RemoteInternal("),
        "{}",
        replayed.stdout_text()
    );
    assert!(replayed.stdout_contains("MCP upstream authorization required (HTTP 401)"));
    assert!(replayed.stdout_contains("oauth-answer:31:oauth-stdout"));
    assert!(replayed.stdout_contains("test-oauth"));
    assert!(replayed.stdout_contains("Authorize the MCP import"));
    assert!(replayed.stdout_contains("oauth-answer:37:oauth-stdout"));
}
