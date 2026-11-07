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

use crate::command_handler::agent::parse_worker_error;
use crate::command_handler::agent::stream_output::AgentStreamOutput;
use crate::model::agent::AgentLogStreamOptions;
use crate::model::format::Format;
use anyhow::{Context, anyhow};
use bytes::Bytes;
use futures_util::future::Either;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt, future};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::ComponentId;
use golem_common::model::{AgentEvent, AgentId, LogLevel, Timestamp};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio::{task, time};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream, connect_async_tls_with_config, tungstenite,
};
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, error, info, trace};
use url::Url;
use uuid::Uuid;

#[derive(Debug)]
struct InsecureServerCertVerifier {
    signature_algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.signature_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.signature_algorithms.supported_schemes()
    }
}

pub(crate) fn insecure_connector() -> anyhow::Result<Connector> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = InsecureServerCertVerifier {
        signature_algorithms: provider.signature_verification_algorithms,
    };
    let tls_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Connector::Rustls(Arc::new(tls_config)))
}

/// The invocation that a connection shows and whose `InvocationFinished` ends it. The caller sets
/// the target when the server returns the accepted invocation ID, which can be after the
/// invocation finished. The target's `InvocationFinished` is shown only after the caller has
/// written its own output of the invocation, such as streamed result values.
#[derive(Clone)]
pub struct InvocationGoal {
    target: watch::Sender<Option<Uuid>>,
    output_written: watch::Sender<bool>,
    reached: Arc<AtomicBool>,
}

impl InvocationGoal {
    fn new() -> Self {
        Self {
            target: watch::Sender::new(None),
            output_written: watch::Sender::new(false),
            reached: Arc::default(),
        }
    }

    pub fn set_target(&self, invocation_id: Uuid) {
        self.target.send_replace(Some(invocation_id));
    }

    pub fn set_output_written(&self) {
        self.output_written.send_replace(true);
    }

    fn is_reached(&self) -> bool {
        self.reached.load(Ordering::Acquire)
    }
}

/// Selects the events of one invocation. A new connection first receives the agent's recent
/// event history, so until the target is known the filter holds events per invocation.
#[derive(Default)]
struct TargetFilter {
    target: Option<Uuid>,
    held: HashMap<Uuid, Vec<AgentEvent>>,
}

impl TargetFilter {
    /// Returns the events to show now.
    fn accept(&mut self, event: AgentEvent) -> Vec<AgentEvent> {
        if let AgentEvent::ClientLagged { .. } = event {
            return vec![event];
        }
        let Some(invocation_id) = event_invocation_id(&event) else {
            return Vec::new();
        };
        match self.target {
            Some(target) if target == invocation_id => vec![event],
            Some(_) => Vec::new(),
            None => {
                self.held.entry(invocation_id).or_default().push(event);
                Vec::new()
            }
        }
    }

    /// Sets the target and returns its held events.
    fn set_target(&mut self, target: Uuid) -> Vec<AgentEvent> {
        self.target = Some(target);
        let events = self.held.remove(&target).unwrap_or_default();
        self.held.clear();
        events
    }
}

fn event_invocation_id(event: &AgentEvent) -> Option<Uuid> {
    match event {
        AgentEvent::StdOut { invocation_id, .. }
        | AgentEvent::StdErr { invocation_id, .. }
        | AgentEvent::Log { invocation_id, .. } => *invocation_id,
        AgentEvent::InvocationStart { invocation_id, .. }
        | AgentEvent::InvocationFinished { invocation_id, .. } => Some(*invocation_id),
        AgentEvent::PluginError { .. }
        | AgentEvent::SnapshotRecoverySucceeded { .. }
        | AgentEvent::SnapshotRecoveryFailed { .. }
        | AgentEvent::ClientLagged { .. } => None,
    }
}

pub struct AgentConnection {
    request: Request,
    connector: Option<Connector>,
    output: AgentStreamOutput,
    agent_name: String,
    goal: Option<InvocationGoal>,
    ping_interval: Duration,
}

