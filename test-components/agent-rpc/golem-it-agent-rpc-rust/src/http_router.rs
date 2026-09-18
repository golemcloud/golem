use golem_rust::agentic::{AgentStream, spawn_local};
use golem_rust::{
    FromSchema, IntoSchema, agent_definition, agent_implementation, description, endpoint,
};

#[derive(IntoSchema, FromSchema)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

#[derive(IntoSchema, FromSchema)]
pub struct HttpRequest {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<Header>,
    pub body: AgentStream<Vec<u8>>,
}

#[derive(IntoSchema, FromSchema)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: AgentStream<Vec<u8>>,
}

#[agent_definition(
    kind = "http-router",
    ephemeral,
    mount = "/raw",
    auth = false,
    cors = ["https://allowed.test"],
    snapshotting = "disabled",
)]
#[description("Raw streaming HTTP router test fixture")]
pub trait RawHttpRouter {
    fn new() -> Self;
    #[endpoint(any = "/")]
    async fn route(&self, request: HttpRequest) -> HttpResponse;
}

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

#[agent_implementation]
impl RawHttpRouter for RawHttpRouterImpl {
    fn new() -> Self {
        Self
    }

    async fn route(&self, request: HttpRequest) -> HttpResponse {
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
