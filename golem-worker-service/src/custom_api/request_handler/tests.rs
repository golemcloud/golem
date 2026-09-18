use super::*;
use crate::config::RouteResolverConfig;
use crate::custom_api::api_definition_lookup::{
    ApiDefinitionLookupError, HttpApiDefinitionsLookup,
};
use crate::custom_api::oidc::DefaultIdentityProvider;
use crate::custom_api::oidc::model::{
    McpPendingAuth, McpProxyCodeEntry, PendingOidcLogin, SessionId,
};
use crate::custom_api::oidc::session_store::{SessionStore, SessionStoreError};
use crate::mcp::InvocationHarness;
use async_trait::async_trait;
use golem_common::base_model::Empty;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, AgentTypeName, HttpMethod};
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::EnvironmentId;
use golem_common::schema::{
    AgentConstructorSchema, InputSchema, OutputSchema, SchemaGraph, SchemaType,
};
use golem_service_base::custom_api::{
    AgentRouteMode, CallAgentBehaviour, CompiledInputSchema, CompiledOutputSchema, CompiledRoute,
    CompiledRoutes, CompiledSchema, CorsOptions, OriginPattern, PathSegment, RequestBodySchema,
    RouteBehaviour, RouteSecurity, SessionFromHeaderRouteSecurity,
};
use std::collections::HashMap;
use std::time::Duration;
use test_r::test;

#[path = "live_files_tests.rs"]
mod live_files;

struct StaticApiDefinitionsLookup;

#[async_trait]
impl HttpApiDefinitionsLookup for StaticApiDefinitionsLookup {
    async fn get(
        &self,
        _domain: &golem_common::model::domain_registration::Domain,
    ) -> Result<CompiledRoutes, ApiDefinitionLookupError> {
        Ok(durable_stream_routes())
    }
}

struct UnusedSessionStore;

