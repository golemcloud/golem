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

//! The decisions of the session layer, as functions over plain values.

use super::store::{McpSessionId, McpSessionRecord, McpSessionStoreError};
use crate::mcp::serving::{ServeKind, ServeRefusal};
use golem_common::model::deployment::DeploymentRevision;
use golem_service_base::mcp::CompiledMcp;
use poem::http::{HeaderMap, Method, StatusCode};
use rmcp::model::{ClientJsonRpcMessage, ClientNotification, ClientRequest, RequestId};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_MCP_PROTOCOL_VERSION, HEADER_SESSION_ID,
    JSON_MIME_TYPE,
};
use std::borrow::Cow;
use std::time::Duration;

/// What the layer does with a request whose method and headers passed [`request_checks`].
#[derive(Debug, PartialEq)]
pub(super) enum RequestClass {
    /// An `initialize` request: rmcp answers it, and a new session is created. Any
    /// `Mcp-Session-Id` it carries is ignored.
    Initialize,
    /// A request without a session that rmcp answers as it is: `server/discover`, or a body
    /// that does not parse (rmcp answers its parse error).
    PassThrough,
    /// A request that needs a session but carries no `Mcp-Session-Id`.
    MissingSession,
    /// An `Mcp-Session-Id` that is not a session ID.
    MalformedSession,
    /// A request of the session with this ID.
    Session(McpSessionId, SessionRequest),
}

/// A request of a session, by what the layer does with it after the session is admitted.
#[derive(Debug, PartialEq)]
pub(super) enum SessionRequest {
    /// A JSON-RPC request: it runs as a call of the session.
    Call(RequestId),
    /// `notifications/cancelled`: cancels the session's calls with this request ID.
    Cancel(Option<RequestId>),
    /// Any other notification, or a body that does not parse: rmcp answers it.
    Forward,
    /// A JSON-RPC response or error: accepted and dropped, as a stateless server has no request
    /// of its own to answer.
    Reply,
    /// A GET without `Last-Event-ID`: a stream of the session's notifications.
    Stream,
    /// A GET with `Last-Event-ID`: the rest of the response stream that event belongs to.
    Resume(Box<str>),
    /// DELETE: ends the session.
    Close,
}

/// The answer to a request that the layer refuses before it reads the session: its status, the
/// headers it needs (`Allow` for 405), and its body text, with rmcp's texts where rmcp makes the
/// same check.
#[derive(Debug, PartialEq)]
pub(super) struct Refusal {
    pub(super) status: StatusCode,
    pub(super) headers: &'static [(&'static str, &'static str)],
    pub(super) text: Cow<'static, str>,
}

impl Refusal {
    fn new(status: StatusCode, text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            headers: &[],
            text: text.into(),
        }
    }
}

/// The checks that come before any session or store step: the HTTP method, and rmcp's own
/// `Accept`, `Content-Type` and `MCP-Protocol-Version` checks for the method. A refused request
/// makes no store call, so it does not move the session's expiry.
pub(super) fn request_checks(method: &Method, headers: &HeaderMap) -> Option<Refusal> {
    let header = |name: &str| headers.get(name).map(|value| value.to_str().unwrap_or(""));
    let accepts = |mime: &str| {
        header(poem::http::header::ACCEPT.as_str()).is_some_and(|accept| accept.contains(mime))
    };
    match *method {
        Method::POST => {
            if !(accepts(JSON_MIME_TYPE) && accepts(EVENT_STREAM_MIME_TYPE)) {
                Some(Refusal::new(
                    StatusCode::NOT_ACCEPTABLE,
                    "Not Acceptable: Client must accept both application/json and text/event-stream",
                ))
            } else if !header(poem::http::header::CONTENT_TYPE.as_str())
                .is_some_and(|content_type| content_type.starts_with(JSON_MIME_TYPE))
            {
                Some(Refusal::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    "Unsupported Media Type: Content-Type must be application/json",
                ))
            } else {
                None
            }
        }
        Method::GET if !accepts(EVENT_STREAM_MIME_TYPE) => Some(Refusal::new(
            StatusCode::NOT_ACCEPTABLE,
            "Not Acceptable: Client must accept text/event-stream",
        )),
        Method::GET | Method::DELETE => protocol_version_check(headers),
        _ => Some(Refusal {
            headers: &[("allow", "GET, POST, DELETE")],
            ..Refusal::new(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed")
        }),
    }
}

