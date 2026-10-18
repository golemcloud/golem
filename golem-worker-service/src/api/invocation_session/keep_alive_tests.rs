// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
use crate::transport_test_support::{IDLE_TIMEOUT, INTERVAL, SILENCE, idle_proxy, server};
use golem_client::invocation_session::{InvocationSessionTransport, ServerFrame};
use golem_common::model::invocation_session_public::INVOCATION_SESSION_SUBPROTOCOL;
use poem::web::{Data, websocket::WebSocket};
use poem::{EndpointExt, IntoResponse};
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::sync::CancellationToken;

struct FailedOutput;

impl futures::Sink<Message> for FailedOutput {
    type Error = std::io::Error;
    fn poll_ready(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()))
    }
    fn start_send(self: std::pin::Pin<&mut Self>, _: Message) -> std::io::Result<()> {
        unreachable!()
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
    fn poll_close(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

#[test]
#[timeout("5s")]
async fn failed_output_cancels_an_input_dependent_session_without_peer_input() {
    let cancelled = CancellationToken::new();
    let observed = cancelled.clone();
    let socket = WebSocketKeepAlive::from_sink_and_stream(
        futures::stream::pending::<std::io::Result<Message>>(),
        FailedOutput,
        Duration::from_secs(3600),
        Duration::ZERO,
    )
    .map(|message| message.map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut)));
    let connection = serve_invocation_session_connection(
        socket,
        move |mut incoming, outbound, _budgets, setup_complete| async move {
            let _cancelled = cancelled.drop_guard();
            setup_complete.send(()).unwrap();
            outbound
                .send(Message::Text("output".into()), None)
                .await
                .unwrap();
            let _ = incoming.next().await;
        },
    );
    tokio::time::timeout(Duration::from_secs(1), connection)
        .await
        .expect("a failed output must terminate an input-dependent session");
    assert!(observed.is_cancelled());
}

#[derive(Clone, Default)]
struct PendingSetup {
    started: Arc<tokio::sync::Notify>,
    cancelled: CancellationToken,
    drained: CancellationToken,
    config: InvocationSessionConfig,
}

#[poem::handler]
async fn blocked_setup(websocket: WebSocket, state: Data<&PendingSetup>) -> impl IntoResponse {
    let state = state.0.clone();
    websocket.on_upgrade(move |socket| async move {
        let config = state.config.clone();
        serve_invocation_session_connection(
            invocation_session_websocket(socket, config),
            move |incoming, outbound, budgets, setup_complete| async move {
                let _cancelled = state.cancelled.drop_guard();
                let _held = (incoming, outbound, budgets, setup_complete);
                state.started.notify_one();
                std::future::pending::<()>().await;
            },
        )
        .await;
    })
}

async fn pending_setup_connection(
    config: InvocationSessionConfig,
) -> (
    PendingSetup,
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_util::task::AbortOnDropHandle<std::io::Result<()>>,
) {
    let state = PendingSetup {
        config,
        ..Default::default()
    };
    let (address, server) = server(blocked_setup.data(state.clone()).map_to_response()).await;
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    state.started.notified().await;
    (state, client, server)
}

#[test]
#[timeout("10s")]
async fn pong_timeout_cancels_pending_setup_and_closes_the_socket() {
    let (state, mut client, _server) = pending_setup_connection(InvocationSessionConfig {
        keep_alive_interval: INTERVAL,
        max_pong_timeout: INTERVAL,
    })
    .await;
    // Do not read the Ping until the server's Pong deadline has passed.
    tokio::time::timeout(Duration::from_secs(2), state.cancelled.cancelled())
        .await
        .expect("Pong timeout must cancel setup without waiting for executor acceptance");
    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(Ok(message)) = client.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await
    .expect("Pong timeout must close the socket without waiting for executor acceptance");
}

#[test]
#[timeout("10s")]
async fn client_close_cancels_pending_setup() {
    let (state, mut client, _server) = pending_setup_connection(InvocationSessionConfig {
        keep_alive_interval: INTERVAL,
        max_pong_timeout: INTERVAL,
    })
    .await;
    client.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), state.cancelled.cancelled())
        .await
        .expect("reader termination must cancel setup without waiting for executor acceptance");
}

