use super::*;
use crate::transport::{Client, Limits};
use base64::Engine;
use futures::{StreamExt, stream};
use http::{StatusCode, header};
use http_body::Frame;
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc, time::Duration};
use test_r::{test, timeout};
use tokio::{net::TcpListener, sync::mpsc, task::JoinSet};

type TestBody = UnsyncBoxBody<Bytes, Infallible>;

struct Server {
    url: String,
    requests: mpsc::UnboundedReceiver<Request<Bytes>>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(
        handler: impl Fn(&Request<Bytes>) -> Response<TestBody> + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (sent, requests) = mpsc::unbounded_channel();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let sent = sent.clone();
                        let handler = handler.clone();
                        connections.spawn(async move {
                            let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                                let sent = sent.clone();
                                let handler = handler.clone();
                                async move {
                                    let (parts, body) = request.into_parts();
                                    let request = Request::from_parts(parts, body.collect().await.unwrap().to_bytes());
                                    let response = handler(&request);
                                    sent.send(request).unwrap();
                                    Ok::<_, Infallible>(response)
                                }
                            });
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(socket), service).await;
                        });
                    }
                    Some(result) = connections.join_next(), if !connections.is_empty() => { result.unwrap(); }
                }
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn json_response(request: &Request<Bytes>, result: Value) -> Response<TestBody> {
    let request: Value = serde_json::from_slice(request.body()).unwrap();
    let body = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)).boxed_unsync())
        .unwrap()
}

#[derive(Debug, PartialEq)]
enum TestError {
    Transport(TransportError),
    Suspend,
}

impl From<TransportError> for TestError {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

struct Policy {
    target: Uri,
    checks: usize,
    charges: usize,
    deny_after: usize,
    quota: usize,
}

impl Policy {
    fn new(url: &str) -> Self {
        Self {
            target: url.parse().unwrap(),
            checks: 0,
            charges: 0,
            deny_after: usize::MAX,
            quota: usize::MAX,
        }
    }
}

impl HttpPolicy for Policy {
    type Error = TestError;

    async fn admit(&mut self, target: &Uri) -> Result<(), Self::Error> {
        self.checks += 1;
        if self.checks > self.deny_after || target != &self.target {
            return Err(TransportError::Denied.into());
        }
        if self.charges == self.quota {
            return Err(TestError::Suspend);
        }
        self.charges += 1;
        Ok(())
    }
}

#[test]
#[timeout("10s")]
async fn paginated_http_rechecks_authority_and_charges_every_request() {
    let mut server = Server::start(|request| {
        let body: Value = serde_json::from_slice(request.body()).unwrap();
        json_response(
            request,
            if body["params"].get("cursor").is_none() {
                json!({"tools":[{"name":"first"}],"nextCursor":"page2"})
            } else {
                json!({"tools":[{"name":"second"}]})
            },
        )
    })
    .await;
    let mut sender = HttpSender::new(Policy::new(&server.url)).unwrap();
    let client = Client::new(&server.url, None, Limits::default())
        .unwrap()
        .with_bearer("dummy-token")
        .unwrap();
    let listing = client.list_tools(&mut sender).await.unwrap();
    assert_eq!(
        listing.tools,
        vec![json!({"name":"first"}), json!({"name":"second"})]
    );
    assert_eq!((sender.policy.checks, sender.policy.charges), (2, 2));
    for _ in 0..2 {
        let request = server.requests.recv().await.unwrap();
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Bearer dummy-token"
        );
        assert_eq!(request.headers()[header::ACCEPT_ENCODING], "identity");
        assert_eq!(request.headers()["Mcp-Method"], "tools/list");
        assert!(!request.headers().contains_key(header::PROXY_AUTHORIZATION));
    }

    let mut policy = Policy::new(&server.url);
    policy.deny_after = 1;
    let mut sender = HttpSender::new(policy).unwrap();
    assert_eq!(
        client.list_tools(&mut sender).await.unwrap_err(),
        TestError::Transport(TransportError::Denied)
    );
    assert_eq!((sender.policy.checks, sender.policy.charges), (2, 1));
    server.requests.recv().await.unwrap();
    assert!(server.requests.try_recv().is_err());
}

#[test]
#[timeout("10s")]
async fn host_quota_failure_propagates_without_dispatch_or_retry() {
    let mut server = Server::start(|_| panic!("quota-denied request reached server")).await;
    let mut policy = Policy::new(&server.url);
    policy.quota = 0;
    let mut sender = HttpSender::new(policy).unwrap();
    let client = Client::new(&server.url, None, Limits::default()).unwrap();
    assert_eq!(
        client.list_tools(&mut sender).await.unwrap_err(),
        TestError::Suspend
    );
    assert_eq!((sender.policy.checks, sender.policy.charges), (1, 0));
    assert!(server.requests.try_recv().is_err());
}

