use super::*;
use bytes::Bytes;
use http::{HeaderMap, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    time::Duration,
};
use test_r::test;

const RESOURCE: &str = "https://mcp.example/a/b";

fn metadata() -> Value {
    json!({
        "issuer":"https://issuer.example/tenant",
        "authorization_endpoint":"https://issuer.example/authorize?tenant=one",
        "token_endpoint":"https://issuer.example/token",
        "response_types_supported":["code"],
        "code_challenge_methods_supported":["S256"],
        "authorization_response_iss_parameter_supported":true
    })
}

fn validate(value: Value) -> Result<AuthorizationServer, TransportError> {
    AuthorizationServer::validate(
        serde_json::from_value(value).unwrap(),
        "https://issuer.example/tenant",
    )
}

#[test]
fn metadata_requires_exact_issuer_and_advertised_pkce() {
    assert!(validate(metadata()).is_ok());
    for (field, value) in [
        ("issuer", json!("https://ISSUER.example/tenant")),
        ("issuer", json!("https://issuer.example/tenant/")),
        ("issuer", json!("https://issuer.example:443/tenant")),
        ("code_challenge_methods_supported", Value::Null),
        ("code_challenge_methods_supported", json!(["plain"])),
        ("response_types_supported", json!(["token"])),
        (
            "authorization_endpoint",
            json!("http://issuer.example/authorize"),
        ),
        (
            "authorization_endpoint",
            json!("https://issuer.example/authorize?client_id=other"),
        ),
        (
            "token_endpoint",
            json!("https://user:password@issuer.example/token"),
        ),
    ] {
        let mut input = metadata();
        input[field] = value;
        assert!(validate(input).is_err(), "accepted {field}");
    }
}

#[test]
fn callback_issuer_uses_the_rfc9207_validation_matrix() {
    let expected = "https://issuer.example";
    for required in [false, true] {
        assert!(validate_callback_issuer(expected, required, Some(expected)).is_ok());
        for mismatch in [
            "https://issuer.example/",
            "https://ISSUER.example",
            "https://issuer.example:443",
            "https://attacker.example",
        ] {
            assert!(validate_callback_issuer(expected, required, Some(mismatch)).is_err());
        }
        assert_eq!(
            validate_callback_issuer(expected, required, None).is_ok(),
            !required
        );
    }
}

#[test]
fn authorization_requests_bind_resource_callback_scope_and_pkce() {
    let server = validate(metadata()).unwrap();
    let client = server
        .client(
            "registered-client",
            Some("do-not-print"),
            "https://golem.example/callback",
            RESOURCE,
        )
        .unwrap();
    let request = client
        .authorize(&["tools:read".into(), "files:write".into()])
        .unwrap();
    let params: BTreeMap<_, _> = request.url.query_pairs().into_owned().collect();
    assert_eq!(params["client_id"], "registered-client");
    assert_eq!(params["resource"], "https://mcp.example/a/b");
    assert_eq!(params["redirect_uri"], "https://golem.example/callback");
    assert_eq!(params["scope"], "tools:read files:write");
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["tenant"], "one");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["state"], *request.state.secret());
    assert_eq!(
        params["code_challenge"],
        *PkceCodeChallenge::from_code_verifier_sha256(&request.verifier).as_str()
    );
    assert!(!request.url.as_str().contains("do-not-print"));
    let second = client.authorize(&[]).unwrap();
    assert_ne!(request.state.secret(), second.state.secret());
    assert_ne!(request.verifier.secret(), second.verifier.secret());
}

#[test]
fn callback_and_client_authentication_constraints_are_enforced() {
    let server = validate(metadata()).unwrap();
    assert!(
        server
            .client(
                "client",
                Some("secret"),
                "http://localhost:8080/callback",
                RESOURCE
            )
            .is_ok()
    );
    assert!(
        server
            .client(
                "client",
                Some("secret"),
                "http://evil.example/callback",
                RESOURCE
            )
            .is_err()
    );
    assert!(
        server
            .client(
                "client",
                Some("secret"),
                "https://golem.example/callback#fragment",
                RESOURCE,
            )
            .is_err()
    );
    assert!(
        server
            .client("client", None, "https://golem.example/callback", RESOURCE)
            .is_err()
    );
    let mut input = metadata();
    input["token_endpoint_auth_methods_supported"] = json!(["private_key_jwt"]);
    assert!(
        validate(input)
            .unwrap()
            .client(
                "client",
                Some("secret"),
                "https://golem.example/callback",
                RESOURCE
            )
            .is_err()
    );
}

