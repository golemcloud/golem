use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt, stream};
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use poem::endpoint::{make, make_sync};
use poem::listener::TcpAcceptor;
use poem::{Body, EndpointExt, IntoResponse};
use test_r::{test, timeout};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use super::run;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Default)]
struct LiveBodies {
    count: Arc<AtomicUsize>,
    changed: Arc<Notify>,
}

impl LiveBodies {
    fn guard(&self) -> BodyGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        BodyGuard(self.clone())
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    async fn wait_for(&self, expected: usize) {
        tokio::time::timeout(CLEANUP_TIMEOUT, async {
            loop {
                let changed = self.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if self.count() == expected {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("body count did not become {expected}; was {}", self.count()));
    }
}

struct BodyGuard(LiveBodies);

impl Drop for BodyGuard {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

fn idle_body(guard: BodyGuard) -> Body {
    let initial = stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"initial\n")) });
    let pending = stream::pending::<Result<Bytes, std::io::Error>>();
    Body::from_bytes_stream(initial.chain(pending).map(move |item| {
        let _ = &guard;
        item
    }))
}

async fn spawn_server(
    endpoint: impl poem::Endpoint<Output = poem::Response> + 'static,
) -> (SocketAddr, tokio::task::JoinHandle<std::io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = TcpAcceptor::from_tokio(listener).unwrap();
    (address, tokio::spawn(run(acceptor, endpoint)))
}

async fn http1_client(
    address: SocketAddr,
) -> (
    hyper::client::conn::http1::SendRequest<Empty<Bytes>>,
    tokio::task::JoinHandle<Result<(), hyper::Error>>,
) {
    let io = TokioIo::new(TcpStream::connect(address).await.unwrap());
    let (sender, connection) = hyper::client::conn::http1::handshake(io).await.unwrap();
    (sender, tokio::spawn(connection.with_upgrades()))
}

fn request(path: &str) -> Request<Empty<Bytes>> {
    Request::builder().uri(path).body(Empty::new()).unwrap()
}

#[poem::handler]
async fn websocket_echo(
    websocket: poem::web::websocket::WebSocket,
    bodies: poem::web::Data<&LiveBodies>,
) -> poem::Response {
    let bodies = bodies.0.clone();
    websocket
        .on_upgrade(move |mut socket| async move {
            let _guard = bodies.guard();
            while let Some(Ok(message)) = socket.next().await {
                if socket.send(message).await.is_err() {
                    break;
                }
            }
        })
        .into_response()
}

#[test]
#[timeout("20s")]
async fn websocket_outlives_http_upgrade_and_releases_on_disconnect() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let bodies = LiveBodies::default();
    let (address, server) = spawn_server(websocket_echo.data(bodies.clone())).await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n").await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(socket.read_u8().await.unwrap());
    }
    assert!(headers.starts_with(b"HTTP/1.1 101 "));
    bodies.wait_for(1).await;
    // A masked client text frame containing "hi".
    socket
        .write_all(&[0x81, 0x82, 1, 2, 3, 4, b'h' ^ 1, b'i' ^ 2])
        .await
        .unwrap();
    let mut reply = [0; 4];
    socket.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply, [0x81, 2, b'h', b'i']);
    drop(socket);
    bodies.wait_for(0).await;
    server.abort();
}

#[test]
#[timeout("20s")]
async fn http1_idle_body_is_live_until_the_client_disconnects() {
    let bodies = LiveBodies::default();
    let endpoint = make_sync({
        let bodies = bodies.clone();
        move |_| poem::Response::builder().body(idle_body(bodies.guard()))
    });
    let (address, server) = spawn_server(endpoint).await;
    let (mut sender, connection) = http1_client(address).await;

    let response = sender.send_request(request("/")).await.unwrap();
    let mut body = response.into_body();
    let frame = body.frame().await.unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), "initial\n");
    bodies.wait_for(1).await;

    assert!(
        tokio::time::timeout(Duration::from_millis(100), bodies.changed.notified())
            .await
            .is_err()
    );
    drop(sender);
    connection.abort();
    bodies.wait_for(0).await;
    drop(body);
    server.abort();
}

