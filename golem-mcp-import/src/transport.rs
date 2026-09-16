//! Bounded, single-attempt Streamable HTTP. The caller owns authorization,
//! accounting, credential acquisition, and durable retry decisions.

use crate::tool::{ProjectedTool, encode_header_value};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures::{StreamExt, TryStreamExt};
use headers::HeaderMapExt;
use http::{HeaderValue, Request, Response, StatusCode, header};
use http_body::Body;
use http_body_util::BodyExt;
use rmcp::model::{
    ClientCapabilities, Implementation, JsonRpcError, JsonRpcRequest, ProtocolVersion,
    Request as RpcRequest, RequestId, RequestMetaObject,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

pub mod sender;

pub const PROTOCOL_VERSION: &str = "2026-07-28";
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[PROTOCOL_VERSION];

/// Every invocation sends exactly one request, without redirects, authentication
/// retries, reconnects, proxies with independent credentials, or request replay.
/// Implementations must authorize the target before charging network quotas and
/// dispatching. Dropping the future/body must cancel the HTTP operation.
/// Preserve response encoding and bytes: the transport requests identity encoding
/// and rejects compressed success bodies rather than accepting unbounded decoding.
pub trait HttpSend {
    type Body: Body<Data = Bytes, Error: std::error::Error + Send + Sync + 'static> + Send;
    /// Host failures (including quota traps) must reach the durable caller intact.
    type Error: From<TransportError> + Send;

    fn send(
        &mut self,
        request: Request<Bytes>,
    ) -> impl Future<Output = Result<Response<Self::Body>, Self::Error>> + Send;
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub response_bytes: usize,
    pub request_bytes: usize,
    pub listing_bytes: usize,
    pub pages: usize,
    pub tools: usize,
    pub notifications: usize,
    pub json_depth: usize,
    pub concurrency: usize,
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            response_bytes: 16 << 20,
            request_bytes: 8 << 20,
            listing_bytes: 16 << 20,
            pages: 64,
            tools: 1024,
            notifications: 1024,
            json_depth: 512,
            concurrency: 16,
            timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, thiserror::Error, Serialize, Deserialize, PartialEq)]
pub enum TransportError {
    #[error("invalid MCP configuration: {0}")]
    Configuration(String),
    #[error("invalid MCP input: {0}")]
    InvalidInput(String),
    #[error("MCP network access denied")]
    Denied,
    #[error("MCP HTTP quota exhausted")]
    QuotaExhausted,
    #[error("MCP network request failed")]
    Network,
    #[error("MCP operation timed out")]
    Timeout,
    #[error("MCP {0} limit exceeded")]
    Limit(String),
    #[error("MCP upstream authorization required (HTTP {0})")]
    AuthorizationRequired(u16),
    #[error("MCP upstream HTTP status {0}")]
    HttpStatus(u16),
    #[error("invalid MCP response: {0}")]
    Protocol(String),
    #[error("MCP requires an unsupported client capability or continuation")]
    UnsupportedCapability,
    #[error("MCP error {code}: {message}")]
    Remote {
        code: i32,
        message: String,
        data: Option<Value>,
    },
}

fn protocol(message: &str) -> TransportError {
    TransportError::Protocol(message.into())
}

fn limit(name: &str) -> TransportError {
    TransportError::Limit(name.into())
}

/// A credential-scoped endpoint. Clones share the concurrency bound, never a
/// session or metadata cache. Credentials are neither serializable nor printable.
#[derive(Clone)]
pub struct Client {
    url: url::Url,
    authorization: Option<HeaderValue>,
    limits: Limits,
    permits: Arc<Semaphore>,
    next_request_id: Arc<AtomicI64>,
}

#[derive(Debug)]
pub struct Listing {
    pub tools: Vec<Value>,
    pub protocol_version: String,
}

