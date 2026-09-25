use golem_rust::agentic::{AgentStream, Config, Header, HttpRequest, HttpResponse, HttpRouter, spawn_local};
use golem_rust::http_router;

struct RawHttpRouterImpl;

fn stream(chunks: Vec<Vec<u8>>) -> AgentStream<Vec<u8>> {
    let (mut writer, stream) = AgentStream::new();
    spawn_local(async move {
        let _ = writer.write_all(chunks).await;
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
        use golem_rust::agentic::{get_agent_id, get_principal};
        use golem_rust::golem_agentic::golem::agent::common::Principal;
        assert!(matches!(get_principal(), Some(Principal::Anonymous)));
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
            "x-provider-agent": get_agent_id().agent_id,
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
        let local_path = request.path.strip_prefix("/raw").unwrap_or(&request.path);
        let (status, body) = match local_path {
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
                    while let Ok(Some(chunk)) = input.next().await {
                        if writer.write_one(chunk).await.is_err() {
                            break;
                        }
                    }
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