#[test]
#[timeout("10s")]
async fn disabled_pong_deadline_keeps_sending_pings_without_closing_setup() {
    let (state, mut client, _server) = pending_setup_connection(InvocationSessionConfig {
        keep_alive_interval: INTERVAL,
        ..Default::default()
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !state.cancelled.is_cancelled(),
        "missing Pongs must not cancel setup when the deadline is disabled"
    );
    // Read the wire directly so tungstenite cannot send automatic Pongs between reads.
    let tokio_tungstenite::MaybeTlsStream::Plain(stream) = client.get_mut() else {
        unreachable!()
    };
    for _ in 0..3 {
        let mut frame = [0; 2];
        tokio::time::timeout(
            Duration::from_secs(1),
            tokio::io::AsyncReadExt::read_exact(stream, &mut frame),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            frame,
            [0x89, 0],
            "expected repeated empty Ping frames without any Pong"
        );
    }
    client.close(None).await.unwrap();
}

#[poem::handler]
async fn draining_session(websocket: WebSocket, state: Data<&PendingSetup>) -> impl IntoResponse {
    let state = state.0.clone();
    websocket.on_upgrade(move |socket| async move {
        serve_invocation_session_connection(
            invocation_session_websocket(socket, state.config.clone()),
            move |mut incoming, outbound, budgets, setup_complete| async move {
                let _held = (outbound, budgets);
                setup_complete.send(()).unwrap();
                state.started.notify_one();
                assert!(incoming.next().await.is_none());
                tokio::time::sleep(INTERVAL).await;
                state.drained.cancel();
            },
        )
        .await;
    })
}

#[test]
#[timeout("10s")]
async fn normal_client_close_waits_for_the_established_session_to_drain() {
    let state = PendingSetup::default();
    let (address, _server) = server(draining_session.data(state.clone()).map_to_response()).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    state.started.notified().await;
    client.close(None).await.unwrap();
    while let Some(Ok(message)) = client.next().await {
        if message.is_close() {
            break;
        }
    }
    assert!(
        state.drained.is_cancelled(),
        "the close handshake must wait for the private session drain"
    );
}

#[test]
#[timeout("10s")]
async fn pong_timeout_after_setup_waits_for_the_established_session_to_drain() {
    let state = PendingSetup {
        config: InvocationSessionConfig {
            keep_alive_interval: INTERVAL,
            max_pong_timeout: INTERVAL,
        },
        ..Default::default()
    };
    let (address, _server) = server(draining_session.data(state.clone()).map_to_response()).await;
    let (_client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    state.started.notified().await;
    tokio::time::timeout(Duration::from_secs(2), state.drained.cancelled())
        .await
        .expect("an established private session must drain after a read error");
}

#[derive(Clone)]
struct SlowOutput {
    ready: Arc<tokio::sync::Notify>,
    resume: CancellationToken,
    items: usize,
    bytes_per_item: usize,
}

#[poem::handler]
async fn buffered_output(websocket: WebSocket, state: Data<&SlowOutput>) -> impl IntoResponse {
    let state = state.0.clone();
    websocket
        .protocols([INVOCATION_SESSION_SUBPROTOCOL])
        .on_upgrade(move |socket| async move {
            serve_invocation_session_connection(
                invocation_session_websocket(
                    socket,
                    InvocationSessionConfig {
                        keep_alive_interval: INTERVAL,
                        ..Default::default()
                    },
                ),
                move |_incoming, outbound, _budgets, setup_complete| async move {
                    let _held = (_incoming, _budgets);
                    setup_complete.send(()).unwrap();
                    for index in 0..state.items {
                        let message = PublicServerMessage::OutputStreamItem {
                            channel: 1,
                            cursor_token: format!("cursor-{index}"),
                            mappings: vec![],
                            sequence: DecimalU64(index as u64),
                            value: serde_json::Value::String("x".repeat(state.bytes_per_item)),
                            version: 1,
                        };
                        outbound
                            .send(Message::Text(encode_text(&message).unwrap()), None)
                            .await
                            .unwrap();
                    }
                    state.ready.notify_one();
                    state.resume.cancelled().await;
                    let finished = PublicServerMessage::InvocationFinished {
                        outcome: PublicInvocationOutcome::Success,
                        version: 1,
                    };
                    outbound
                        .send(Message::Text(encode_text(&finished).unwrap()), None)
                        .await
                        .unwrap();
                },
            )
            .await;
        })
}

async fn slow_output_consumer_keeps_connection(items: usize, bytes_per_item: usize) {
    let state = SlowOutput {
        ready: Arc::new(tokio::sync::Notify::new()),
        resume: CancellationToken::new(),
        items,
        bytes_per_item,
    };
    let (address, _server) = server(buffered_output.data(state.clone()).map_to_response()).await;
    let mut client = InvocationSessionTransport::connect(format!("ws://{address}/"), None)
        .await
        .unwrap();
    state.ready.notified().await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    for index in 0..items {
        let frame = client.receive().await.unwrap();
        let ServerFrame::Message(PublicServerMessage::OutputStreamItem { sequence, .. }) =
            frame.frame()
        else {
            panic!("expected queued output stream item");
        };
        assert_eq!(sequence.0, index as u64);
    }
    state.resume.cancel();
    assert!(matches!(
        client.receive().await.unwrap().frame(),
        ServerFrame::Message(PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Success,
            ..
        })
    ));
}

#[test]
#[timeout("20s")]
async fn full_client_item_queue_does_not_trigger_a_pong_deadline() {
    // Exceed the client's 16-item queue with messages too small to exhaust its byte budget.
    slow_output_consumer_keeps_connection(64, 0).await;
}

#[test]
#[timeout("30s")]
async fn full_client_byte_budget_does_not_trigger_a_pong_deadline() {
    // Exceed the client's 32 MiB byte budget before its 16-item queue can fill.
    slow_output_consumer_keeps_connection(4, 12 * 1024 * 1024).await;
}

#[poem::handler]
async fn silent_invocation(websocket: WebSocket, enabled: Data<&bool>) -> impl IntoResponse {
    let enabled = *enabled.0;
    websocket
        .protocols([INVOCATION_SESSION_SUBPROTOCOL])
        .on_upgrade(move |socket| async move {
            let config = InvocationSessionConfig {
                keep_alive_interval: if enabled {
                    INTERVAL
                } else {
                    Duration::from_secs(60)
                },
                max_pong_timeout: IDLE_TIMEOUT,
            };
            let (mut sink, stream) = invocation_session_websocket(socket, config).split();
            let (incoming, _messages) = mpsc::channel(CHANNEL_CAPACITY);
            let _reader = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(read_websocket(
                stream,
                incoming,
                SessionBudgets::new(),
            )));
            tokio::time::sleep(SILENCE).await;
            let finished = PublicServerMessage::InvocationFinished {
                outcome: PublicInvocationOutcome::Success,
                version: 1,
            };
            let _ = sink
                .send(Message::Text(encode_text(&finished).unwrap()))
                .await;
        })
}

