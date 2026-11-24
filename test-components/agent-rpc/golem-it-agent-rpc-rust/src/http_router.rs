use golem_rust::agentic::{
    AgentStream, Config, Header, HttpRequest, HttpResponse, HttpRouter, spawn_local,
};
use golem_rust::http_router;

struct RawHttpRouterImpl;

fn stream(chunks: Vec<Vec<u8>>) -> AgentStream<Vec<u8>> {
    let (mut writer, stream) = AgentStream::new();
    spawn_local(async move {
        let _ = writer.write_all(chunks).await;
        let _ = writer.close().await;
    });
    stream
}

fn header(name: &str, value: &[u8]) -> Header {
    Header {
        name: name.to_string(),
        value: value.to_vec(),
    }
}

fn failing_stream(
    mut input: AgentStream<Vec<u8>>,
    before_failure: &'static [u8],
) -> AgentStream<Vec<u8>> {
    use golem_rust::retry::{NamedPolicy, Policy, set_named_policy};

    set_named_policy(&NamedPolicy::named("http-test-failure", Policy::never()).priority(u32::MAX))
        .expect("valid no-retry policy");
    let (mut writer, output) = AgentStream::new();
    spawn_local(async move {
        let _ = writer.write_one(before_failure.to_vec()).await;
        // Disposal of a suppressed body must not suppress the invocation failure.
        let _ = input.next().await;
        panic!("intentional HTTP response producer failure");
    });
    output
}

struct StaticHttpRouterImpl;

struct AuthenticatedHttpRouterImpl;

#[http_router(name = "AuthenticatedHttpRouter", mount = "/raw", auth = true,
    static_files = [("/static/*", "/assets/$1")])]
impl HttpRouter for AuthenticatedHttpRouterImpl {
    type Config = ();

    fn new(_: Config<()>) -> Self {
        Self
    }

    async fn handle(&self, request: HttpRequest) -> HttpResponse {
        RawHttpRouterImpl.handle(request).await
    }
}

#[http_router(name = "StaticHttpRouter", mount = "/raw", auth = false,
    cors = ["https://allowed.test"],
    static_files = [("/favicon", "/assets/asset.txt"), ("/static/*", "/assets/$1")])]
impl HttpRouter for StaticHttpRouterImpl {
    type Config = ();

    fn new(_: Config<()>) -> Self {
        Self
    }

    async fn handle(&self, request: HttpRequest) -> HttpResponse {
        RawHttpRouterImpl.handle(request).await
    }

    async fn openapi(&self) -> String {
        use golem_rust::agentic::get_agent_id;
        use golem_rust::golem_agentic::golem::agent::common::Principal;
        use golem_rust::golem_agentic::golem::agent::invocation_context;
        assert!(matches!(
            invocation_context::caller().immediate,
            Principal::Anonymous
        ));
        if std::env::var("TEST_OPENAPI_INVALID").as_deref() == Ok("true") {
            return "invalid-provider-document".to_string();
        }
        serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "Provider", "version": "1"},
            "paths": {
                "/": {"get": {"responses": {"200": {"description": "root"}}}},
                "/echo/": {"post": {"operationId": "rawEcho", "responses": {"200": {"description": "echo"}}}}
            },
            "x-provider-agent": get_agent_id().uuid.to_string(),
            "x-provider-revision": golem_rust::get_self_metadata().unwrap().component_revision
        }).to_string()
    }
}

#[http_router(name = "RawHttpRouter", mount = "/raw", auth = false, cors = ["https://allowed.test"])]
impl HttpRouter for RawHttpRouterImpl {
    type Config = ();

    fn new(_: Config<()>) -> Self {
        Self
    }

