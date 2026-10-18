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

use super::proxy_worker_connection;
use crate::config::WebSocketTransportLimits;
use crate::transport_test_support::server;
use futures::{SinkExt, StreamExt};
use golem_common::model::AgentId;
use golem_common::model::component::ComponentId;
use golem_common::tracing::TraceOrigin;
use poem::web::Data;
use poem::web::websocket::WebSocket;
use poem::{EndpointExt, IntoResponse};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::Message as ClientMessage;
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data as FrameData, OpCode};
use tokio_util::task::AbortOnDropHandle;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const SMALL_LIMIT: usize = 1024;

fn small_limits() -> WebSocketTransportLimits {
    WebSocketTransportLimits {
        max_message_size: SMALL_LIMIT,
        max_frame_size: SMALL_LIMIT,
        ..Default::default()
    }
}

/// Serves `proxy_worker_connection` with a worker stream that sends no events.
#[poem::handler]
async fn connect(
    websocket: WebSocket,
    limits: Data<&WebSocketTransportLimits>,
) -> impl IntoResponse {
    websocket
        .config(limits.0.websocket_config())
        .on_upgrade(|socket| async move {
            let (sink, stream) = socket.split();
            let _ = proxy_worker_connection(
                AgentId {
                    component_id: ComponentId::new(),
                    agent_id: "agent".to_string(),
                },
                TraceOrigin::none(),
                futures::stream::pending(),
                sink,
                stream,
                Duration::from_secs(3600),
                Duration::ZERO,
            )
            .await;
        })
}

async fn open(
    limits: WebSocketTransportLimits,
) -> (Client, AbortOnDropHandle<std::io::Result<()>>) {
    let (address, server) = server(connect.data(limits).map_to_response()).await;
    let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    (client, server)
}

async fn expect_size_close(client: &mut Client) {
    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("the proxy must close the connection")
        .expect("the proxy must send a close frame before it ends the connection")
        .expect("the proxy must send a close frame rather than reset the connection");
    let ClientMessage::Close(Some(frame)) = message else {
        panic!("expected close 1009, got {message:?}");
    };
    assert_eq!(u16::from(frame.code), 1009);
}

#[test]
#[timeout("10s")]
async fn frame_header_above_the_default_limit_gets_close_1009() {
    let limits = WebSocketTransportLimits::default();
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&((limits.max_frame_size + 1) as u64).to_be_bytes());
    header.extend_from_slice(&[0; 4]);
    let (mut client, _server) = open(limits).await;
    let tokio_tungstenite::MaybeTlsStream::Plain(stream) = client.get_mut() else {
        unreachable!()
    };

    stream.write_all(&header).await.unwrap();

    expect_size_close(&mut client).await;
}

#[test]
#[timeout("10s")]
async fn fragmented_message_above_the_limit_gets_close_1009() {
    let (mut client, _server) = open(small_limits()).await;

    for (payload, opcode, is_final) in [
        (SMALL_LIMIT / 2, OpCode::Data(FrameData::Binary), false),
        (SMALL_LIMIT / 2 + 1, OpCode::Data(FrameData::Continue), true),
    ] {
        client
            .send(ClientMessage::Frame(Frame::message(
                vec![0; payload],
                opcode,
                is_final,
            )))
            .await
            .unwrap();
    }

    expect_size_close(&mut client).await;
}

#[test]
#[timeout("10s")]
async fn message_at_the_limit_keeps_the_connection_open() {
    let (mut client, _server) = open(small_limits()).await;

    client
        .send(ClientMessage::Binary(vec![0; SMALL_LIMIT].into()))
        .await
        .unwrap();
    client
        .send(ClientMessage::Ping(b"open".to_vec().into()))
        .await
        .unwrap();

    let message = tokio::time::timeout(Duration::from_secs(5), client.next())
        .await
        .expect("the proxy must answer the Ping")
        .unwrap()
        .unwrap();
    assert_eq!(message, ClientMessage::Pong(b"open".to_vec().into()));
}