fn protocol_version_check(headers: &HeaderMap) -> Option<Refusal> {
    let value = headers.get(HEADER_MCP_PROTOCOL_VERSION)?;
    let Ok(version) = value.to_str() else {
        return Some(Refusal::new(
            StatusCode::BAD_REQUEST,
            "Bad Request: Invalid MCP-Protocol-Version header encoding",
        ));
    };
    (!rmcp::model::ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .any(|known| known.as_str() == version))
    .then(|| {
        Refusal::new(
            StatusCode::BAD_REQUEST,
            format!("Bad Request: Unsupported MCP-Protocol-Version: {version}"),
        )
    })
}

/// Classifies a GET, POST or DELETE. `parsed` is the POST body as rmcp parses it, and `None`
/// for GET and DELETE. An `initialize` request is recognised before its session header is read.
pub(super) fn classify(
    method: &Method,
    headers: &HeaderMap,
    parsed: Option<&Result<ClientJsonRpcMessage, serde_json::Error>>,
) -> RequestClass {
    let message = parsed.and_then(|parsed| parsed.as_ref().ok());
    if let Some(ClientJsonRpcMessage::Request(request)) = message
        && matches!(request.request, ClientRequest::InitializeRequest(_))
    {
        return RequestClass::Initialize;
    }
    let Some(header) = headers.get(HEADER_SESSION_ID) else {
        return match (message, parsed) {
            (Some(ClientJsonRpcMessage::Request(request)), _)
                if matches!(request.request, ClientRequest::DiscoverRequest(_)) =>
            {
                RequestClass::PassThrough
            }
            (None, Some(Err(_))) => RequestClass::PassThrough,
            _ => RequestClass::MissingSession,
        };
    };
    let Some(id) = header
        .to_str()
        .ok()
        .and_then(|header| header.parse::<McpSessionId>().ok())
    else {
        return RequestClass::MalformedSession;
    };
    let request = match (method, message) {
        (&Method::GET, _) => headers
            .get(HEADER_LAST_EVENT_ID)
            .map(|last| SessionRequest::Resume(last.to_str().unwrap_or("").into()))
            .unwrap_or(SessionRequest::Stream),
        (&Method::DELETE, _) => SessionRequest::Close,
        (_, Some(ClientJsonRpcMessage::Request(request))) => {
            SessionRequest::Call(request.id.clone())
        }
        (_, Some(ClientJsonRpcMessage::Notification(notification))) => {
            match &notification.notification {
                ClientNotification::CancelledNotification(cancelled) => {
                    SessionRequest::Cancel(cancelled.params.request_id.clone())
                }
                _ => SessionRequest::Forward,
            }
        }
        (_, Some(ClientJsonRpcMessage::Response(_) | ClientJsonRpcMessage::Error(_))) => {
            SessionRequest::Reply
        }
        (_, None) => SessionRequest::Forward,
    };
    RequestClass::Session(id, request)
}

/// The outcome of reading a session's record for a request served with `served`.
#[derive(Debug, PartialEq)]
pub(super) enum Admission {
    /// The session is live for the request's domain and environment. Its client last listed
    /// the tools of this deployment revision (the pin).
    Admitted(DeploymentRevision),
    /// The record is missing or expired, or belongs to another domain or environment.
    Gone,
    /// The store could not be read.
    Unavailable,
}

pub(super) fn admit(
    read: &Result<Option<McpSessionRecord>, McpSessionStoreError>,
    served: &CompiledMcp,
) -> Admission {
    match read {
        Ok(Some(record))
            if record.domain == served.domain && record.environment_id == served.environment_id =>
        {
            Admission::Admitted(record.deployment_revision)
        }
        Ok(_) => Admission::Gone,
        Err(_) => Admission::Unavailable,
    }
}