impl Client {
    pub fn new(url: &str, version: Option<&str>, limits: Limits) -> Result<Self, TransportError> {
        let configuration = |message: &str| TransportError::Configuration(message.into());
        if version.is_some_and(|v| !SUPPORTED_PROTOCOL_VERSIONS.contains(&v)) {
            return Err(configuration("unsupported protocol version"));
        }
        if url.bytes().any(|b| b.is_ascii_control()) {
            return Err(configuration("invalid endpoint"));
        }
        let url = url::Url::parse(url).map_err(|_| configuration("invalid endpoint"))?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(configuration("invalid endpoint"));
        }
        if limits.json_depth == 0
            || limits.json_depth > 512
            || limits.concurrency == 0
            || limits.concurrency > Semaphore::MAX_PERMITS
            || limits.timeout.is_zero()
        {
            return Err(configuration("invalid transport limits"));
        }
        Ok(Self {
            url,
            authorization: None,
            limits,
            permits: Arc::new(Semaphore::new(limits.concurrency)),
            next_request_id: Arc::new(AtomicI64::new(1)),
        })
    }

    pub fn with_bearer(mut self, token: &str) -> Result<Self, TransportError> {
        // RFC 6750 b64token: padding may occur only at the end.
        let unpadded = token.trim_end_matches('=');
        if unpadded.is_empty()
            || !unpadded.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
            })
        {
            return Err(TransportError::Configuration(
                "invalid bearer credential".into(),
            ));
        }
        self.set_authorization(format!("Bearer {token}"))?;
        Ok(self)
    }

    pub fn with_basic(mut self, user: &str, password: &str) -> Result<Self, TransportError> {
        if user.contains(':')
            || user
                .bytes()
                .chain(password.bytes())
                .any(|b| b.is_ascii_control())
        {
            return Err(TransportError::Configuration(
                "invalid basic credential".into(),
            ));
        }
        self.set_authorization(format!(
            "Basic {}",
            STANDARD.encode(format!("{user}:{password}"))
        ))?;
        Ok(self)
    }

    fn set_authorization(&mut self, value: String) -> Result<(), TransportError> {
        let mut value = HeaderValue::from_str(&value)
            .map_err(|_| TransportError::Configuration("invalid credential header".into()))?;
        value.set_sensitive(true);
        self.authorization = Some(value);
        Ok(())
    }

    /// Publishes only complete observations. Unsupported-version errors are
    /// returned: the tested set currently has no second version to negotiate.
    pub async fn list_tools<S: HttpSend>(&self, sender: &mut S) -> Result<Listing, S::Error> {
        tokio::time::timeout(self.limits.timeout, async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| TransportError::Network)?;
            let mut tools = Vec::new();
            let mut cursor = None;
            let mut cursors = BTreeSet::new();
            let mut remaining = self.limits.listing_bytes;
            for _ in 0..self.limits.pages {
                let mut params = json!({});
                if let Some(cursor) = cursor.take() {
                    params["cursor"] = cursor;
                }
                let (mut page, bytes) = self
                    .post(sender, "tools/list", params, &[], remaining)
                    .await?;
                remaining -= bytes;
                let entries = page
                    .get_mut("tools")
                    .and_then(Value::as_array_mut)
                    .ok_or_else(|| protocol("tools/list must contain an array"))?;
                if entries.len() > self.limits.tools.saturating_sub(tools.len()) {
                    return Err(limit("tool count").into());
                }
                tools.append(entries);
                match page.get("nextCursor") {
                    None => {
                        return Ok(Listing {
                            tools,
                            protocol_version: PROTOCOL_VERSION.into(),
                        });
                    }
                    Some(Value::String(next)) => {
                        if !cursors.insert(next.clone()) {
                            return Err(protocol("repeated pagination cursor").into());
                        }
                        cursor = Some(Value::String(next.clone()));
                    }
                    Some(_) => return Err(protocol("invalid pagination cursor").into()),
                }
            }
            Err(limit("pagination").into())
        })
        .await
        .map_err(|_| TransportError::Timeout)?
    }

    /// Arguments and headers come from the admission-time projection. No prior
    /// listing, fresh metadata lookup, or SDK schema cache is consulted.
    pub async fn call_tool<S: HttpSend>(
        &self,
        sender: &mut S,
        tool: &ProjectedTool,
        input: &golem_schema::schema::SchemaValue,
        idempotency_key: Option<&str>,
    ) -> Result<Value, S::Error> {
        let arguments = tool
            .arguments(input)
            .map_err(|e| TransportError::InvalidInput(e.to_string()))?;
        let mut headers = tool
            .parameter_headers(&arguments)
            .map_err(|e| TransportError::InvalidInput(e.to_string()))?;
        headers.push(("Mcp-Name".into(), encode_header_value(&tool.upstream_name)));
        if let Some(key) = idempotency_key {
            headers.push(("Idempotency-Key".into(), key.into()));
        }
        tokio::time::timeout(self.limits.timeout, async {
            let _permit = self
                .permits
                .acquire()
                .await
                .map_err(|_| TransportError::Network)?;
            let (result, _) = self
                .post(
                    sender,
                    "tools/call",
                    json!({"name":tool.upstream_name, "arguments":arguments}),
                    &headers,
                    self.limits.response_bytes,
                )
                .await?;
            match result.get("resultType") {
                None => Ok(result),
                Some(Value::String(kind)) if kind == "complete" => Ok(result),
                Some(Value::String(_)) => Err(TransportError::UnsupportedCapability.into()),
                Some(_) => Err(protocol("invalid resultType").into()),
            }
        })
        .await
        .map_err(|_| TransportError::Timeout)?
    }

    async fn post<S: HttpSend>(
        &self,
        sender: &mut S,
        method: &str,
        mut params: Value,
        headers: &[(String, String)],
        remaining: usize,
    ) -> Result<(Value, usize), S::Error> {
        let request_id = self
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| limit("request IDs"))?;
        let meta = RequestMetaObject::with_client_context(
            ProtocolVersion::V_2026_07_28,
            Implementation::new("golem-mcp-import", env!("CARGO_PKG_VERSION")),
            ClientCapabilities::default(),
        );
        params["_meta"] =
            serde_json::to_value(meta).map_err(|_| protocol("invalid client metadata"))?;
        params["_meta"]["progressToken"] = json!(request_id);
        let mut rpc = RpcRequest::<String, Value>::new(params);
        rpc.method = method.into();
        let rpc = JsonRpcRequest::new(RequestId::Number(request_id), rpc);
        crate::limits::check_bytes(&rpc, self.limits.request_bytes)
            .map_err(|_| limit("request bytes"))?;
        let bytes = serde_json::to_vec(&rpc).map_err(|_| protocol("invalid request"))?;
        let mut request = Request::post(self.url.as_str())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header(header::ACCEPT_ENCODING, "identity")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .header("Mcp-Method", method);
        if let Some(authorization) = &self.authorization {
            request = request.header(header::AUTHORIZATION, authorization.clone());
        }
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let request = request
            .body(Bytes::from(bytes))
            .map_err(|_| TransportError::InvalidInput("invalid request header".into()))?;
        let response = sender.send(request).await?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(TransportError::AuthorizationRequired(status.as_u16()).into());
        }
        if status.is_redirection() {
            return Err(TransportError::HttpStatus(status.as_u16()).into());
        }
        let maximum = remaining.min(self.limits.response_bytes);
        if let Some(length) = response
            .headers()
            .typed_try_get::<headers::ContentLength>()
            .map_err(|_| protocol("invalid Content-Length"))?
            && length.0 > maximum as u64
        {
            return Err(limit("response bytes").into());
        }
        // ContentEncoding's typed parser exposes only case-sensitive membership,
        // not the complete coding list. Inspect every coding so an identity token
        // cannot hide compression. HTTP list recipients ignore empty members.
        if response
            .headers()
            .get_all(header::CONTENT_ENCODING)
            .iter()
            .flat_map(|v| v.as_bytes().split(|b| *b == b','))
            .map(|v| v.trim_ascii())
            .any(|v| !v.is_empty() && !v.eq_ignore_ascii_case(b"identity"))
        {
            return Err((if status.is_success() {
                protocol("unsupported Content-Encoding")
            } else {
                TransportError::HttpStatus(status.as_u16())
            })
            .into());
        }
        // Content-Type is a singleton; the typed parser alone reads only the
        // first field, which would hide conflicting repeated values.
        if response
            .headers()
            .get_all(header::CONTENT_TYPE)
            .iter()
            .count()
            > 1
        {
            return Err(protocol("multiple Content-Type fields").into());
        }
        let content_type = response
            .headers()
            .typed_try_get::<headers::ContentType>()
            .ok()
            .flatten();
        let sse = match content_type.map(mime::Mime::from) {
            Some(mime) if mime.essence_str() == "text/event-stream" && status.is_success() => true,
            Some(mime) if mime.essence_str() == "application/json" => false,
            _ if !status.is_success() => {
                return Err(TransportError::HttpStatus(status.as_u16()).into());
            }
            _ => return Err(protocol("expected JSON or SSE Content-Type").into()),
        };
        let mut used = 0;
        let body = futures::stream::try_unfold(
            (Box::pin(response.into_body()), &mut used),
            |(mut body, used)| async move {
                loop {
                    // Also bounds recursive polling in the SDK SSE parser when
                    // the peer sends a long series of immediately ready comments.
                    tokio::task::yield_now().await;
                    match body.frame().await {
                        None => return Ok(None),
                        Some(Err(_)) => return Err(TransportError::Network),
                        Some(Ok(frame)) => {
                            if let Ok(bytes) = frame.into_data() {
                                if bytes.len() > maximum.saturating_sub(*used) {
                                    return Err(limit("response bytes"));
                                }
                                *used += bytes.len();
                                return Ok(Some((bytes, (body, used))));
                            }
                        }
                    }
                }
            },
        );
        let result = if sse {
            let mut events = Box::pin(sse_stream::SseStream::from_bytes_stream(body));
            let mut count = 0;
            loop {
                let event = events
                    .next()
                    .await
                    .ok_or(TransportError::Network)?
                    .map_err(|error| match error {
                        sse_stream::Error::Body(error) => error
                            .downcast::<TransportError>()
                            .map(|error| *error)
                            .unwrap_or(TransportError::Network),
                        _ => protocol("invalid SSE stream"),
                    })?;
                let Some(data) = event.data else { continue };
                if matches!(event.event.as_deref(), None | Some("" | "message")) {
                    let message = parse_json(data.as_bytes(), self.limits.json_depth)?;
                    if let Some(result) = response_message(message, status, request_id)? {
                        break result;
                    }
                }
                count += 1;
                if count > self.limits.notifications {
                    return Err(limit("notification count").into());
                }
            }
        } else {
            let bytes = body
                .try_fold(Vec::new(), |mut result, bytes| async move {
                    result.extend_from_slice(&bytes);
                    Ok(result)
                })
                .await?;
            if !status.is_success() {
                return Err((match parse_json(&bytes, self.limits.json_depth) {
                    Ok(message) => match response_message(message, status, request_id) {
                        Err(error @ TransportError::Remote { .. }) => error,
                        _ => TransportError::HttpStatus(status.as_u16()),
                    },
                    Err(error @ TransportError::Limit(_)) => error,
                    Err(_) => TransportError::HttpStatus(status.as_u16()),
                })
                .into());
            }
            response_message(
                parse_json(&bytes, self.limits.json_depth)?,
                status,
                request_id,
            )?
            .ok_or_else(|| protocol("JSON response cannot be a notification"))?
        };
        Ok((result, used))
    }
}