#[test]
fn discovery_url_priority_preserves_the_issuer_path() {
    assert_eq!(
        authorization_metadata_urls("https://issuer.example/tenant%2Fone")
            .unwrap()
            .iter()
            .map(Url::as_str)
            .collect::<Vec<_>>(),
        vec![
            "https://issuer.example/.well-known/oauth-authorization-server/tenant%2Fone",
            "https://issuer.example/.well-known/openid-configuration/tenant%2Fone",
            "https://issuer.example/tenant%2Fone/.well-known/openid-configuration",
        ]
    );
    assert_eq!(
        authorization_metadata_urls("https://issuer.example")
            .unwrap()
            .len(),
        2
    );
    assert!(authorization_metadata_urls("https://issuer.example?tenant=one").is_err());
}

#[test]
fn protected_resource_metadata_cannot_switch_the_configured_authorization_server() {
    let metadata = ProtectedResourceMetadata {
        resource: "https://mcp.example/path".into(),
        authorization_servers: vec![
            "https://other.example".into(),
            "https://issuer.example".into(),
        ],
        scopes_supported: None,
    };
    assert!(
        metadata
            .validate("https://mcp.example/path", "https://issuer.example")
            .is_ok()
    );
    assert!(
        metadata
            .validate("https://mcp.example/other", "https://issuer.example")
            .is_err()
    );
    assert!(
        metadata
            .validate("https://mcp.example/path", "https://issuer.example/")
            .is_err()
    );
}

type TestBody = UnsyncBoxBody<Bytes, std::convert::Infallible>;

#[derive(Default)]
struct Provider {
    requests: Vec<Request<Bytes>>,
    responses: VecDeque<Response<TestBody>>,
    failure: Option<TransportError>,
    delay: Duration,
}

impl HttpSend for Provider {
    type Body = TestBody;
    type Error = TransportError;

    async fn send(
        &mut self,
        request: Request<Bytes>,
    ) -> Result<Response<TestBody>, TransportError> {
        self.requests.push(request);
        if let Some(failure) = self.failure.take() {
            return Err(failure);
        }
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(self
            .responses
            .pop_front()
            .expect("unexpected extra request"))
    }
}

fn response(status: StatusCode, value: Value) -> Response<TestBody> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(value.to_string())).boxed_unsync())
        .unwrap()
}

fn resource_metadata() -> Value {
    json!({"resource":RESOURCE, "authorization_servers":["https://issuer.example/tenant"],
        "scopes_supported":["default:scope"]})
}

fn client() -> OAuthClient {
    validate(metadata())
        .unwrap()
        .client(
            "client",
            Some("secret"),
            "https://golem.example/callback",
            RESOURCE,
        )
        .unwrap()
}

fn tokens() -> Value {
    json!({"access_token":"access", "token_type":"Bearer", "refresh_token":"rotated",
        "expires_in":120, "scope":"tools:read"})
}

