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

//! One `run` call through the live session, which is what lets Ctrl+C stop a command.

use crate::command_handler::agent::invocation_session::CliSessionRequestProvider;
use crate::command_handler::tool::{ToolOwner, public_target, public_typed_value};
use crate::context::Context;
use golem_client::invocation_session::{
    InvocationSession, InvocationSessionRequestProvider, InvocationSessionStateSnapshot,
    SessionTransportError, drive_native_tool_session_with_stop,
};
use golem_common::model::IdempotencyKey;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_VERSION, PublicClientMessage, PublicInvocationResult,
};
use golem_common::schema::ExternalTypedSchemaValue;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;
use tokio_tungstenite::tungstenite;

/// How long a call may take to reach the server. Opening a session is tried again for as long
/// as the server cannot be reached, and a call that has not reached it has not run.
const OPEN_WAIT: Duration = Duration::from_secs(10);

/// An input that never has data and never ends: the stdin stays open until it is cancelled.
struct OpenInput;

impl tokio::io::AsyncRead for OpenInput {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
}

/// Runs one `run` call through the live session with an open, empty stdin. `stop` cancels the
/// stdin, which asks the tool to stop the script; `give_up` leaves the call running and
/// returns the transport's `Interrupted` error.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_live(
    ctx: &Arc<Context>,
    owner: &ToolOwner,
    tool: &str,
    command_path: Vec<String>,
    input: ExternalTypedSchemaValue,
    key: &IdempotencyKey,
    stop: impl Future<Output = ()>,
    give_up: impl Future<Output = ()>,
) -> Result<PublicInvocationResult, SessionTransportError> {
    let input = public_typed_value(Some(input))
        .map_err(|error| SessionTransportError::Protocol(format!("{error:#}")))?;
    let initial = InvocationSessionStateSnapshot {
        delivered_output_cursors: Default::default(),
        stable_stream_bindings: Default::default(),
        pending_operation: Some(PublicClientMessage::ToolStart {
            attempt_id: uuid::Uuid::new_v4(),
            application: owner.application_name.to_string(),
            environment: owner.environment_name.to_string(),
            idempotency_key: key.value.clone(),
            tool_name: tool.to_string(),
            command_path,
            target: public_target(owner),
            input: Box::new(input),
            stdin: true,
            stdout: false,
            stderr: false,
            version: INVOCATION_SESSION_VERSION,
        }),
        session_token: None,
    };
    let requests = Arc::new(CliSessionRequestProvider::new(ctx.clone()));
    open_and_drive(requests, initial, OPEN_WAIT, stop, give_up).await
}

/// Opens the session for `initial` within `open_wait` and drives the call to its result. A
/// connection lost after that is the session's to find again, for as long as it takes.
async fn open_and_drive(
    requests: Arc<dyn InvocationSessionRequestProvider>,
    initial: InvocationSessionStateSnapshot,
    open_wait: Duration,
    stop: impl Future<Output = ()>,
    give_up: impl Future<Output = ()>,
) -> Result<PublicInvocationResult, SessionTransportError> {
    tokio::pin!(give_up);
    let session = tokio::select! {
        session = InvocationSession::open(requests, None, initial, false, Arc::new(())) => session?,
        _ = tokio::time::sleep(open_wait) => {
            return Err(SessionTransportError::Connect(tungstenite::Error::Io(
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "the server could not be reached in {} seconds",
                        open_wait.as_secs()
                    ),
                ),
            )));
        }
        // The session that would be told to give up is not there yet.
        _ = &mut give_up => {
            return Err(SessionTransportError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "the call was given up before it was connected",
            )));
        }
    };
    drive_native_tool_session_with_stop(
        session,
        Some(OpenInput),
        &mut tokio::io::sink(),
        &mut tokio::io::sink(),
        give_up,
        stop,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{OpenInput, open_and_drive};
    use golem_client::invocation_session::{
        InvocationSessionRequestProvider, InvocationSessionStateSnapshot, SessionTransportError,
    };
    use golem_common::model::invocation_session_public::{
        INVOCATION_SESSION_VERSION, PublicClientMessage, PublicInvocationResult,
        PublicNativeToolTarget, PublicTypedValue,
    };
    use golem_common::schema::{SchemaGraph, SchemaType};
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use tokio::io::AsyncReadExt;
    use tokio_tungstenite::tungstenite;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    #[test]
    async fn the_open_input_has_no_data_and_does_not_end() {
        let mut input = OpenInput;
        let mut byte = [0];
        let read = tokio::time::timeout(Duration::from_millis(50), input.read(&mut byte)).await;
        assert!(read.is_err(), "the input ended or had data: {read:?}");
    }

    /// Asks for a session at an address where nothing listens.
    struct Unreachable(SocketAddr);

    #[async_trait::async_trait]
    impl InvocationSessionRequestProvider for Unreachable {
        async fn request(&self) -> Result<tungstenite::http::Request<()>, SessionTransportError> {
            format!("ws://{}", self.0)
                .into_client_request()
                .map_err(SessionTransportError::Request)
        }
    }

    /// Opens and drives a call to a server that cannot be reached, for at most five seconds.
    async fn call_an_unreachable_server(
        open_wait: Duration,
        give_up: impl Future<Output = ()>,
    ) -> Result<PublicInvocationResult, SessionTransportError> {
        // The port is free again once its listener is gone.
        let address = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let initial = InvocationSessionStateSnapshot {
            delivered_output_cursors: Default::default(),
            stable_stream_bindings: Default::default(),
            pending_operation: Some(PublicClientMessage::ToolStart {
                attempt_id: uuid::Uuid::new_v4(),
                application: "app".to_string(),
                environment: "env".to_string(),
                idempotency_key: "key".to_string(),
                tool_name: "bash".to_string(),
                command_path: vec!["run".to_string()],
                target: PublicNativeToolTarget::Component {
                    component_id: uuid::Uuid::new_v4(),
                },
                input: Box::new(PublicTypedValue {
                    graph: SchemaGraph::anonymous(SchemaType::string()),
                    value: serde_json::json!({"kind": "string", "value": "true"}),
                }),
                stdin: true,
                stdout: false,
                stderr: false,
                version: INVOCATION_SESSION_VERSION,
            }),
            session_token: None,
        };
        let call = open_and_drive(
            Arc::new(Unreachable(address)),
            initial,
            open_wait,
            std::future::pending(),
            give_up,
        );
        tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("the call to a server that cannot be reached did not end")
    }

    #[test]
    async fn a_call_that_cannot_reach_the_server_ends_when_the_time_to_open_is_over() {
        let error = call_an_unreachable_server(Duration::from_millis(300), std::future::pending())
            .await
            .unwrap_err();
        assert!(
            matches!(error, SessionTransportError::Connect(_)),
            "{error:?}"
        );
        assert!(
            error
                .to_string()
                .contains("the server could not be reached"),
            "{error}"
        );
    }

    #[test]
    async fn giving_up_ends_a_call_that_is_still_connecting() {
        let error = call_an_unreachable_server(Duration::from_secs(60), std::future::ready(()))
            .await
            .unwrap_err();
        assert!(
            matches!(error, SessionTransportError::Io(ref error) if error.kind() == std::io::ErrorKind::Interrupted),
            "{error:?}"
        );
    }
}