/// What serving a request of `kind` with the `served` revision does with the session's pin
/// (`stored`).
#[derive(Debug, PartialEq)]
pub(super) enum PinStep {
    Serve,
    /// Serve, after the pin is moved to this revision.
    Write(DeploymentRevision),
    /// The client listed another revision's tools: it must list again.
    Changed,
}

/// `initialize` and the lists serve any revision and move the pin to it; a call, resource read
/// or prompt get is served only with the revision its client listed. Only equality counts, so
/// a rollback to an older revision is a change too.
pub(super) fn pin_step(
    kind: ServeKind,
    stored: DeploymentRevision,
    served: DeploymentRevision,
) -> PinStep {
    match (stored == served, kind) {
        (true, _) => PinStep::Serve,
        (false, ServeKind::Initialize | ServeKind::List) => PinStep::Write(served),
        (false, ServeKind::Invoke) => PinStep::Changed,
    }
}

/// The outcome of a pin write: `replace` finds no live record when the session has ended.
pub(super) fn pin_written(
    replaced: &Result<bool, McpSessionStoreError>,
) -> Result<(), ServeRefusal> {
    match replaced {
        Ok(true) => Ok(()),
        Ok(false) => Err(ServeRefusal::Gone),
        Err(_) => Err(ServeRefusal::Unavailable),
    }
}

/// The longest time between two expiry updates of a session with a running call.
const MAX_HEARTBEAT_PERIOD: Duration = Duration::from_secs(60);

/// How often a running call moves its session's expiry forward: three times per idle timeout,
/// at most once a minute, and never more often than once a millisecond.
pub(super) fn heartbeat_period(idle: Duration) -> Duration {
    (idle / 3)
        .min(MAX_HEARTBEAT_PERIOD)
        .max(Duration::from_millis(1))
}