fn response_message(
    mut message: Value,
    status: StatusCode,
    request_id: i64,
) -> Result<Option<Value>, TransportError> {
    let object = message
        .as_object_mut()
        .ok_or_else(|| protocol("expected one JSON-RPC object"))?;
    if object.get("jsonrpc") != Some(&json!("2.0")) {
        return Err(protocol("invalid JSON-RPC version"));
    }
    if let Some(method) = object.get("method") {
        if object.contains_key("id")
            || object.contains_key("result")
            || object.contains_key("error")
        {
            return Err(TransportError::UnsupportedCapability);
        }
        if method == "notifications/progress" {
            let params = object
                .get("params")
                .ok_or_else(|| protocol("missing progress params"))?;
            if params.get("progressToken") != Some(&json!(request_id))
                || !params.get("progress").is_some_and(Value::is_number)
            {
                return Err(protocol("invalid progress notification"));
            }
        } else if !method.is_string() {
            return Err(protocol("invalid notification method"));
        }
        return Ok(None);
    }
    if object.contains_key("error") {
        if object.contains_key("result") {
            return Err(protocol("both result and error"));
        }
        if object.get("id").is_some_and(Value::is_null) {
            return Err(protocol("invalid response ID"));
        }
        let error: JsonRpcError =
            serde_json::from_value(message).map_err(|_| protocol("invalid JSON-RPC error"))?;
        let valid_id = match &error.id {
            Some(id) => *id == RequestId::Number(request_id),
            None => matches!(error.error.code.0, -32700 | -32600),
        };
        if !valid_id {
            return Err(protocol("response ID mismatch"));
        }
        return Err(TransportError::Remote {
            code: error.error.code.0,
            message: error.error.message.into_owned(),
            data: error.error.data,
        });
    }
    if !status.is_success() {
        return Err(TransportError::HttpStatus(status.as_u16()));
    }
    if object.get("id") != Some(&json!(request_id)) {
        return Err(protocol("response ID mismatch"));
    }
    let result = object
        .remove("result")
        .ok_or_else(|| protocol("missing result"))?;
    if !result.is_object() {
        return Err(protocol("result must be an object"));
    }
    Ok(Some(result))
}

fn parse_json(bytes: &[u8], depth_limit: usize) -> Result<Value, TransportError> {
    // Check nesting before entering recursive serde code. Strings cannot make
    // braces count as structure; syntax/UTF-8 validation remains serde's job.
    let (mut depth, mut string, mut escaped) = (0usize, false, false);
    for byte in bytes {
        if string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                string = false;
            }
        } else {
            match byte {
                b'"' => string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > depth_limit {
                        return Err(limit("JSON depth"));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    stacker::maybe_grow(2 << 20, 16 << 20, || {
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        deserializer.disable_recursion_limit();
        let value = Value::deserialize(&mut deserializer).map_err(|_| protocol("invalid JSON"))?;
        deserializer
            .end()
            .map_err(|_| protocol("trailing JSON data"))?;
        Ok(value)
    })
}

#[cfg(test)]
mod tests;