async fn invocation_through_proxy(enabled: bool) -> Result<ServerFrame, String> {
    let (address, _server) = server(silent_invocation.data(enabled).map_to_response()).await;
    let (proxy, _proxy) = idle_proxy(address).await;
    let mut client = InvocationSessionTransport::connect(format!("ws://{proxy}/"), None)
        .await
        .unwrap();
    // The socket task must answer Pings even when the caller does not poll receive().
    tokio::time::sleep(SILENCE + INTERVAL).await;
    client
        .receive()
        .await
        .map(|frame| frame.frame().clone())
        .map_err(|error| error.to_string())
}

#[test]
#[timeout("15s")]
async fn silent_invocation_without_keep_alive_loses_proxy_connection() {
    let result = invocation_through_proxy(false).await;
    assert!(
        result.is_err(),
        "the idle proxy must close a silent connection"
    );
    eprintln!("without WebSocket keep-alive: {}", result.unwrap_err());
}

#[test]
#[timeout("15s")]
async fn silent_invocation_with_keep_alive_completes_through_idle_proxy() {
    assert!(matches!(
        invocation_through_proxy(true).await.unwrap(),
        ServerFrame::Message(PublicServerMessage::InvocationFinished {
            outcome: PublicInvocationOutcome::Success,
            ..
        })
    ));
    eprintln!("with WebSocket keep-alive: invocation finished after 3s through a 1s-idle proxy");
}

#[poem::handler]
async fn missing_pong(
    websocket: WebSocket,
    observed: Data<&mpsc::Sender<std::io::ErrorKind>>,
) -> impl IntoResponse {
    let observed = observed.0.clone();
    websocket.on_upgrade(move |socket| async move {
        let mut socket = invocation_session_websocket(
            socket,
            InvocationSessionConfig {
                keep_alive_interval: INTERVAL,
                max_pong_timeout: INTERVAL,
            },
        );
        while let Some(message) = socket.next().await {
            if let Err(error) = message {
                observed.send(error.kind()).await.unwrap();
                return;
            }
        }
    })
}

#[test]
#[timeout("10s")]
async fn invocation_session_times_out_when_client_does_not_read_pings() {
    let (observed, mut errors) = mpsc::channel::<std::io::ErrorKind>(1);
    let (address, _server) = server(missing_pong.data(observed).map_to_response()).await;
    let (_client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    assert_eq!(errors.recv().await.unwrap(), std::io::ErrorKind::TimedOut);
}

#[test]
async fn prefetched_websocket_messages_retain_input_and_session_byte_budgets() {
    let budgets = SessionBudgets {
        input: Arc::new(Semaphore::new(128)),
        session: Arc::new(Semaphore::new(128)),
        ..SessionBudgets::new()
    };
    let (incoming, mut messages) = mpsc::channel(CHANNEL_CAPACITY);
    let stream = futures::stream::iter([Ok(Message::Text("test".to_string()))]);
    read_websocket(stream, incoming, budgets.clone())
        .await
        .unwrap();
    assert_eq!(budgets.input.available_permits(), 124);
    assert_eq!(budgets.session.available_permits(), 124);
    let message = messages.recv().await.unwrap().unwrap();
    assert_eq!(budgets.input.available_permits(), 124);
    assert_eq!(budgets.session.available_permits(), 124);
    drop(message);
    assert_eq!(budgets.input.available_permits(), 128);
    assert_eq!(budgets.session.available_permits(), 128);
}