    async fn handle(&self, request: HttpRequest) -> HttpResponse {
        let mut headers = vec![
            header("x-echo-method", request.method.as_bytes()),
            header("x-echo-path", request.path.as_bytes()),
            header(
                "x-echo-query",
                request.query.as_deref().unwrap_or("").as_bytes(),
            ),
            header("set-cookie", b"first=one"),
            header("set-cookie", b"second=\x80"),
        ];
        if let Some(key) = request
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("Idempotency-Key"))
        {
            headers.push(header("x-echo-idempotency-key", &key.value));
        }
        let local_path = request.path.strip_prefix("/raw").unwrap_or(&request.path);
        let (status, body) = match local_path {
            "/count-effect" => {
                let url = request
                    .headers
                    .iter()
                    .find(|h| h.name.eq_ignore_ascii_case("x-counter-url"))
                    .unwrap();
                let counter_request =
                    golem_rust::http::Request::post(std::str::from_utf8(&url.value).unwrap())
                        .body(Vec::new())
                        .unwrap();
                let response = golem_rust::http::send(counter_request).await.unwrap();
                let bytes = golem_rust::http::response_bytes(response).await.unwrap();
                (200, stream(vec![bytes.to_vec()]))
            }
            "/400" => (400, stream(vec![b"guest-error".to_vec()])),
            "/cache-public" | "/cache-no-store" => {
                headers.push(header(
                    "cache-control",
                    if local_path == "/cache-public" {
                        b"public, max-age=60"
                    } else {
                        b"no-store"
                    },
                ));
                headers.push(header("vary", b"Accept-Language"));
                (200, stream(vec![b"cache-body".to_vec()]))
            }
            "/catch-input" => {
                headers.push(header(
                    "x-echo-agent",
                    golem_rust::agentic::get_agent_id()
                        .uuid
                        .to_string()
                        .as_bytes(),
                ));
                let mut input = request.body;
                let mut bytes = Vec::new();
                let mut status = 200;
                while let Some(chunk) = input.next().await {
                    match chunk {
                        Ok(chunk) => bytes.extend(chunk),
                        Err(golem_rust::schema::StreamError::Interrupted(details)) => {
                            bytes.extend(format!("|{details}|handled").as_bytes());
                            status = 422;
                            break;
                        }
                        Err(error) => panic!("unexpected request body failure: {error}"),
                    }
                }
                (status, stream(vec![bytes]))
            }
            "/empty" => (200, stream(vec![])),
            "/known-one" => {
                headers.push(header("content-length", b"1"));
                (200, stream(vec![b"K".to_vec()]))
            }
            "/head" => (200, stream(vec![b"must-not-be-polled".to_vec()])),
            "/204" => {
                headers.push(header("content-length", b"17"));
                (204, stream(vec![b"must-not-be-polled".to_vec()]))
            }
            "/205" => {
                headers.push(header("content-length", b"17"));
                (205, stream(vec![b"must-not-be-polled".to_vec()]))
            }
            "/304" => {
                headers.push(header("content-length", b"17"));
                (304, stream(vec![b"must-not-be-polled".to_vec()]))
            }
            "/cl-zero" => {
                headers.push(header("content-length", b"0"));
                (200, stream(vec![b"must-not-be-polled".to_vec()]))
            }
            "/invalid-status" => (700, stream(vec![])),
            "/invalid-header" => {
                headers.push(header("bad header", b"value"));
                (200, stream(vec![]))
            }
            "/short-cl" => {
                headers.push(header("content-length", b"5"));
                (200, stream(vec![b"xy".to_vec()]))
            }
            "/overlong-cl" => {
                headers.push(header("content-length", b"2"));
                (200, stream(vec![b"long".to_vec()]))
            }
            "/early-response" => (202, stream(vec![b"accepted".to_vec()])),
            "/large-chunk" => (200, stream(vec![vec![0xab; 512 * 1024]])),
            "/wait-head" => {
                let _input = request.body;
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(30_000_000_000).await;
                unreachable!()
            }
            "/result-late-fail" => (200, failing_stream(request.body, b"before-error")),
            "/one-byte-late-fail" => {
                headers.push(header("content-length", b"1"));
                (200, failing_stream(request.body, b"x"))
            }
            "/cl-zero-late-fail" => {
                headers.push(header("content-length", b"0"));
                (200, failing_stream(request.body, b"must-not-be-polled"))
            }
            "/slow" => {
                let (mut writer, body) = AgentStream::new();
                spawn_local(async move {
                    while writer.write_one(vec![0xa7; 64 * 1024]).await.is_ok() {}
                });
                (200, body)
            }
            _ => {
                let mut input = request.body;
                let (mut writer, output) = AgentStream::new();
                spawn_local(async move {
                    while let Some(chunk) = input.next().await {
                        let chunk = match chunk {
                            Ok(chunk) => chunk,
                            Err(error) => panic!("request body failed: {error}"),
                        };
                        if writer.write_one(chunk).await.is_err() {
                            return;
                        }
                    }
                    let _ = writer.close().await;
                });
                (200, output)
            }
        };
        HttpResponse {
            status,
            headers,
            body,
        }
    }
}
