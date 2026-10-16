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
use crate::transport_test_support::INTERVAL;
use std::time::Duration;
use test_r::{test, timeout};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::PollSender;

async fn delayed_pong(pong_timeout: Duration, byte_backpressure: bool) -> std::io::Result<()> {
    let (peer, received) = mpsc::channel::<std::io::Result<Message>>(4);
    let (pings, mut sent) = mpsc::channel(4);
    let sink = PollSender::new(pings)
        .sink_map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe));
    let websocket = WebSocketKeepAlive::from_sink_and_stream(
        ReceiverStream::new(received),
        sink,
        INTERVAL,
        pong_timeout,
    )
    .map(|result| {
        result.map_err(|error| match error {
            KeepAliveError::Sink(error) | KeepAliveError::Stream(error) => error,
            KeepAliveError::Timeout => std::io::Error::from(std::io::ErrorKind::TimedOut),
        })
    });
    let bytes = Arc::new(Semaphore::new(if byte_backpressure { 1 } else { 128 }));
    let (incoming, mut messages) = mpsc::channel(if byte_backpressure {
        CHANNEL_CAPACITY
    } else {
        1
    });
    let reader = read_websocket(websocket, incoming, bytes.clone());
    futures::pin_mut!(reader);
    let ping = tokio::select! {
        ping = sent.recv() => ping.unwrap(),
        _ = &mut reader => panic!("reader ended before sending its Ping"),
    };
    let Message::Ping(payload) = ping else {
        panic!("expected Ping");
    };
    peer.send(Ok(Message::Text("a".into()))).await.unwrap();
    peer.send(Ok(Message::Text("b".into()))).await.unwrap();
    assert!(futures::poll!(&mut reader).is_pending());
    assert_eq!(messages.len(), 1);
    if byte_backpressure {
        assert_eq!(bytes.available_permits(), 0);
    }
    // The live peer answers only after the server's application admission is blocked.
    peer.send(Ok(Message::Pong(payload))).await.unwrap();
    peer.send(Ok(Message::Close(None))).await.unwrap();
    tokio::time::sleep(INTERVAL * 2).await;
    drop(messages.recv().await.unwrap());
    tokio::time::timeout(Duration::from_secs(1), &mut reader)
        .await
        .unwrap()
}

#[test]
#[timeout("5s")]
async fn strict_pong_deadline_can_expire_behind_a_full_server_input_queue() {
    let error = delayed_pong(INTERVAL, false).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
#[timeout("5s")]
async fn strict_pong_deadline_can_expire_behind_a_full_server_input_byte_budget() {
    let error = delayed_pong(INTERVAL, true).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
#[timeout("5s")]
async fn keep_alive_only_resumes_a_pong_behind_a_full_server_input_queue() {
    delayed_pong(Duration::ZERO, false).await.unwrap();
}

#[test]
#[timeout("5s")]
async fn keep_alive_only_resumes_a_pong_behind_a_full_server_input_byte_budget() {
    delayed_pong(Duration::ZERO, true).await.unwrap();
}