impl AgentConnection {
    /// Initializes a worker stream. Use `run_forever` to connect and output worker events.
    pub async fn new(
        worker_service_url: Url,
        auth_token: TokenSecret,
        component_id: &ComponentId,
        agent_id: String,
        connect_options: AgentLogStreamOptions,
        allow_insecure: bool,
        format: Format,
        ping_interval: Duration,
        wait_for_invocation: bool,
    ) -> anyhow::Result<AgentConnection> {
        let (request, connector) = Self::create_request(
            worker_service_url,
            auth_token.secret().to_string(),
            component_id.0,
            agent_id.clone(),
            allow_insecure,
        )?;
        let output = AgentStreamOutput::new(connect_options, format);

        Ok(Self {
            request,
            connector,
            output,
            agent_name: agent_id,
            goal: wait_for_invocation.then(InvocationGoal::new),
            ping_interval,
        })
    }

    /// The goal of a connection created with `wait_for_invocation`.
    pub fn invocation_goal(&self) -> Option<InvocationGoal> {
        self.goal.clone()
    }

    /// Creates a new worker connection and every time the connection is dropped tries to
    /// reconnect. The loop exits when the invocation goal is reached.
    pub async fn run_forever(self) -> anyhow::Result<()> {
        while !self.goal.as_ref().is_some_and(InvocationGoal::is_reached) {
            if let Err(error) = self.run().await
                && error
                    .downcast_ref::<crate::error::service::ServiceError>()
                    .is_some_and(|error| error.is_previous_invocation_failed())
            {
                return Err(error);
            }
            self.output.flush().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    /// Connects to the worker event stream and outputs incoming messages until the connection is dropped
    async fn run(&self) -> anyhow::Result<()> {
        let (ws_stream, _) = connect_async_tls_with_config(
            self.request.clone(),
            None,
            false,
            self.connector.clone(),
        )
        .await
        .map_err(|e| match e {
            tungstenite::error::Error::Http(http_error_response) => {
                let status = http_error_response.status().as_u16();
                match http_error_response.body().clone() {
                    Some(body) => anyhow!(parse_worker_error(status, body)),
                    None => anyhow!("Websocket connect failed, HTTP error: {}", status),
                }
            }
            _ => anyhow!("Websocket connect failed, error: {}", e),
        })?;

        let (write, read) = ws_stream.split();

        let ping_interval = self.ping_interval;
        let pings = AbortOnDropHandle::new(task::spawn(async move {
            Self::ping_loop(write, ping_interval).await
        }));

        let output = self.output.clone();
        let agent_name = self.agent_name.clone();
        let goal = self.goal.clone();
        let read_messages = AbortOnDropHandle::new(task::spawn(async move {
            Self::read_loop(read, output, agent_name, goal).await;
        }));

        // Both tasks end with this connection, also when the caller drops it: a detached read
        // loop of a dropped connection would show events again after the reconnect.
        match future::select(pings, read_messages).await {
            Either::Left((Ok(result), _)) => Err(result),
            Either::Left((Err(err), _)) => Err(anyhow!(err)),
            Either::Right((Ok(_), _)) => Ok(()),
            Either::Right((Err(err), _)) => Err(anyhow!(err)),
        }
    }

    async fn emit(output: &AgentStreamOutput, agent_name: &str, event: AgentEvent) {
        let display_id = |invocation_id: Uuid| format!("{agent_name}.{invocation_id}");
        match event {
            AgentEvent::StdOut {
                timestamp,
                bytes,
                invocation_id,
            } => {
                output
                    .emit_stdout(
                        timestamp,
                        String::from_utf8_lossy(&bytes).to_string(),
                        invocation_id.map(display_id),
                    )
                    .await;
            }
            AgentEvent::StdErr {
                timestamp,
                bytes,
                invocation_id,
            } => {
                output
                    .emit_stderr(
                        timestamp,
                        String::from_utf8_lossy(&bytes).to_string(),
                        invocation_id.map(display_id),
                    )
                    .await;
            }
            AgentEvent::Log {
                timestamp,
                level,
                context,
                message,
                invocation_id,
            } => {
                output
                    .emit_log(
                        timestamp,
                        level,
                        context,
                        message,
                        invocation_id.map(display_id),
                    )
                    .await;
            }
            AgentEvent::InvocationStart {
                timestamp,
                function,
                invocation_id,
            } => {
                output
                    .emit_invocation_start(timestamp, function, display_id(invocation_id))
                    .await;
            }
            AgentEvent::InvocationFinished {
                timestamp,
                function,
                invocation_id,
            } => {
                output
                    .emit_invocation_finished(timestamp, function, display_id(invocation_id))
                    .await;
            }
            AgentEvent::PluginError {
                timestamp,
                plugin_name,
                message,
            } => {
                output
                    .emit_log(
                        timestamp,
                        LogLevel::Error,
                        format!("plugin:{plugin_name}"),
                        message,
                        None,
                    )
                    .await;
            }
            AgentEvent::SnapshotRecoverySucceeded {
                timestamp,
                snapshot_index,
            } => {
                output
                    .emit_log(
                        timestamp,
                        LogLevel::Info,
                        "snapshot-recovery".to_string(),
                        format!("loaded snapshot from oplog index {snapshot_index}"),
                        None,
                    )
                    .await;
            }
            AgentEvent::SnapshotRecoveryFailed {
                timestamp,
                snapshot_index,
                error,
            } => {
                output
                    .emit_log(
                        timestamp,
                        LogLevel::Error,
                        "snapshot-recovery".to_string(),
                        format!(
                            "failed to load snapshot from oplog index {snapshot_index}: {error}"
                        ),
                        None,
                    )
                    .await;
            }
            AgentEvent::ClientLagged {
                number_of_missed_messages,
            } => {
                output
                    .emit_missed_messages(Timestamp::now_utc(), number_of_missed_messages)
                    .await;
            }
        }
    }

    fn create_request(
        worker_service_url: Url,
        auth_token: String,
        component_id: Uuid,
        agent_id: String,
        allow_insecure: bool,
    ) -> anyhow::Result<(Request, Option<Connector>)> {
        AgentId::validate_length(&agent_id).map_err(|err| anyhow!(err))?;

        let mut url = worker_service_url;

        let ws_schema = if url.scheme() == "http" { "ws" } else { "wss" };

        url.set_scheme(ws_schema)
            .map_err(|()| anyhow!("Failed to set ws url schema".to_string()))?;
        url.path_segments_mut()
            .map_err(|()| anyhow!("Failed to get url path for ws url".to_string()))?
            .push("v1")
            .push("components")
            .push(&component_id.to_string())
            .push("workers")
            .push(&agent_id)
            .push("connect");

        debug!(url = url.as_str(), "Worker stream connect");

        let mut request = url
            .to_string()
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = request.headers_mut();
            headers.insert("Authorization", format!("Bearer {auth_token}").parse()?);
        }

        let connector = if allow_insecure {
            Some(insecure_connector()?)
        } else {
            None
        };
        let connector = golem_client::tls::connector_for(request.uri(), connector)?;

        Ok((request, connector))
    }

    async fn ping_loop(
        mut write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        ping_interval: Duration,
    ) -> anyhow::Error {
        let mut interval = time::interval(ping_interval);
        let mut cnt: i64 = 1;

        loop {
            interval.tick().await;

            let ping_result = write
                .send(Message::Ping(Bytes::from(cnt.to_ne_bytes().to_vec())))
                .await
                .context("Failed to send ping");

            if let Err(err) = ping_result {
                break err;
            }

            cnt += 1;
        }
    }

    async fn read_loop(
        mut read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        output: AgentStreamOutput,
        agent_name: String,
        goal: Option<InvocationGoal>,
    ) {
        let mut filter = TargetFilter::default();
        let mut target = goal.as_ref().map(|goal| goal.target.subscribe());
        let result = loop {
            let events = if let Some(target_rx) = target.as_mut()
                && filter.target.is_none()
                && let Some(invocation_id) = *target_rx.borrow_and_update()
            {
                filter.set_target(invocation_id)
            } else {
                let message = tokio::select! {
                    message = read.next() => message,
                    _ = async {
                        match target.as_mut() {
                            Some(target_rx) if filter.target.is_none() => {
                                let _ = target_rx.changed().await;
                            }
                            _ => future::pending::<()>().await,
                        }
                    } => continue,
                };
                let message = match message {
                    None => break Ok(()),
                    Some(Err(error)) => break Err(error),
                    Some(Ok(message)) => message,
                };
                match Self::parse_websocket_message(message) {
                    None => continue,
                    Some(event) if goal.is_some() => filter.accept(event),
                    Some(event) => vec![event],
                }
            };
            let mut target_finished = false;
            for event in events {
                let is_target_finish = matches!(
                    &event,
                    AgentEvent::InvocationFinished { invocation_id, .. }
                        if filter.target == Some(*invocation_id)
                );
                if is_target_finish && let Some(goal) = &goal {
                    let _ = goal
                        .output_written
                        .subscribe()
                        .wait_for(|written| *written)
                        .await;
                }
                target_finished |= is_target_finish;
                Self::emit(&output, &agent_name, event).await;
            }
            if target_finished && let Some(goal) = &goal {
                goal.reached.store(true, Ordering::Release);
                break Ok(());
            }
        };

        match result {
            Ok(()) | Err(tungstenite::error::Error::ConnectionClosed) => {
                // successful exit
                output.emit_stream_closed(Timestamp::now_utc()).await;
            }
            Err(error) => {
                output.emit_stream_error(Timestamp::now_utc(), error).await;
            }
        }
    }

    fn parse_websocket_message(message: Message) -> Option<AgentEvent> {
        match message {
            Message::Text(str) => {
                let parsed: serde_json::Result<AgentEvent> = serde_json::from_str(str.as_str());

                match parsed {
                    Ok(parsed) => Some(parsed),
                    Err(err) => {
                        error!("Failed to parse worker connect message: {err}");
                        None
                    }
                }
            }
            Message::Binary(data) => {
                let parsed: serde_json::Result<AgentEvent> = serde_json::from_slice(data.as_ref());
                match parsed {
                    Ok(parsed) => Some(parsed),
                    Err(err) => {
                        error!("Failed to parse worker connect message: {err}");
                        None
                    }
                }
            }
            Message::Ping(_) => {
                trace!("Ignore ping");
                None
            }
            Message::Pong(_) => {
                trace!("Ignore pong");
                None
            }
            Message::Close(details) => {
                match details {
                    Some(closed_frame) => {
                        info!("Connection Closed: {}", closed_frame);
                    }
                    None => {
                        info!("Connection Closed");
                    }
                }
                None
            }
            Message::Frame(f) => {
                debug!("Ignored unexpected frame {f:?}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    async fn connection(url: Url) -> anyhow::Result<AgentConnection> {
        AgentConnection::new(
            url,
            TokenSecret::new(),
            &ComponentId(Uuid::new_v4()),
            "test-agent".into(),
            AgentLogStreamOptions {
                colors: false,
                show_timestamp: false,
                show_level: false,
                logs_only: false,
            },
            false,
            Format::Json,
            Duration::from_secs(60),
            true,
        )
        .await
    }

    /// An unrelated invocation finishes before the target, and nested calls of the target
    /// never finish.
    fn events(target: Uuid) -> Vec<AgentEvent> {
        let child = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        let timestamp = Timestamp::now_utc();
        vec![
            AgentEvent::InvocationStart {
                timestamp,
                function: "unrelated".into(),
                invocation_id: unrelated,
            },
            AgentEvent::StdOut {
                timestamp,
                bytes: b"unrelated output\n".to_vec(),
                invocation_id: Some(unrelated),
            },
            AgentEvent::InvocationFinished {
                timestamp,
                function: "unrelated".into(),
                invocation_id: unrelated,
            },
            AgentEvent::InvocationStart {
                timestamp,
                function: "hello".into(),
                invocation_id: target,
            },
            AgentEvent::InvocationStart {
                timestamp,
                function: "child".into(),
                invocation_id: child,
            },
            AgentEvent::StdOut {
                timestamp,
                bytes: b"nested stdout\n".to_vec(),
                invocation_id: Some(child),
            },
            AgentEvent::Log {
                timestamp,
                level: LogLevel::Info,
                context: "hello".into(),
                message: "summary after nested calls".into(),
                invocation_id: Some(target),
            },
            // Terminal events can use a different function name from the start.
            AgentEvent::InvocationFinished {
                timestamp,
                function: "interrupted during retry".into(),
                invocation_id: target,
            },
        ]
    }

    async fn send_events(listener: TcpListener, events: Vec<AgentEvent>, close: bool) {
        let (socket, _) = listener.accept().await.unwrap();
        let mut websocket = accept_async(socket).await.unwrap();
        for event in events {
            websocket
                .send(Message::Text(serde_json::to_string(&event).unwrap().into()))
                .await
                .unwrap();
        }
        if close {
            websocket.close(None).await.unwrap();
        } else {
            future::pending::<()>().await;
        }
    }

    #[test]
    fn target_filter_shows_only_the_target_after_replayed_history() {
        let target = Uuid::new_v4();
        let earlier = Uuid::new_v4();
        let timestamp = Timestamp::now_utc();
        let stdout = |text: &str, invocation_id| AgentEvent::StdOut {
            timestamp,
            bytes: text.as_bytes().to_vec(),
            invocation_id,
        };
        let mut filter = TargetFilter::default();

        let lagged = AgentEvent::ClientLagged {
            number_of_missed_messages: 3,
        };
        assert_eq!(filter.accept(lagged.clone()), vec![lagged]);

        // Replayed history and early target output are held until the target is known.
        for event in [
            stdout("initialization", None),
            stdout("earlier", Some(earlier)),
            AgentEvent::InvocationFinished {
                timestamp,
                function: "run".into(),
                invocation_id: earlier,
            },
            stdout("target 1", Some(target)),
        ] {
            assert_eq!(filter.accept(event), vec![]);
        }
        assert_eq!(
            filter.set_target(target),
            vec![stdout("target 1", Some(target))]
        );

        assert_eq!(filter.accept(stdout("later", Some(earlier))), vec![]);
        assert_eq!(filter.accept(stdout("no invocation", None)), vec![]);
        assert_eq!(
            filter.accept(stdout("target 2", Some(target))),
            vec![stdout("target 2", Some(target))]
        );
    }

    #[test]
    async fn invocation_stream_waits_for_target_finish_after_nested_calls() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let target = Uuid::new_v4();
        let connection = connection(url).await.unwrap();
        let goal = connection.invocation_goal().unwrap();
        goal.set_target(target);
        goal.set_output_written();

        time::timeout(Duration::from_secs(5), async {
            let (result, ()) = tokio::join!(
                connection.run_forever(),
                send_events(listener, events(target), true)
            );
            result.unwrap();
        })
        .await
        .expect("the stream must exit after the target finish, even with unfinished nested calls");
    }

    #[test]
    async fn invocation_stream_ends_after_the_target_finish_and_the_callers_output() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let target = Uuid::new_v4();
        let connection = connection(url).await.unwrap();
        let goal = connection.invocation_goal().unwrap();
        // The socket stays open, so only the goal can end the connection.
        let server = tokio::spawn(send_events(listener, events(target), false));
        let run = tokio::spawn(connection.run_forever());

        time::sleep(Duration::from_millis(200)).await;
        assert!(
            !run.is_finished(),
            "an unrelated finish must not end the stream"
        );
        goal.set_target(target);
        time::sleep(Duration::from_millis(200)).await;
        assert!(
            !run.is_finished(),
            "the target finish must wait for the caller's own output"
        );
        goal.set_output_written();

        time::timeout(Duration::from_secs(5), run)
            .await
            .expect("setting an already finished target must end the stream")
            .unwrap()
            .unwrap();
        server.abort();
    }
}
