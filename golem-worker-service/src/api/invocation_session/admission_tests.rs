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
use golem_common::model::invocation_session_public::decode_server_text;
use std::time::Duration;
use test_r::{test, timeout};
use tokio_stream::wrappers::ReceiverStream;

async fn reject_initial_with_saturated_data_budgets(first: Message) -> Message {
    let bytes = message_size(&first);
    let budgets = SessionBudgets {
        session: Arc::new(Semaphore::new(2 * bytes)),
        ..SessionBudgets::new()
    };
    let _output = acquire_bytes(&budgets.output, OUTPUT_BYTE_BUDGET)
        .await
        .unwrap();
    let (wire, received) = mpsc::channel(2);
    wire.send(Ok(first)).await.unwrap();
    wire.send(Ok(Message::Binary(vec![0; bytes])))
        .await
        .unwrap();
    let (incoming, messages) = mpsc::channel(CHANNEL_CAPACITY);
    let reader = read_websocket(
        ReceiverStream::new(received),
        incoming,
        budgets.session.clone(),
    );
    futures::pin_mut!(reader);
    assert!(futures::poll!(&mut reader).is_pending());
    assert_eq!(budgets.session.available_permits(), 0);
    assert_eq!(budgets.output.available_permits(), 0);

    let (sender, mut outgoing) = mpsc::channel(CHANNEL_CAPACITY);
    let outbound = Outbound { sender, budgets };
    let keyring = InvocationSessionTokenKeyring::new(&Default::default()).unwrap();
    let bindings = token_bindings(&AuthCtx::system());
    let mut messages = ReceiverStream::new(messages);
    let rejected = tokio::time::timeout(
        Duration::from_secs(1),
        receive_initial(&mut messages, &outbound, &keyring, &bindings),
    )
    .await
    .expect("prefetched input must not block protocol rejection writes");
    assert!(rejected.is_err());
    outgoing
        .try_recv()
        .expect("protocol rejection must be queued without releasing input")
        .message
}

#[test]
#[timeout("5s")]
async fn protocol_close_bypasses_saturated_input_and_output_byte_budgets() {
    let message = reject_initial_with_saturated_data_budgets(Message::Binary(vec![0; 256])).await;
    let Message::Close(Some((code, reason))) = message else {
        panic!("expected protocol-error Close");
    };
    assert_eq!(code, CloseCode::Protocol);
    assert_eq!(reason, "first application message must be text");
}

#[test]
#[timeout("5s")]
async fn invocation_rejection_bypasses_saturated_input_and_output_byte_budgets() {
    let resume = PublicClientMessage::ResumeAttach {
        attempt_id: Uuid::new_v4(),
        operation: PublicResumeOperation::Resume,
        output_cursors: vec![],
        session_token: "invalid-session-token".to_string(),
        version: 1,
    };
    let message =
        reject_initial_with_saturated_data_budgets(Message::Text(encode_text(&resume).unwrap()))
            .await;
    let Message::Text(text) = message else {
        panic!("expected invocation rejection");
    };
    assert!(matches!(
        decode_server_text(text.as_bytes()).unwrap(),
        PublicServerMessage::InvocationRejected { .. }
    ));
}

#[test]
async fn oversized_rejection_keeps_its_identity_with_a_bounded_safe_message() {
    let budgets = SessionBudgets::new();
    let (sender, mut outgoing) = mpsc::channel(CHANNEL_CAPACITY);
    let outbound = Outbound {
        sender,
        budgets: budgets.clone(),
    };
    let attempt_id = Uuid::new_v4();
    send_text(
        &outbound,
        &PublicServerMessage::InvocationRejected {
            attempt_id: Some(attempt_id),
            code: PublicErrorCode::ProtocolError,
            message: "x".repeat(CONTROL_BYTE_BUDGET),
            retryable: true,
            version: 1,
        },
    )
    .await
    .unwrap();
    let queued = outgoing.recv().await.unwrap();
    assert!(message_size(&queued.message) <= CONTROL_BYTE_BUDGET);
    let Message::Text(text) = &queued.message else {
        panic!("expected rejection text");
    };
    assert_eq!(
        decode_server_text(text.as_bytes()).unwrap(),
        PublicServerMessage::InvocationRejected {
            attempt_id: Some(attempt_id),
            code: PublicErrorCode::ProtocolError,
            message: safe_rejection_message(PublicErrorCode::ProtocolError),
            retryable: true,
            version: 1,
        }
    );
    assert!(budgets.control.available_permits() < CONTROL_BYTE_BUDGET);
    drop(queued);
    assert_eq!(budgets.control.available_permits(), CONTROL_BYTE_BUDGET);
}