/// The event ID of the first SSE event in `frame`, the start of a response body. rmcp starts
/// every streamed POST response with a priming event that carries the ID.
pub(super) fn priming_event_id(frame: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(frame).ok()?;
    let event = text.split("\n\n").next()?;
    event
        .lines()
        .find_map(|line| line.strip_prefix("id:"))
        .map(|id| id.strip_prefix(' ').unwrap_or(id))
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::application::ApplicationName;
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::domain_registration::Domain;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use poem::http::HeaderValue;
    use serde_json::json;
    use test_r::test;

    use crate::mcp::session::test_ids::{SESSION, session};

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| {
                (
                    poem::http::HeaderName::from_static(name),
                    HeaderValue::from_str(value).unwrap(),
                )
            })
            .collect()
    }

    fn parse(body: serde_json::Value) -> Result<ClientJsonRpcMessage, serde_json::Error> {
        serde_json::from_value(body)
    }

    fn initialize() -> serde_json::Value {
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "test", "version": "1"}}})
    }

    fn discover() -> serde_json::Value {
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {}})
    }

    fn tools_list() -> serde_json::Value {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"})
    }

    fn post(
        session: Option<&str>,
        body: Result<ClientJsonRpcMessage, serde_json::Error>,
    ) -> RequestClass {
        let headers = session
            .map(|id| headers(&[("mcp-session-id", id)]))
            .unwrap_or_default();
        classify(&Method::POST, &headers, Some(&body))
    }

    fn unparsable() -> Result<ClientJsonRpcMessage, serde_json::Error> {
        serde_json::from_slice(b"{not json")
    }

    #[test]
    fn classify_table() {
        let call =
            |id: i64| RequestClass::Session(session(), SessionRequest::Call(RequestId::Number(id)));
        let full_meta_list = json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list", "params": {
            "_meta": {"io.modelcontextprotocol/protocolVersion": "2025-03-26",
                      "io.modelcontextprotocol/clientInfo": {"name": "t", "version": "1"},
                      "io.modelcontextprotocol/clientCapabilities": {}}}});
        [
            ("initialize, no session", post(None, parse(initialize())), RequestClass::Initialize),
            ("initialize, malformed session", post(Some("nope"), parse(initialize())), RequestClass::Initialize),
            ("initialize, other session", post(Some(SESSION), parse(initialize())), RequestClass::Initialize),
            ("discover, no session", post(None, parse(discover())), RequestClass::PassThrough),
            (
                "initialize-shaped custom request, no session",
                post(None, parse(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}))),
                RequestClass::MissingSession,
            ),
            (
                "malformed discover, no session",
                post(None, parse(json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"}))),
                RequestClass::MissingSession,
            ),
            ("tools/list with full _meta, no session", post(None, parse(full_meta_list)), RequestClass::MissingSession),
            ("tools/list, no session", post(None, parse(tools_list())), RequestClass::MissingSession),
            (
                "escaped tools/call, no session",
                post(None, serde_json::from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"tools\/call","params":{"name":"x"}}"#)),
                RequestClass::MissingSession,
            ),
            (
                "notification, no session",
                post(None, parse(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))),
                RequestClass::MissingSession,
            ),
            (
                "response, no session",
                post(None, parse(json!({"jsonrpc": "2.0", "id": 3, "result": {}}))),
                RequestClass::MissingSession,
            ),
            ("unparsable, no session", post(None, unparsable()), RequestClass::PassThrough),
            ("malformed session", post(Some("NOPE"), parse(tools_list())), RequestClass::MalformedSession),
            ("malformed session, unparsable", post(Some("nope"), unparsable()), RequestClass::MalformedSession),
            ("request with session", post(Some(SESSION), parse(tools_list())), call(7)),
            (
                "escaped tools/call with session",
                post(Some(SESSION), serde_json::from_slice(br#"{"jsonrpc":"2.0","id":1,"method":"tools\/call","params":{"name":"x"}}"#)),
                call(1),
            ),
            (
                "discover with session",
                post(Some(SESSION), parse(discover())),
                call(1),
            ),
            (
                "cancel with session",
                post(Some(SESSION), parse(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}))),
                RequestClass::Session(session(), SessionRequest::Cancel(Some(RequestId::Number(7)))),
            ),
            (
                "other notification with session",
                post(Some(SESSION), parse(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))),
                RequestClass::Session(session(), SessionRequest::Forward),
            ),
            (
                "response with session",
                post(Some(SESSION), parse(json!({"jsonrpc": "2.0", "id": 3, "result": {}}))),
                RequestClass::Session(session(), SessionRequest::Reply),
            ),
            (
                "error with session",
                post(Some(SESSION), parse(json!({"jsonrpc": "2.0", "id": 3, "error": {"code": 1, "message": "x"}}))),
                RequestClass::Session(session(), SessionRequest::Reply),
            ),
            ("unparsable with session", post(Some(SESSION), unparsable()), RequestClass::Session(session(), SessionRequest::Forward)),
            (
                "GET, no session",
                classify(&Method::GET, &headers(&[("mcp-protocol-version", "2026-07-28")]), None),
                RequestClass::MissingSession,
            ),
            ("GET with session", classify(&Method::GET, &headers(&[("mcp-session-id", SESSION)]), None), RequestClass::Session(session(), SessionRequest::Stream)),
            (
                "GET with session and Last-Event-ID",
                classify(&Method::GET, &headers(&[("mcp-session-id", SESSION), ("last-event-id", "s/1")]), None),
                RequestClass::Session(session(), SessionRequest::Resume("s/1".into())),
            ),
            ("DELETE, no session", classify(&Method::DELETE, &HeaderMap::new(), None), RequestClass::MissingSession),
            ("DELETE with session", classify(&Method::DELETE, &headers(&[("mcp-session-id", SESSION)]), None), RequestClass::Session(session(), SessionRequest::Close)),
        ]
        .into_iter()
        .for_each(|(row, actual, expected)| assert_eq!(actual, expected, "{row}"));
    }

    #[test]
    fn request_checks_table() {
        let both = "application/json, text/event-stream";
        let refused = |method: Method, pairs: &[(&'static str, &str)]| {
            request_checks(&method, &headers(pairs)).map(|refusal| refusal.status)
        };
        [
            (
                "POST ok",
                refused(
                    Method::POST,
                    &[("accept", both), ("content-type", "application/json")],
                ),
                None,
            ),
            (
                "POST json only",
                refused(
                    Method::POST,
                    &[
                        ("accept", "application/json"),
                        ("content-type", "application/json"),
                    ],
                ),
                Some(StatusCode::NOT_ACCEPTABLE),
            ),
            (
                "POST no accept",
                refused(Method::POST, &[("content-type", "application/json")]),
                Some(StatusCode::NOT_ACCEPTABLE),
            ),
            (
                "POST text body",
                refused(
                    Method::POST,
                    &[("accept", both), ("content-type", "text/plain")],
                ),
                Some(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ),
            (
                "POST no content type",
                refused(Method::POST, &[("accept", both)]),
                Some(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            ),
            (
                "POST unknown version is rmcp's",
                refused(
                    Method::POST,
                    &[
                        ("accept", both),
                        ("content-type", "application/json"),
                        ("mcp-protocol-version", "1999-01-01"),
                    ],
                ),
                None,
            ),
            (
                "GET ok",
                refused(Method::GET, &[("accept", "text/event-stream")]),
                None,
            ),
            (
                "GET known version",
                refused(
                    Method::GET,
                    &[
                        ("accept", "text/event-stream"),
                        ("mcp-protocol-version", "2025-03-26"),
                    ],
                ),
                None,
            ),
            (
                "GET json",
                refused(Method::GET, &[("accept", "application/json")]),
                Some(StatusCode::NOT_ACCEPTABLE),
            ),
            (
                "GET unknown version",
                refused(
                    Method::GET,
                    &[
                        ("accept", "text/event-stream"),
                        ("mcp-protocol-version", "1999-01-01"),
                    ],
                ),
                Some(StatusCode::BAD_REQUEST),
            ),
            (
                "GET wrong accept before version",
                refused(Method::GET, &[("mcp-protocol-version", "1999-01-01")]),
                Some(StatusCode::NOT_ACCEPTABLE),
            ),
            ("DELETE ok", refused(Method::DELETE, &[]), None),
            (
                "DELETE unknown version",
                refused(Method::DELETE, &[("mcp-protocol-version", "1999-01-01")]),
                Some(StatusCode::BAD_REQUEST),
            ),
            (
                "PUT",
                refused(Method::PUT, &[("accept", both)]),
                Some(StatusCode::METHOD_NOT_ALLOWED),
            ),
            (
                "OPTIONS",
                refused(Method::OPTIONS, &[]),
                Some(StatusCode::METHOD_NOT_ALLOWED),
            ),
        ]
        .into_iter()
        .for_each(|(row, actual, expected)| assert_eq!(actual, expected, "{row}"));
    }

    fn deployment(domain: &str, environment_id: EnvironmentId) -> CompiledMcp {
        CompiledMcp {
            account_id: AccountId::new(),
            account_email: AccountEmail::new("owner@example.com"),
            application_name: ApplicationName("app".into()),
            environment_id,
            environment_name: EnvironmentName("env".into()),
            deployment_revision: DeploymentRevision::INITIAL,
            domain: Domain(domain.into()),
            security_scheme_name: None,
            security_scheme: None,
            registered_agent_types: vec![],
            tools: vec![],
        }
    }

    #[test]
    fn admit_table() {
        let environment = EnvironmentId::new();
        let served = deployment("a.example.com", environment);
        let record = |domain: &str, environment_id| McpSessionRecord {
            domain: Domain(domain.into()),
            environment_id,
            deployment_revision: DeploymentRevision::INITIAL,
        };
        [
            (
                "same domain and environment",
                Ok(Some(record("a.example.com", environment))),
                Admission::Admitted(DeploymentRevision::INITIAL),
            ),
            (
                "other revision is admitted with its pin",
                Ok(Some(McpSessionRecord {
                    deployment_revision: DeploymentRevision::INITIAL.next().unwrap(),
                    ..record("a.example.com", environment)
                })),
                Admission::Admitted(DeploymentRevision::INITIAL.next().unwrap()),
            ),
            (
                "other domain",
                Ok(Some(record("b.example.com", environment))),
                Admission::Gone,
            ),
            (
                "other environment",
                Ok(Some(record("a.example.com", EnvironmentId::new()))),
                Admission::Gone,
            ),
            ("missing", Ok(None), Admission::Gone),
            (
                "store error",
                Err(McpSessionStoreError::InternalError(anyhow::anyhow!("down"))),
                Admission::Unavailable,
            ),
        ]
        .into_iter()
        .for_each(|(row, read, expected)| assert_eq!(admit(&read, &served), expected, "{row}"));
    }

    #[test]
    fn pin_step_table() {
        let n = DeploymentRevision::INITIAL.next().unwrap();
        let older = DeploymentRevision::INITIAL;
        let newer = n.next().unwrap();
        [
            (
                "initialize, same",
                ServeKind::Initialize,
                n,
                n,
                PinStep::Serve,
            ),
            (
                "initialize, newer",
                ServeKind::Initialize,
                n,
                newer,
                PinStep::Write(newer),
            ),
            ("list, same", ServeKind::List, n, n, PinStep::Serve),
            (
                "list, newer",
                ServeKind::List,
                n,
                newer,
                PinStep::Write(newer),
            ),
            (
                "list, older (stale replica)",
                ServeKind::List,
                n,
                older,
                PinStep::Write(older),
            ),
            ("invoke, same", ServeKind::Invoke, n, n, PinStep::Serve),
            (
                "invoke, newer (deploy)",
                ServeKind::Invoke,
                n,
                newer,
                PinStep::Changed,
            ),
            (
                "invoke, older (rollback)",
                ServeKind::Invoke,
                n,
                older,
                PinStep::Changed,
            ),
        ]
        .into_iter()
        .for_each(|(row, kind, stored, served, expected)| {
            assert_eq!(pin_step(kind, stored, served), expected, "{row}")
        });
    }

    #[test]
    fn pin_written_table() {
        [
            ("replaced", Ok(true), Ok(())),
            ("no live record", Ok(false), Err(ServeRefusal::Gone)),
            (
                "store error",
                Err(McpSessionStoreError::InternalError(anyhow::anyhow!("down"))),
                Err(ServeRefusal::Unavailable),
            ),
        ]
        .into_iter()
        .for_each(|(row, replaced, expected)| {
            assert_eq!(pin_written(&replaced), expected, "{row}")
        });
    }

    #[test]
    fn heartbeat_period_table() {
        let ms = Duration::from_millis;
        [
            (
                "default idle timeout",
                Duration::from_secs(300),
                Duration::from_secs(60),
            ),
            (
                "at the cap",
                Duration::from_secs(180),
                Duration::from_secs(60),
            ),
            (
                "below the cap",
                Duration::from_secs(90),
                Duration::from_secs(30),
            ),
            ("one second", ms(1000), Duration::from_nanos(333_333_333)),
            ("shortest idle timeout", ms(1), ms(1)),
        ]
        .into_iter()
        .for_each(|(row, idle, expected)| assert_eq!(heartbeat_period(idle), expected, "{row}"));
    }

    #[test]
    fn priming_event_id_table() {
        [
            (
                "rmcp priming",
                &b"data:\nid: s1/0\nretry: 3000\n\n"[..],
                Some("s1/0"),
            ),
            ("no space", b"id:s1/0\n\n", Some("s1/0")),
            ("id of the first event only", b"data:\n\nid: s1/1\n\n", None),
            (
                "partial first event",
                b"retry: 3000\nid: s1/0",
                Some("s1/0"),
            ),
            ("no id", b"retry: 3000\n\n", None),
            ("empty id", b"id:\n\n", None),
            ("not utf-8", b"id: \xff\n\n", None),
        ]
        .into_iter()
        .for_each(|(row, frame, expected)| assert_eq!(priming_event_id(frame), expected, "{row}"));
    }
}