#[test]
#[timeout("20s")]
async fn disconnect_cancels_a_pending_http1_handler() {
    let entered = Arc::new(Notify::new());
    let handlers = LiveBodies::default();
    let endpoint = make({
        let entered = entered.clone();
        let handlers = handlers.clone();
        move |_| {
            let entered = entered.clone();
            let guard = handlers.guard();
            async move {
                entered.notify_one();
                std::future::pending::<()>().await;
                drop(guard);
                poem::Response::default()
            }
        }
    });
    let (address, server) = spawn_server(endpoint).await;
    let mut socket = TcpStream::connect(address).await.unwrap();
    use tokio::io::AsyncWriteExt;
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    entered.notified().await;
    handlers.wait_for(1).await;
    drop(socket);
    handlers.wait_for(0).await;
    server.abort();
}

#[test]
#[timeout("20s")]
async fn http2_stream_and_transport_lifetimes_are_independent() {
    let bodies = LiveBodies::default();
    let endpoint = make_sync({
        let bodies = bodies.clone();
        move |_| poem::Response::builder().body(idle_body(bodies.guard()))
    });
    let (address, server) = spawn_server(endpoint).await;
    let relay_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_address = relay_listener.local_addr().unwrap();
    let relay = tokio::spawn(async move {
        let (mut client, _) = relay_listener.accept().await.unwrap();
        let mut upstream = TcpStream::connect(address).await.unwrap();
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    });
    let io = TokioIo::new(TcpStream::connect(relay_address).await.unwrap());
    let (mut sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
        .await
        .unwrap();
    let connection = tokio::spawn(connection);

    let mut first = sender
        .send_request(request("/one"))
        .await
        .unwrap()
        .into_body();
    let mut second = sender
        .send_request(request("/two"))
        .await
        .unwrap()
        .into_body();
    first.frame().await.unwrap().unwrap();
    second.frame().await.unwrap().unwrap();
    bodies.wait_for(2).await;

    drop(first);
    bodies.wait_for(1).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), bodies.changed.notified())
            .await
            .is_err()
    );

    drop(sender);
    relay.abort();
    connection.abort();
    bodies.wait_for(0).await;
    drop(second);
    server.abort();
}

#[test]
#[timeout("20s")]
async fn aborting_the_server_releases_active_response_bodies() {
    let bodies = LiveBodies::default();
    let endpoint = make_sync({
        let bodies = bodies.clone();
        move |_| poem::Response::builder().body(idle_body(bodies.guard()))
    });
    let (address, server) = spawn_server(endpoint).await;
    let (mut sender, connection) = http1_client(address).await;
    let mut body = sender.send_request(request("/")).await.unwrap().into_body();
    body.frame().await.unwrap().unwrap();
    bodies.wait_for(1).await;

    let io = TokioIo::new(TcpStream::connect(address).await.unwrap());
    let (mut h2_sender, h2_connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), io)
            .await
            .unwrap();
    let h2_connection = tokio::spawn(h2_connection);
    let mut h2_body = h2_sender
        .send_request(request("/"))
        .await
        .unwrap()
        .into_body();
    h2_body.frame().await.unwrap().unwrap();
    bodies.wait_for(2).await;

    server.abort();
    bodies.wait_for(0).await;
    connection.abort();
    h2_connection.abort();
}

#[test]
#[timeout("20s")]
async fn request_metadata_errors_and_http1_keep_alive_are_preserved() {
    let endpoint = make_sync(|req: poem::Request| {
        if req.uri().path() == "/error" {
            return Err(poem::Error::from_status(StatusCode::INTERNAL_SERVER_ERROR));
        }
        Ok(poem::Response::builder().body(format!(
            "{}|{}|{}",
            req.scheme(),
            req.local_addr(),
            req.remote_addr()
        )))
    });
    let (address, server) = spawn_server(endpoint).await;
    let (mut sender, connection) = http1_client(address).await;

    let error = sender.send_request(request("/error")).await.unwrap();
    assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
    error.into_body().collect().await.unwrap();

    let response = sender.send_request(request("/metadata")).await.unwrap();
    let text = String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap();
    let mut fields = text.split('|');
    let expected_local_address = format!("socket://{address}");
    assert_eq!(fields.next(), Some("http"));
    assert_eq!(fields.next(), Some(expected_local_address.as_str()));
    assert!(fields.next().unwrap().starts_with("socket://127.0.0.1:"));
    assert_eq!(fields.next(), None);

    drop(sender);
    connection.abort();
    server.abort();
}