#[async_trait]
impl SessionStore for UnusedSessionStore {
    async fn store_pending_oidc_login(
        &self,
        _: &str,
        _: PendingOidcLogin,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn take_pending_oidc_login(
        &self,
        _: &str,
    ) -> Result<Option<PendingOidcLogin>, SessionStoreError> {
        unreachable!()
    }

    async fn store_authenticated_session(
        &self,
        _: &SessionId,
        _: crate::custom_api::OidcSession,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn get_authenticated_session(
        &self,
        _: &SessionId,
    ) -> Result<Option<crate::custom_api::OidcSession>, SessionStoreError> {
        unreachable!()
    }

    async fn store_mcp_pending_auth(
        &self,
        _: &str,
        _: McpPendingAuth,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn take_mcp_pending_auth(
        &self,
        _: &str,
    ) -> Result<Option<McpPendingAuth>, SessionStoreError> {
        unreachable!()
    }

    async fn store_mcp_proxy_code(
        &self,
        _: &str,
        _: McpProxyCodeEntry,
    ) -> Result<(), SessionStoreError> {
        unreachable!()
    }

    async fn take_mcp_proxy_code(
        &self,
        _: &str,
    ) -> Result<Option<McpProxyCodeEntry>, SessionStoreError> {
        unreachable!()
    }
}

fn durable_stream_routes() -> CompiledRoutes {
    CompiledRoutes {
        account_id: AccountId::new(),
        account_email: AccountEmail::new("oracle@golem.cloud"),
        environment_id: EnvironmentId::new(),
        deployment_revision: DeploymentRevision::INITIAL,
        security_schemes: HashMap::new(),
        routes: vec![CompiledRoute {
            route_id: 1,
            route_match: HttpMethod::Post(Empty {}).into(),
            path: vec![PathSegment::Literal {
                value: "stream".to_string(),
            }],
            body: RequestBodySchema::JsonBody {
                expected: CompiledSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                },
            },
            behavior: RouteBehaviour::CallAgent(CallAgentBehaviour {
                route_mode: AgentRouteMode::DurableStreams,
                base_path_variables: 0,
                component_id: ComponentId::new(),
                component_revision: ComponentRevision::INITIAL,
                agent_type: AgentTypeName("oracle-agent".to_string()),
                agent_mode: AgentMode::Durable,
                constructor_input: CompiledInputSchema {
                    graph: SchemaGraph::empty(),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                constructor_parameters: vec![],
                phantom: false,
                method_name: "stream".to_string(),
                method_input: CompiledInputSchema {
                    graph: SchemaGraph::empty(),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                method_parameters: vec![],
                expected_agent_response: CompiledOutputSchema {
                    graph: SchemaGraph::empty(),
                    output_schema: OutputSchema::Unit,
                },
                method_description: None,
                read_only: None,
            }),
            security: RouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity {
                header_name: "x-golem-session".to_string(),
            }),
            cors: CorsOptions {
                allowed_patterns: vec![OriginPattern("https://client.example".to_string())],
            },
        }],
    }
}

fn request_handler() -> RequestHandler {
    request_handler_with(
        RouteResolver::new(
            &RouteResolverConfig {
                router_cache_max_capacity: 1,
                router_cache_ttl: Duration::from_secs(60),
                router_cache_eviction_period: Duration::from_secs(60),
                trusted_ingress_addresses: vec![],
            },
            Arc::new(StaticApiDefinitionsLookup),
        ),
        Arc::new(InitialAgentFilesService::new(Arc::new(
            golem_service_base::storage::blob::memory::InMemoryBlobStorage::new(),
        ))),
    )
}

fn request_handler_with(
    route_resolver: RouteResolver,
    initial_files: Arc<InitialAgentFilesService>,
) -> RequestHandler {
    request_handler_with_worker(
        route_resolver,
        initial_files,
        invocation_harness().worker_service,
    )
}

fn invocation_harness() -> InvocationHarness {
    InvocationHarness::new(
        AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentInitialization,
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        },
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        vec![],
    )
}

fn request_handler_with_worker(
    route_resolver: RouteResolver,
    initial_files: Arc<InitialAgentFilesService>,
    worker_service: Arc<crate::service::worker::WorkerService>,
) -> RequestHandler {
    RequestHandler::new(
        Arc::new(route_resolver),
        Arc::new(CallAgentHandler::new(worker_service.clone())),
        Arc::new(DurableStreamsHandler::new(
            worker_service.clone(),
            Arc::new(CallAgentHandler::new(worker_service.clone())),
            &crate::config::DurableStreamsConfig::default(),
        )),
        Arc::new(OidcHandler::new(
            Arc::new(UnusedSessionStore),
            Arc::new(DefaultIdentityProvider),
        )),
        Arc::new(WebhookCallbackHandler::new(worker_service.clone(), vec![])),
        worker_service,
        Default::default(),
        initial_files,
    )
}

fn request(body: &'static str, session: bool, origin: bool) -> Request {
    let mut builder = Request::builder()
        .method(http::Method::POST)
        .uri("http://ds.example/stream".parse().unwrap())
        .header(http::header::HOST, "ds.example")
        .content_type("application/json");
    if session {
        builder = builder.header("x-golem-session", "{}");
    }
    if origin {
        builder = builder.header(http::header::ORIGIN, "https://client.example");
    }
    builder.body(body)
}

#[test]
async fn typed_route_can_decline_h2c_upgrade_and_dispatch_over_http1() {
    let mut request = request("not-json", true, false);
    request
        .headers_mut()
        .insert(http::header::UPGRADE, "h2c".parse().unwrap());
    request.headers_mut().insert(
        http::header::CONNECTION,
        "Upgrade, HTTP2-Settings".parse().unwrap(),
    );
    request.headers_mut().insert(
        "http2-settings",
        "AAEAAEAAAAIAAAABAAMAAABk".parse().unwrap(),
    );
    let response = request_handler().handle_request(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers()[http::header::ALLOW],
        "PUT, HEAD, GET, DELETE"
    );
}

#[test]
async fn durable_stream_route_is_guarded_at_the_request_handler_boundary() {
    let handler = request_handler();

    let response = handler
        .handle_request(request("not-json", true, false))
        .await
        .expect("durable-stream dispatch must not parse the REST body");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response.headers().get(http::header::ALLOW),
        Some(&"PUT, HEAD, GET, DELETE".parse().unwrap())
    );

    let response = handler
        .handle_request(request("not-json", false, false))
        .await
        .expect("missing session header is a response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = handler
        .handle_request(request("not-json", true, true))
        .await
        .expect("valid session reaches durable-stream dispatch");
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        response
            .headers()
            .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&"https://client.example".parse().unwrap())
    );
    assert_eq!(
        response
            .headers()
            .get(http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
        Some(&"true".parse().unwrap())
    );
    assert_eq!(
        response.headers().get(http::header::VARY),
        Some(&"Origin".parse().unwrap())
    );
}

#[test]
async fn immutable_file_corpus_through_request_handler() {
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::FileMapping;
    use golem_service_base::custom_api::RouterFileIndexEntry;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;

    fn decode(value: &serde_json::Value) -> Vec<u8> {
        value
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let mut tested = 0;
    for case in corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["suite"] == "files" && case["input"]["mode"] == "immutable")
    {
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let expect = &case["expect"];
        let files = Arc::new(InitialAgentFilesService::new(Arc::new(
            InMemoryBlobStorage::new(),
        )));
        let mut route = test_route(1, "/", None, "router");
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.static_bindings = FileMapping::compile_list([("/*", "/$1")]).unwrap();
        let mut provisions = vec![];
        if input["body_hex"].is_string() {
            provisions.push((decode(&input["body_hex"]), true));
        } else if let Some(revisions) = input["revisions"].as_object() {
            router.component_revision =
                ComponentRevision::try_from(input["selected_revision"].as_u64().unwrap()).unwrap();
            for (revision, file) in revisions {
                provisions.push((
                    decode(&file["body_hex"]),
                    revision.parse::<u64>().unwrap()
                        == input["selected_revision"].as_u64().unwrap(),
                ));
            }
        } else if let Some(indexes) = input["indexes"].as_array() {
            for index in indexes {
                provisions.push((
                    decode(&index["body_hex"]),
                    index["owner"] == input["selected_owner"],
                ));
            }
        } else {
            assert_eq!(input["provisioned"]["read_only"], false, "{id}");
            provisions.push((decode(&input["provisioned"]["body_hex"]), false));
        }
        for (bytes, indexed) in provisions {
            let size = bytes.len() as u64;
            let key = files
                .put_if_not_exists(
                    EnvironmentId(uuid::Uuid::nil()),
                    bytes
                        .map_item(|item| item.map_err(anyhow::Error::from))
                        .map_error(anyhow::Error::from),
                )
                .await
                .unwrap();
            if indexed {
                router.file_index.push(RouterFileIndexEntry {
                    path: input["path"].as_str().unwrap().into(),
                    blob_key: key,
                    size,
                });
            }
        }
        let handler = request_handler_with(test_resolver(vec![route]), files);
        let mut request = Request::builder()
            .uri(input["path"].as_str().unwrap().parse().unwrap())
            .method(input["method"].as_str().unwrap().parse().unwrap())
            .header("host", "example.com")
            .header("origin", "https://client.example");
        for header in input["headers"].as_array().into_iter().flatten() {
            request = request.header(header[0].as_str().unwrap(), header[1].as_str().unwrap());
        }
        let response = handler
            .handle_request(request.finish())
            .await
            .unwrap_or_else(|error| panic!("{id}: {error:?}"));
        let status = expect["status"]
            .as_u64()
            .unwrap_or(if expect["lookup"] == "absent" {
                404
            } else {
                200
            });
        assert_eq!(u64::from(response.status().as_u16()), status, "{id}");
        assert_eq!(
            response.headers()["access-control-allow-origin"],
            "https://client.example",
            "{id}"
        );
        for header in expect["headers"].as_array().into_iter().flatten() {
            assert_eq!(
                response.headers().get(header[0].as_str().unwrap()).unwrap(),
                header[1].as_str().unwrap(),
                "{id}"
            );
        }
        for header in expect["absent_headers"].as_array().into_iter().flatten() {
            assert!(
                !response.headers().contains_key(header.as_str().unwrap()),
                "{id}"
            );
        }
        if !expect["body_hex"].is_null() {
            assert_eq!(
                response.into_body().into_vec().await.unwrap(),
                decode(&expect["body_hex"]),
                "{id}"
            );
        }
        tested += 1;
    }
    assert_eq!(tested, 11);
}

#[test]
#[test_r::timeout("20s")]
async fn immutable_files_over_http1_and_http2_do_not_cache_contents() {
    use crate::custom_api::poem_endpoint::CustomApiPoemEndpoint;
    use crate::custom_api::route_resolver::tests::{test_resolver, test_route};
    use golem_common::model::agent::FileMapping;
    use golem_service_base::custom_api::RouterFileIndexEntry;
    use golem_service_base::replayable_stream::ReplayableStream;
    use golem_service_base::storage::blob::{
        BlobStorage, BlobStorageNamespace, memory::InMemoryBlobStorage,
    };
    use tokio_util::task::AbortOnDropHandle;

    for http2 in [false, true] {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let files = Arc::new(InitialAgentFilesService::new(storage.clone()));
        let environment_id = EnvironmentId(uuid::Uuid::nil());
        let key = files
            .put_if_not_exists(
                environment_id,
                b"abcdefg"
                    .to_vec()
                    .map_item(|item| item.map_err(anyhow::Error::from))
                    .map_error(anyhow::Error::from),
            )
            .await
            .unwrap();
        let mut route = test_route(1, "/", None, "router");
        route.cors.allowed_patterns = vec![OriginPattern("https://client.example".into())];
        let RouteBehaviour::HttpRouter(router) = &mut route.behavior else {
            unreachable!()
        };
        router.static_bindings = FileMapping::compile_list([("/asset", "/private/a.txt")]).unwrap();
        router.file_index.push(RouterFileIndexEntry {
            path: "/private/a.txt".into(),
            blob_key: key,
            size: 7,
        });
        let handler = Arc::new(request_handler_with(test_resolver(vec![route]), files));
        let endpoint = CustomApiPoemEndpoint::new(handler);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let acceptor = poem::listener::TcpAcceptor::from_tokio(listener).unwrap();
        let _server =
            AbortOnDropHandle::new(tokio::spawn(crate::gateway_server::run(acceptor, endpoint)));
        let builder = reqwest::Client::builder().timeout(Duration::from_secs(5));
        let client = if http2 {
            builder.http2_prior_knowledge()
        } else {
            builder.http1_only()
        }
        .build()
        .unwrap();
        for (method, conditional, status, length, bytes) in [
            (
                reqwest::Method::GET,
                false,
                200,
                Some("7"),
                b"abcdefg".as_slice(),
            ),
            (reqwest::Method::HEAD, false, 200, Some("7"), b"".as_slice()),
            (reqwest::Method::GET, true, 304, None, b"".as_slice()),
            (reqwest::Method::HEAD, true, 304, None, b"".as_slice()),
        ] {
            let mut request = client.request(method, format!("http://{address}/asset"));
            if conditional {
                request = request.header("if-none-match", "*");
            }
            let response = request.send().await.unwrap();
            assert_eq!(
                response.version(),
                if http2 {
                    http::Version::HTTP_2
                } else {
                    http::Version::HTTP_11
                }
            );
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get("content-length")
                    .map(|v| v.to_str().unwrap()),
                length
            );
            assert_eq!(response.bytes().await.unwrap().as_ref(), bytes);
        }
        storage
            .delete(
                "test",
                "delete",
                BlobStorageNamespace::InitialAgentFiles { environment_id },
                &std::path::PathBuf::from(key.0.to_string()),
            )
            .await
            .unwrap();
        for (method, conditional) in [
            (reqwest::Method::GET, false),
            (reqwest::Method::GET, true),
            (reqwest::Method::HEAD, true),
        ] {
            let mut request = client
                .request(method, format!("http://{address}/asset"))
                .header("origin", "https://client.example");
            if conditional {
                request = request.header("if-none-match", "*");
            }
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(
                response.headers()["access-control-allow-origin"],
                "https://client.example"
            );
            let body = response.text().await.unwrap();
            assert!(
                !body.contains("private") && !body.contains("blob"),
                "{body}"
            );
        }
    }
}