#[test]
async fn discovery_uses_bearer_challenge_not_other_schemes_or_quoted_substrings() {
    let mut headers = HeaderMap::new();
    headers.append(header::WWW_AUTHENTICATE, r#"Basic realm="resource_metadata=\"https://evil.example\"", resource_metadata="https://wrong.example""#.parse().unwrap());
    headers.append(header::WWW_AUTHENTICATE, r#"bEaReR ReSoUrCe_MeTaDaTa = "https://metadata.example/path?a=1,b=2", scope="files:write tools:read", Other realm="ignored""#.parse().unwrap());
    let mut provider = Provider {
        responses: VecDeque::from([
            response(StatusCode::OK, resource_metadata()),
            response(StatusCode::OK, metadata()),
        ]),
        ..Default::default()
    };
    let found = discover(
        &mut provider,
        RESOURCE,
        "https://issuer.example/tenant",
        &headers,
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(found.scopes, ["files:write", "tools:read"]);
    assert_eq!(found.server.issuer(), "https://issuer.example/tenant");
    assert_eq!(provider.requests.len(), 2);
    assert_eq!(
        provider.requests[0].uri().to_string(),
        "https://metadata.example/path?a=1,b=2"
    );
    assert_eq!(
        provider.requests[1].uri().to_string(),
        "https://issuer.example/.well-known/oauth-authorization-server/tenant"
    );
    for request in &provider.requests {
        assert_eq!(request.method(), http::Method::GET);
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        assert!(request.body().is_empty());
    }
}

#[test]
async fn discovery_fallback_order_and_invalid_metadata_are_distinct() {
    let mut provider = Provider {
        responses: VecDeque::from([
            response(StatusCode::NOT_FOUND, Value::Null),
            response(StatusCode::OK, resource_metadata()),
            response(StatusCode::NOT_FOUND, Value::Null),
            response(StatusCode::NOT_FOUND, Value::Null),
            response(StatusCode::OK, metadata()),
        ]),
        ..Default::default()
    };
    let result = discover(
        &mut provider,
        RESOURCE,
        "https://issuer.example/tenant",
        &HeaderMap::new(),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.scopes, ["default:scope"]);
    let paths: Vec<_> = provider.requests.iter().map(|r| r.uri().path()).collect();
    assert_eq!(
        paths,
        [
            "/.well-known/oauth-protected-resource/a/b",
            "/.well-known/oauth-protected-resource",
            "/.well-known/oauth-authorization-server/tenant",
            "/.well-known/openid-configuration/tenant",
            "/tenant/.well-known/openid-configuration"
        ]
    );
    for (field, value) in [
        ("resource", json!("https://other.example")),
        ("authorization_servers", json!(["https://other.example"])),
    ] {
        let mut resource = resource_metadata();
        resource[field] = value;
        let mut provider = Provider {
            responses: VecDeque::from([response(StatusCode::OK, resource)]),
            ..Default::default()
        };
        assert!(
            discover(
                &mut provider,
                RESOURCE,
                "https://issuer.example/tenant",
                &HeaderMap::new(),
                Limits::default()
            )
            .await
            .is_err()
        );
        assert_eq!(provider.requests.len(), 1);
    }
    let mut wrong_issuer = metadata();
    wrong_issuer["issuer"] = json!("https://ISSUER.example/tenant");
    let mut provider = Provider {
        responses: VecDeque::from([
            response(StatusCode::OK, resource_metadata()),
            response(StatusCode::OK, wrong_issuer),
        ]),
        ..Default::default()
    };
    assert!(
        discover(
            &mut provider,
            RESOURCE,
            "https://issuer.example/tenant",
            &HeaderMap::new(),
            Limits::default()
        )
        .await
        .is_err()
    );
    assert_eq!(provider.requests.len(), 2);
}

#[test]
async fn ambiguous_invalid_and_oversized_challenges_never_trigger_discovery() {
    for value in [
        r#"Bearer scope="a", Scope="b""#,
        r#"Bearer scope="a", Bearer scope="b""#,
        r#"Bearer resource_metadata="http://insecure.example""#,
        r#"Bearer resource_metadata="https://user:secret@host.example""#,
        r#"Bearer realm="unterminated"#,
    ] {
        let headers = HeaderMap::from_iter([(header::WWW_AUTHENTICATE, value.parse().unwrap())]);
        let mut provider = Provider::default();
        assert!(
            discover(
                &mut provider,
                RESOURCE,
                "https://issuer.example/tenant",
                &headers,
                Limits::default()
            )
            .await
            .is_err()
        );
        assert!(provider.requests.is_empty());
    }
    let headers = HeaderMap::from_iter([(header::WWW_AUTHENTICATE, "Bearer".parse().unwrap())]);
    let mut provider = Provider::default();
    assert!(matches!(
        discover(
            &mut provider,
            RESOURCE,
            "https://issuer.example/tenant",
            &headers,
            Limits {
                challenge_bytes: 5,
                ..Limits::default()
            }
        )
        .await,
        Err(TransportError::Limit(_))
    ));
    assert!(provider.requests.is_empty());
}

#[test]
async fn exchanges_send_resource_pkce_callback_and_rotating_tokens_once() {
    let client = client();
    let authorization = client.authorize(&[]).unwrap();
    let verifier = authorization.verifier.secret().clone();
    let mut provider = Provider {
        responses: VecDeque::from([
            response(StatusCode::OK, tokens()),
            response(StatusCode::OK, tokens()),
        ]),
        ..Default::default()
    };
    let result = client
        .exchange_code(
            &mut provider,
            AuthorizationCode::new("code+&= value".into()),
            authorization.verifier,
            Some("https://issuer.example/tenant"),
            Limits::default(),
        )
        .await
        .unwrap();
    assert_eq!(result.access_token().secret(), "access");
    assert_eq!(result.refresh_token().unwrap().secret(), "rotated");
    assert_eq!(result.expires_in(), Some(Duration::from_secs(120)));
    client
        .refresh(
            &mut provider,
            result.refresh_token().unwrap(),
            Limits::default(),
        )
        .await
        .unwrap();
    assert_eq!(provider.requests.len(), 2);
    for request in &provider.requests {
        assert_eq!(request.method(), http::Method::POST);
        assert_eq!(request.uri(), "https://issuer.example/token");
        assert_eq!(request.headers()[header::ACCEPT_ENCODING], "identity");
        assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Basic Y2xpZW50OnNlY3JldA=="
        );
        let params: Vec<_> = url::form_urlencoded::parse(request.body())
            .into_owned()
            .collect();
        assert_eq!(
            params.iter().filter(|(key, _)| key == "resource").count(),
            1
        );
        assert!(params.contains(&("resource".into(), RESOURCE.into())));
    }
    let code: BTreeMap<_, _> = url::form_urlencoded::parse(provider.requests[0].body())
        .into_owned()
        .collect();
    assert_eq!(code["grant_type"], "authorization_code");
    assert_eq!(code["code"], "code+&= value");
    assert_eq!(code["code_verifier"], verifier);
    assert_eq!(code["redirect_uri"], "https://golem.example/callback");
    let refresh: BTreeMap<_, _> = url::form_urlencoded::parse(provider.requests[1].body())
        .into_owned()
        .collect();
    assert_eq!(refresh["grant_type"], "refresh_token");
    assert_eq!(refresh["refresh_token"], "rotated");
    assert!(!refresh.contains_key("code_verifier"));
    assert!(!refresh.contains_key("code"));
}

#[test]
async fn mismatched_callback_cannot_transmit_code_and_exchange_failures_do_not_retry() {
    let client = client();
    for issuer in [None, Some("https://issuer.example/tenant/")] {
        let mut provider = Provider::default();
        assert!(
            client
                .exchange_code(
                    &mut provider,
                    AuthorizationCode::new("secret-code".into()),
                    client.authorize(&[]).unwrap().verifier,
                    issuer,
                    Limits::default()
                )
                .await
                .is_err()
        );
        assert!(provider.requests.is_empty());
    }
    for failure in [
        TransportError::QuotaExhausted,
        TransportError::Network,
        TransportError::Denied,
    ] {
        let expected = failure.to_string();
        let mut provider = Provider {
            failure: Some(failure),
            ..Default::default()
        };
        let error = client
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh-secret".into()),
                Limits::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert_eq!(provider.requests.len(), 1);
    }
    for (status, body) in [
        (StatusCode::FOUND, Value::Null),
        (StatusCode::SERVICE_UNAVAILABLE, Value::Null),
        (
            StatusCode::BAD_REQUEST,
            json!({"error":"invalid_grant", "error_description":"refresh-secret"}),
        ),
        (
            StatusCode::OK,
            json!({"access_token":"refresh-secret", "expires_in":"invalid", "token_type":"Bearer"}),
        ),
    ] {
        let mut provider = Provider {
            responses: VecDeque::from([response(status, body)]),
            ..Default::default()
        };
        let error = client
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh-secret".into()),
                Limits::default(),
            )
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("refresh-secret"));
        assert_eq!(provider.requests.len(), 1);
    }
}