#[test]
#[timeout("10s")]
async fn real_http_redirect_and_authentication_failures_never_repeat_requests() {
    let mut destination = Server::start(|_| panic!("redirect followed")).await;
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::UNAUTHORIZED,
        StatusCode::SERVICE_UNAVAILABLE,
    ] {
        let location = destination.url.clone();
        let mut server = Server::start(move |_| {
            Response::builder()
                .status(status)
                .header(header::LOCATION, &location)
                .header(header::WWW_AUTHENTICATE, "Bearer")
                .header(header::RETRY_AFTER, "0")
                .body(Full::new(Bytes::new()).boxed_unsync())
                .unwrap()
        })
        .await;
        let mut sender = HttpSender::new(Policy::new(&server.url)).unwrap();
        let client = Client::new(&server.url, None, Limits::default()).unwrap();
        let expected = if status == StatusCode::UNAUTHORIZED {
            TransportError::AuthorizationRequired(401)
        } else {
            TransportError::HttpStatus(status.as_u16())
        };
        assert_eq!(
            client.list_tools(&mut sender).await.unwrap_err(),
            TestError::Transport(expected)
        );
        assert_eq!((sender.policy.checks, sender.policy.charges), (1, 1));
        server.requests.recv().await.unwrap();
        assert!(server.requests.try_recv().is_err());
    }
    assert!(destination.requests.try_recv().is_err());
}

#[test]
#[timeout("10s")]
async fn http_sender_preserves_encoded_bytes_and_streams_before_eof() {
    let gzip = Bytes::from(
        base64::engine::general_purpose::STANDARD
            .decode("H4sIAAAAAAACA8tIzcnJBwCGphA2BQAAAA==")
            .unwrap(),
    );
    let expected = gzip.clone();
    let server = Server::start(move |_| {
        let frames = stream::once(std::future::ready(Ok(Frame::data(gzip.clone()))))
            .chain(stream::pending());
        Response::builder()
            .header(header::CONTENT_ENCODING, "gzip")
            .body(StreamBody::new(frames).boxed_unsync())
            .unwrap()
    })
    .await;
    let mut sender = HttpSender::new(Policy::new(&server.url)).unwrap();
    let request = Request::post(&server.url).body(Bytes::new()).unwrap();
    let mut response = sender.send(request).await.unwrap();
    assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
    let frame = tokio::time::timeout(Duration::from_secs(2), response.body_mut().frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_data().unwrap(), expected);
}

#[test]
#[timeout("10s")]
async fn timeout_closes_the_real_http_response_stream() {
    struct OnDrop(Arc<tokio::sync::Notify>);
    impl Drop for OnDrop {
        fn drop(&mut self) {
            self.0.notify_one();
        }
    }
    let dropped = Arc::new(tokio::sync::Notify::new());
    let signal = dropped.clone();
    let mut server = Server::start(move |_| {
        let guard = OnDrop(signal.clone());
        let tail = stream::pending::<Result<Frame<Bytes>, Infallible>>().map(move |frame| {
            let _ = &guard;
            frame
        });
        let frames = stream::iter([Ok(Frame::data(Bytes::from_static(b"{")))]).chain(tail);
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json")
            .body(StreamBody::new(frames).boxed_unsync())
            .unwrap()
    })
    .await;
    let mut sender = HttpSender::new(Policy::new(&server.url)).unwrap();
    let client = Client::new(
        &server.url,
        None,
        Limits {
            timeout: Duration::from_millis(500),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        client.list_tools(&mut sender).await.unwrap_err(),
        TestError::Transport(TransportError::Timeout)
    );
    dropped.notified().await;
    server.requests.recv().await.unwrap();
    assert!(server.requests.try_recv().is_err());
    assert_eq!((sender.policy.checks, sender.policy.charges), (1, 1));
}

#[test]
#[timeout("10s")]
async fn lost_response_does_not_resend_the_request() {
    use tokio::io::AsyncReadExt;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let mut sender = HttpSender::new(Policy::new(&url)).unwrap();
    let client = Client::new(&url, None, Limits::default()).unwrap();
    let peer = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        assert!(socket.read(&mut buffer).await.unwrap() > 0);
        drop(socket);
    };
    let (result, ()) = tokio::join!(client.list_tools(&mut sender), peer);
    assert_eq!(
        result.unwrap_err(),
        TestError::Transport(TransportError::Network)
    );
    assert_eq!((sender.policy.checks, sender.policy.charges), (1, 1));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}
