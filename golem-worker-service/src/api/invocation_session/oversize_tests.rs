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
use crate::api::agents::invocation_session_websocket_config;
use crate::transport_test_support::server;
use poem::web::Data;
use poem::web::websocket::WebSocket;
use poem::{EndpointExt, IntoResponse};
use std::time::Duration;
use test_r::{test, timeout};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::{
    Message as ClientMessage,
    protocol::frame::{
        Frame,
        coding::{Data as FrameData, OpCode},
    },
};

#[test]
fn erased_poem_capacity_error_is_classified_without_relabeling_other_errors() {
    use tokio_tungstenite::tungstenite::{Error, error::CapacityError};
    let capacity = Error::Capacity(CapacityError::MessageTooLong {
        size: MAX_WEBSOCKET_MESSAGE_SIZE + 1,
        max_size: MAX_WEBSOCKET_MESSAGE_SIZE,
    });
    let mapped = invocation_session_websocket_error(KeepAliveError::Stream(std::io::Error::other(
        capacity.to_string(),
    )));
    assert!(is_message_too_large(&mapped));
    for diagnostic in [
        "Space limit exceeded: Too many headers",
        "WebSocket protocol error: invalid opcode",
    ] {
        let mapped = invocation_session_websocket_error(KeepAliveError::Stream(
            std::io::Error::other(diagnostic),
        ));
        assert!(!is_message_too_large(&mapped));
        assert_eq!(mapped.to_string(), diagnostic);
    }
}

#[test]
#[timeout("5s")]
async fn impossible_input_credit_is_rejected_without_waiting() {
    let budgets = SessionBudgets::new();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        budgets.admit_inbound_frame(INPUT_BYTE_BUDGET + 1),
    )
    .await
    .expect("impossible credit must fail before the admission timeout");
    assert!(result.is_err());
    assert_eq!(budgets.input.available_permits(), INPUT_BYTE_BUDGET);
    assert_eq!(budgets.session.available_permits(), SESSION_BYTE_BUDGET);
}

#[test]
#[timeout("5s")]
async fn oversized_message_is_rejected_before_input_credit_admission() {
    let budgets = SessionBudgets::new();
    let (incoming, mut messages) = mpsc::channel(CHANNEL_CAPACITY);
    let wire =
        futures::stream::iter([Ok(Message::Binary(vec![0; MAX_WEBSOCKET_MESSAGE_SIZE + 1]))]);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        read_websocket(wire, incoming, budgets.clone()),
    )
    .await
    .expect("oversized input must not wait for impossible byte credit");
    let error = result.expect_err("oversized input must be rejected");
    assert!(is_message_too_large(&error));
    assert_eq!(budgets.input.available_permits(), INPUT_BYTE_BUDGET);
    assert_eq!(budgets.session.available_permits(), SESSION_BYTE_BUDGET);
    assert!(messages.try_recv().is_err());
}

#[poem::handler]
async fn size_limited_session(
    websocket: WebSocket,
    blocked_setup: Data<&bool>,
) -> impl IntoResponse {
    let blocked_setup = *blocked_setup.0;
    websocket
        .config(invocation_session_websocket_config())
        .on_upgrade(move |socket| async move {
            let websocket = invocation_session_websocket(
                socket,
                InvocationSessionConfig {
                    keep_alive_interval: Duration::from_secs(3600),
                    ..Default::default()
                },
            );
            serve_invocation_session_connection(
                websocket,
                |mut incoming, outbound, budgets, setup_complete| async move {
                    let _held = (outbound, budgets, setup_complete);
                    if blocked_setup {
                        std::future::pending::<()>().await;
                    }
                    let _ = incoming.next().await;
                },
            )
            .await;
        })
}

async fn expect_size_close(
    client: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) {
    let message = tokio::time::timeout(Duration::from_secs(2), client.next())
        .await
        .expect("oversized WebSocket input must receive a prompt size-error Close")
        .expect("server must send a Close frame before ending the connection")
        .expect("server must send a Close frame rather than reset the connection");
    let ClientMessage::Close(Some(frame)) = message else {
        panic!("expected Close 1009, got {message:?}");
    };
    assert_eq!(u16::from(frame.code), 1009);
}

#[test]
#[timeout("20s")]
async fn fragmented_oversized_message_gets_close_1009() {
    let (address, _server) = server(size_limited_session.data(false).map_to_response()).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    let first = MAX_WEBSOCKET_MESSAGE_SIZE / 2;
    client
        .send(ClientMessage::Frame(Frame::message(
            vec![0; first],
            OpCode::Data(FrameData::Binary),
            false,
        )))
        .await
        .unwrap();
    client
        .send(ClientMessage::Frame(Frame::message(
            vec![0; MAX_WEBSOCKET_MESSAGE_SIZE - first + 1],
            OpCode::Data(FrameData::Continue),
            true,
        )))
        .await
        .unwrap();
    expect_size_close(&mut client).await;
}

#[test]
#[timeout("10s")]
async fn oversized_frame_header_gets_close_1009_without_payload() {
    oversized_frame_header_close(false).await;
}

async fn oversized_frame_header_close(blocked_setup: bool) {
    let (address, _server) =
        server(size_limited_session.data(blocked_setup).map_to_response()).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/"))
        .await
        .unwrap();
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&((MAX_WEBSOCKET_MESSAGE_SIZE + 1) as u64).to_be_bytes());
    header.extend_from_slice(&[0; 4]);
    let tokio_tungstenite::MaybeTlsStream::Plain(stream) = client.get_mut() else {
        unreachable!()
    };
    stream.write_all(&header).await.unwrap();
    expect_size_close(&mut client).await;
}

#[test]
#[timeout("10s")]
async fn oversized_frame_gets_close_1009_while_setup_is_blocked() {
    oversized_frame_header_close(true).await;
}