#[test]
async fn token_response_limits_include_streamed_bytes_and_normalize_headers() {
    let client = client();
    let length = tokens().to_string().len();
    for maximum in [length, length - 1] {
        let encoded = tokens().to_string();
        let frames: Vec<_> = encoded
            .as_bytes()
            .chunks(7)
            .map(|chunk| {
                Ok::<_, std::convert::Infallible>(http_body::Frame::data(Bytes::copy_from_slice(
                    chunk,
                )))
            })
            .collect();
        let body = StreamBody::new(futures::stream::iter(frames)).boxed_unsync();
        let mut response = Response::new(body);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            "Application/JSON; charset=utf-8".parse().unwrap(),
        );
        response
            .headers_mut()
            .append(header::CONTENT_ENCODING, " Identity , ".parse().unwrap());
        let mut provider = Provider {
            responses: VecDeque::from([response]),
            ..Default::default()
        };
        let result = client
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh".into()),
                Limits {
                    document_bytes: maximum,
                    ..Limits::default()
                },
            )
            .await;
        if maximum == length {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(TransportError::Limit(_))));
        }
    }
    let mut provider = Provider::default();
    assert!(matches!(
        client
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh".into()),
                Limits {
                    request_bytes: 1,
                    ..Limits::default()
                }
            )
            .await,
        Err(TransportError::Limit(_))
    ));
    assert!(provider.requests.is_empty());
}

