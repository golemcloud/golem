// Copyright 2024-2026 Golem Cloud
// Licensed under the Apache License, Version 2.0 (https://www.apache.org/licenses/LICENSE-2.0).

use crate::agentic::{AgentStream, Config, ConfigSchema};
use crate::{FromSchema, IntoSchema};

/// One canonical header occurrence. Values are bytes, not UTF-8 strings.
#[derive(Clone, IntoSchema, FromSchema)]
pub struct Header {
    pub name: String,
    pub value: Vec<u8>,
}

/// The original public request. Bodies are lazy native streams and must not be logged.
#[derive(IntoSchema, FromSchema)]
pub struct HttpRequest {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<Header>,
    pub body: AgentStream<Vec<u8>>,
}

/// A streaming response. The host validates the head and owns HTTP framing.
///
/// Keep repeated headers as separate entries, including `set-cookie`. Dropping
/// the body disposes its producer; this is distinct from invocation cancellation.
#[derive(IntoSchema, FromSchema)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    pub body: AgentStream<Vec<u8>>,
}

/// Original request fields retained in an adapted `http::Request`'s extensions.
/// Framework header/URI views never replace this canonical information.
#[derive(Clone)]
pub struct OriginalHttpRequest {
    pub method: String,
    pub scheme: String,
    pub authority: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<Header>,
}

impl HttpRequest {
    /// Converts the head to Rust HTTP types, transferring the unread body directly.
    ///
    /// The URI retains the full public path, not a mount-relative path. Host is
    /// synthesized from authority. Accepted duplicate headers are appended, never
    /// folded. `http` rejects heads it cannot represent; the canonical API remains
    /// available independently. No transport/version semantics are inferred from
    /// the `http::Request` version default.
    pub fn into_http(self) -> Result<http::Request<AgentStream<Vec<u8>>>, http::Error> {
        let original = OriginalHttpRequest {
            method: self.method.clone(),
            scheme: self.scheme.clone(),
            authority: self.authority.clone(),
            path: self.path.clone(),
            query: self.query.clone(),
            headers: self.headers.clone(),
        };
        let query = self
            .query
            .as_ref()
            .map(|query| format!("?{query}"))
            .unwrap_or_default();
        let mut request = http::Request::builder()
            .method(self.method.as_str())
            .uri(format!(
                "{}://{}{}{query}",
                self.scheme, self.authority, self.path
            ))
            .header(http::header::HOST, self.authority);
        for header in self.headers {
            request = request.header(header.name, header.value);
        }
        let mut request = request.body(self.body)?;
        request.extensions_mut().insert(original);
        Ok(request)
    }
}

impl From<http::Response<AgentStream<Vec<u8>>>> for HttpResponse {
    fn from(response: http::Response<AgentStream<Vec<u8>>>) -> Self {
        let (parts, body) = response.into_parts();
        Self {
            status: parts.status.as_u16(),
            headers: parts
                .headers
                .iter()
                .map(|(name, value)| Header {
                    name: name.as_str().to_string(),
                    value: value.as_bytes().to_vec(),
                })
                .collect(),
            body,
        }
    }
}

/// Implement with `#[http_router(name = "Site", mount = "/site")]`.
///
/// Only explicitly implemented `handle` and `openapi` methods are registered.
/// Omitting `handle` supports static-only/provider-only routers without a fake
/// endpoint. Calling an omitted method directly is a programming error.
///
/// Routers are parameterless ephemeral agents with snapshots disabled. `Config`
/// is injected through the same configuration/secret mechanism as regular agents;
/// use `type Config = ()` when no configuration is needed. Dependencies use normal
/// manifest-declared generated clients. Keep helper methods in an inherent impl.
#[allow(async_fn_in_trait)]
pub trait HttpRouter: Sized + 'static {
    type Config: ConfigSchema;

    fn new(config: Config<Self::Config>) -> Self;

    async fn handle(&self, _request: HttpRequest) -> HttpResponse {
        panic!("HttpRouter::handle is not implemented or registered")
    }

    /// Returns OpenAPI 3.1.0 JSON, not YAML or a response envelope. The host
    /// validates, rebases and merges the document only for OpenAPI requests.
    async fn openapi(&self) -> String {
        panic!("HttpRouter::openapi is not implemented or registered")
    }
}
