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

use super::AgentsApi;
use crate::config::{
    InvocationSessionConfig, InvocationSessionTokenConfig, WebSocketTransportLimits,
};
use crate::invocation_session_token::InvocationSessionTokenKeyring;
use crate::service::auth::{AuthService, AuthServiceError};
use async_trait::async_trait;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::auth::TokenSecret;
use golem_common::model::invocation_session_public::{
    INVOCATION_SESSION_SUBPROTOCOL, MAX_WEBSOCKET_MESSAGE_SIZE,
};
use golem_common::schema::{AgentConstructorSchema, InputSchema, SchemaValue};
use golem_service_base::http_server;
use golem_service_base::model::auth::AuthCtx;
use poem::listener::TcpAcceptor;
use poem_openapi::OpenApiService;
use std::net::SocketAddr;
use std::sync::Arc;
use test_r::{test, timeout};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// WebSocket close code for a message that is too big to process (RFC 6455, section 7.4.1).
const CLOSE_MESSAGE_TOO_BIG: u16 = 1009;
const CLOSE_PROTOCOL_ERROR: u16 = 1002;

struct SystemAuthService;

#[async_trait]
impl AuthService for SystemAuthService {
    async fn authenticate_token(&self, _: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        Ok(AuthCtx::system())
    }
}

async fn spawn_agents_api(
    limits: WebSocketTransportLimits,
) -> (SocketAddr, tokio::task::JoinHandle<std::io::Result<()>>) {
    let harness = crate::mcp::InvocationHarness::new(
        AgentInvocationOutput {
            result: golem_common::model::AgentInvocationResult::AgentMethod {
                output: SchemaValue::Tuple { elements: vec![] },
            },
            consumed_fuel: None,
            invocation_status: None,
            component_revision: None,
            agent_id: None,
            idempotency_key: None,
            oplog_index: None,
            agent_fingerprint: None,
        },
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(vec![]),
        },
        vec![],
    );
    let api = AgentsApi::new(
        harness.worker_service.clone(),
        Arc::new(SystemAuthService),
        Arc::new(
            InvocationSessionTokenKeyring::new(&InvocationSessionTokenConfig::default()).unwrap(),
        ),
        InvocationSessionConfig {
            websocket: limits,
            ..Default::default()
        },
    );
    let service = OpenApiService::new(api, "test", "1.0");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = TcpAcceptor::from_tokio(listener).unwrap();
    (
        address,
        tokio::spawn(http_server::run(acceptor, service, Default::default())),
    )
}

async fn open_invocation_session(address: SocketAddr) -> TcpStream {
    let mut socket = TcpStream::connect(address).await.unwrap();
    let handshake = format!(
        "GET /v1/agents/invoke-agent-session HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: {INVOCATION_SESSION_SUBPROTOCOL}\r\nAuthorization: Bearer lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28\r\n\r\n"
    );
    socket.write_all(handshake.as_bytes()).await.unwrap();
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        headers.push(socket.read_u8().await.unwrap());
    }
    assert!(
        headers.starts_with(b"HTTP/1.1 101 "),
        "{}",
        String::from_utf8_lossy(&headers)
    );
    socket
}

/// Sends the header of one masked text frame. A zero masking key leaves the payload unchanged.
async fn send_text_frame_header(socket: &mut TcpStream, size: usize) {
    let mut header = vec![0x81, 0x80 | 127];
    header.extend_from_slice(&(size as u64).to_be_bytes());
    header.extend_from_slice(&[0; 4]);
    socket.write_all(&header).await.unwrap();
}

/// Reads frames until a close frame, and returns its close code.
async fn read_close_code(socket: &mut TcpStream) -> u16 {
    loop {
        let opcode = socket.read_u8().await.unwrap() & 0x0f;
        let length = match socket.read_u8().await.unwrap() & 0x7f {
            126 => socket.read_u16().await.unwrap() as usize,
            127 => socket.read_u64().await.unwrap() as usize,
            length => length as usize,
        };
        let mut payload = vec![0; length];
        socket.read_exact(&mut payload).await.unwrap();
        if opcode == 0x8 {
            return u16::from_be_bytes([payload[0], payload[1]]);
        }
    }
}

#[test]
#[timeout("60s")]
async fn message_above_the_session_limit_gets_close_code_1009() {
    let (address, server) = spawn_agents_api(WebSocketTransportLimits::default()).await;
    let mut socket = open_invocation_session(address).await;

    send_text_frame_header(&mut socket, MAX_WEBSOCKET_MESSAGE_SIZE + 1).await;

    assert_eq!(read_close_code(&mut socket).await, CLOSE_MESSAGE_TOO_BIG);
    server.abort();
}

#[test]
#[timeout("60s")]
async fn message_at_the_session_limit_reaches_the_session() {
    let (address, server) = spawn_agents_api(WebSocketTransportLimits::default()).await;
    let mut socket = open_invocation_session(address).await;

    send_text_frame_header(&mut socket, MAX_WEBSOCKET_MESSAGE_SIZE).await;
    socket
        .write_all(&vec![b'x'; MAX_WEBSOCKET_MESSAGE_SIZE])
        .await
        .unwrap();

    // The session decodes the message, which is not valid JSON.
    assert_eq!(read_close_code(&mut socket).await, CLOSE_PROTOCOL_ERROR);
    server.abort();
}
