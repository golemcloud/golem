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

use super::replica::McpReplica;
use super::rules::{
    Admission, Refusal, RequestClass, SessionRequest, classify, local_message_check, request_checks,
};
use super::store::McpSessionId;
use crate::mcp::McpDeployment;
use crate::mcp::auth::no_mcp_deployment_response;
use poem::http::{Method, StatusCode};
use poem::{Endpoint, IntoResponse, Middleware, Request, Response, Result};
use rmcp::model::ClientJsonRpcMessage;
use std::sync::Arc;
use tracing::warn;

/// Keeps MCP sessions in front of a stateless rmcp service: issues `Mcp-Session-Id` on
/// `initialize`, admits every other request only for a live session of the request's domain
/// and environment, and serves the session's GET streams, resumes, cancels and DELETE.
pub struct McpSessionLayer {
    replica: Arc<McpReplica>,
}

impl McpSessionLayer {
    pub fn new(replica: Arc<McpReplica>) -> Self {
        Self { replica }
    }
}

impl<E: Endpoint> Middleware<E> for McpSessionLayer {
    type Output = McpSessionEndpoint<E>;

    fn transform(&self, inner: E) -> Self::Output {
        McpSessionEndpoint {
            inner,
            replica: self.replica.clone(),
        }
    }
}

pub struct McpSessionEndpoint<E> {
    inner: E,
    replica: Arc<McpReplica>,
}

impl<E: Endpoint> Endpoint for McpSessionEndpoint<E> {
    type Output = Response;

    async fn call(&self, mut request: Request) -> Result<Self::Output> {
        if let Some(refusal) = request_checks(request.method(), request.headers()) {
            return Ok(refused(refusal));
        }
        let parsed = if request.method() == Method::POST {
            let bytes = request.take_body().into_bytes().await?;
            let parsed = serde_json::from_slice::<ClientJsonRpcMessage>(&bytes);
            request.set_body(bytes);
            Some(parsed)
        } else {
            None
        };
        match classify(request.method(), request.headers(), parsed.as_ref()) {
            RequestClass::Initialize => self.initialize(request).await,
            RequestClass::PassThrough => self.forward(request).await,
            RequestClass::MissingSession => Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("Bad Request: Session ID is required")),
            RequestClass::MalformedSession => Ok(session_not_found()),
            RequestClass::Session(session, kind) => self.session(request, session, kind).await,
        }
    }
}

impl<E: Endpoint> McpSessionEndpoint<E> {
    async fn forward(&self, request: Request) -> Result<Response> {
        self.inner
            .call(request)
            .await
            .map(IntoResponse::into_response)
    }

    async fn initialize(&self, request: Request) -> Result<Response> {
        let Some(deployment) = request.extensions().get::<Arc<McpDeployment>>().cloned() else {
            return Ok(no_mcp_deployment_response());
        };
        let response = self.forward(request).await?;
        if !response.status().is_success() {
            return Ok(response);
        }
        Ok(self
            .replica
            .open_session(response, &deployment)
            .await
            .unwrap_or_else(unavailable))
    }

    async fn session(
        &self,
        mut request: Request,
        session: McpSessionId,
        kind: SessionRequest,
    ) -> Result<Response> {
        let Some(deployment) = request.extensions().get::<Arc<McpDeployment>>().cloned() else {
            return Ok(no_mcp_deployment_response());
        };
        if let Some(refusal) = local_message_check(&kind, request.headers()) {
            return Ok(refused(refusal));
        }
        let pin = match self.replica.admit(session, &deployment.compiled).await {
            Admission::Admitted(pin) => pin,
            Admission::Gone => return Ok(session_not_found()),
            Admission::Unavailable => return Ok(unavailable()),
        };
        match kind {
            SessionRequest::Call(request_id) => {
                let (serving, call) = self
                    .replica
                    .start_call(session, request_id, pin, deployment);
                request.extensions_mut().insert(serving);
                let response = self.forward(request).await?;
                Ok(call.respond(response))
            }
            SessionRequest::Cancel(request_id) => {
                if let Some(request_id) = request_id {
                    self.replica.cancel_request(session, &request_id).await;
                }
                Ok(accepted())
            }
            SessionRequest::Forward => self.forward(request).await,
            SessionRequest::Reply => Ok(accepted()),
            SessionRequest::Stream => Ok(self.replica.notification_stream(session)),
            SessionRequest::Resume(last_event_id) => {
                Ok(self.replica.resume(&last_event_id, session))
            }
            SessionRequest::Close => match self.replica.close_session(session).await {
                Ok(()) => Ok(accepted()),
                Err(error) => {
                    warn!(error = %error, "Failed to delete an MCP session");
                    Ok(unavailable())
                }
            },
        }
    }
}

fn refused(refusal: Refusal) -> Response {
    refusal
        .headers
        .iter()
        .fold(
            Response::builder().status(refusal.status),
            |response, (name, value)| response.header(*name, *value),
        )
        .body(refusal.text.into_owned())
}

fn session_not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body("Not Found: Session not found")
}

fn unavailable() -> Response {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .finish()
}

fn accepted() -> Response {
    Response::builder().status(StatusCode::ACCEPTED).finish()
}