#[test]
async fn discovery_has_one_deadline_across_the_fallback_chain() {
    let mut provider = Provider {
        delay: Duration::from_millis(40),
        responses: VecDeque::from([
            response(StatusCode::NOT_FOUND, Value::Null),
            response(StatusCode::OK, resource_metadata()),
            response(StatusCode::OK, metadata()),
        ]),
        ..Default::default()
    };
    assert!(matches!(
        discover(
            &mut provider,
            RESOURCE,
            "https://issuer.example/tenant",
            &HeaderMap::new(),
            Limits {
                timeout: Duration::from_millis(70),
                ..Limits::default()
            }
        )
        .await,
        Err(TransportError::Timeout)
    ));
    assert!(provider.requests.len() <= 2);
}

#[test]
async fn discovery_preserves_raw_identity_but_uses_rfc_well_known_paths() {
    let resource = "https://mcp.example/a/b/?tenant=one";
    let issuer = "https://issuer.example/tenant/";
    let mut protected = resource_metadata();
    protected["resource"] = json!(resource);
    protected["authorization_servers"] = json!([issuer]);
    let mut server = metadata();
    server["issuer"] = json!(issuer);
    let mut provider = Provider {
        responses: VecDeque::from([
            response(StatusCode::OK, protected),
            response(StatusCode::OK, server),
        ]),
        ..Default::default()
    };
    let found = discover(
        &mut provider,
        resource,
        issuer,
        &HeaderMap::new(),
        Limits::default(),
    )
    .await
    .unwrap();
    assert_eq!(found.server.issuer(), issuer);
    assert_eq!(
        provider.requests[0].uri().to_string(),
        "https://mcp.example/.well-known/oauth-protected-resource/a/b?tenant=one"
    );
    assert_eq!(
        provider.requests[1].uri().to_string(),
        "https://issuer.example/.well-known/oauth-authorization-server/tenant"
    );
}

#[test]
async fn client_post_and_public_authentication_keep_secrets_out_of_uris() {
    for (method, secret) in [("client_secret_post", Some("p&= ss")), ("none", None)] {
        let mut metadata = metadata();
        metadata["token_endpoint_auth_methods_supported"] = json!([method]);
        let client = validate(metadata)
            .unwrap()
            .client(
                "client&",
                secret,
                "https://golem.example/callback",
                RESOURCE,
            )
            .unwrap();
        let mut provider = Provider {
            responses: VecDeque::from([response(StatusCode::OK, tokens())]),
            ..Default::default()
        };
        client
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh".into()),
                Limits::default(),
            )
            .await
            .unwrap();
        let request = &provider.requests[0];
        assert!(request.uri().query().is_none());
        assert!(!request.headers().contains_key(header::AUTHORIZATION));
        let params: BTreeMap<_, _> = url::form_urlencoded::parse(request.body())
            .into_owned()
            .collect();
        assert_eq!(params["client_id"], "client&");
        assert_eq!(params.get("client_secret").map(String::as_str), secret);
    }
}

#[test]
async fn grant_rejection_configuration_and_protocol_errors_remain_distinct_and_redacted() {
    for (status, code, expected) in [
        (
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            TransportError::OAuthGrantRejected,
        ),
        (
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            invalid("OAuth client rejected"),
        ),
        (
            StatusCode::BAD_REQUEST,
            "reflected-refresh-secret",
            protocol("unrecognized OAuth token error"),
        ),
    ] {
        let mut provider = Provider {
            responses: VecDeque::from([response(
                status,
                json!({"error":code, "error_description":"reflected-refresh-secret"}),
            )]),
            ..Default::default()
        };
        let error = client()
            .refresh(
                &mut provider,
                &RefreshToken::new("refresh".into()),
                Limits::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("reflected-refresh-secret"));
    }
    for (name, value) in [
        (header::CONTENT_TYPE, "text/html"),
        (header::CONTENT_ENCODING, "gzip"),
        (header::CONTENT_LENGTH, "not-a-number"),
    ] {
        let mut malformed = response(StatusCode::OK, tokens());
        malformed.headers_mut().insert(name, value.parse().unwrap());
        let mut provider = Provider {
            responses: VecDeque::from([malformed]),
            ..Default::default()
        };
        assert!(matches!(
            client()
                .refresh(
                    &mut provider,
                    &RefreshToken::new("refresh".into()),
                    Limits::default()
                )
                .await,
            Err(TransportError::Protocol(_))
        ));
    }
}
