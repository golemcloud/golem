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
                Arc::new(Semaphore::new(SESSION_BYTE_BUDGET)),
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
async fn prefetched_websocket_messages_retain_the_session_byte_budget() {
    let budget = Arc::new(Semaphore::new(128));
    let (incoming, mut messages) = mpsc::channel(CHANNEL_CAPACITY);
    let stream = futures::stream::iter([Ok(Message::Text("test".to_string()))]);
    read_websocket(stream, incoming, budget.clone()).await;
    assert_eq!(budget.available_permits(), 124);
    let message = messages.recv().await.unwrap().unwrap();
    assert_eq!(budget.available_permits(), 124);
    drop(message);
    assert_eq!(budget.available_permits(), 128);
}
