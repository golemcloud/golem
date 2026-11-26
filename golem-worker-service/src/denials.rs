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

//! The denial renderers of the worker service's surfaces other than REST (REST's is
//! `crate::api::common::RestDenials`).
//!
//! No call site uses them yet. Each surface's owner wires them when it wires `decide`, and also
//! sends its real "not found" answers for designated kinds through [`DenialRenderer::unknown`],
//! so that a denial and an unknown ID give the same answer (5.3 Rule 21). Where a surface already
//! answers an unknown ID, its renderer gives that same answer; a test checks each one.

use crate::api::common::{ApiEndpointError, RestDenials};
use crate::custom_api::model::RouteExecutionResult;
use golem_api_grpc::proto::golem::common::ErrorBody;
use golem_api_grpc::proto::golem::worker::v1::agent_error;
use golem_common::base_model::api::error_code;
use golem_common::model::authority::GrantText;
use golem_common::model::error::ErrorBody as RestErrorBody;
use golem_common::model::invocation_session_public::PublicErrorCode;
use golem_common::model::security_scheme::SecuritySchemeId;
use golem_service_base::model::auth::denial::{DenialRenderer, FORBIDDEN_MESSAGE, UnknownId};
use poem_openapi::payload::Json;

/// The text of an answer that asks the caller to sign in.
const SIGN_IN_REQUIRED_MESSAGE: &str = "Sign-in required";

/// HTTP routes of deployed agents, as their error path answers.
pub struct HttpRouteDenials;

impl DenialRenderer for HttpRouteDenials {
    type Response = ApiEndpointError;

    /// A route call names its agent through the invocation, so the answer carries the
    /// invocation's prefix, as a route call to a missing agent does today.
    fn unknown(&self, id: &UnknownId) -> ApiEndpointError {
        ApiEndpointError::NotFound(Json(RestErrorBody {
            error: format!("Invocation failed: {}", id.message()),
            code: id.code().to_string(),
            cause: None,
        }))
    }

    fn forbidden(&self, missing_grant: Option<&GrantText>) -> ApiEndpointError {
        RestDenials.forbidden(missing_grant)
    }

    fn sign_in_required(&self, scheme: Option<&SecuritySchemeId>) -> ApiEndpointError {
        RestDenials.sign_in_required(scheme)
    }
}

/// Durable Streams reads, which answer outside the routes' error path.
pub struct StreamReadDenials;

impl DenialRenderer for StreamReadDenials {
    type Response = RouteExecutionResult;

    /// A read of a stream that names nothing is an empty `404` that no cache keeps, whatever the
    /// kind of the missing ID.
    fn unknown(&self, _id: &UnknownId) -> RouteExecutionResult {
        crate::custom_api::durable_streams::response(http::StatusCode::NOT_FOUND)
    }

    fn forbidden(&self, _missing_grant: Option<&GrantText>) -> RouteExecutionResult {
        crate::custom_api::durable_streams::response(http::StatusCode::FORBIDDEN)
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> RouteExecutionResult {
        crate::custom_api::durable_streams::response(http::StatusCode::UNAUTHORIZED)
    }
}

/// The invocation attach protocol: the code and the message of `InvocationRejected`.
pub struct AttachDenials;

impl DenialRenderer for AttachDenials {
    type Response = (PublicErrorCode, String);

    fn unknown(&self, id: &UnknownId) -> (PublicErrorCode, String) {
        (PublicErrorCode::NotFound, id.message())
    }

    /// `InvocationRejected` has no field for a grant, so the answer names none.
    fn forbidden(&self, _missing_grant: Option<&GrantText>) -> (PublicErrorCode, String) {
        (PublicErrorCode::Unauthorized, FORBIDDEN_MESSAGE.to_string())
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> (PublicErrorCode, String) {
        (
            PublicErrorCode::Unauthorized,
            SIGN_IN_REQUIRED_MESSAGE.to_string(),
        )
    }
}

/// Typed internal calls, such as calls between agents through the worker service.
pub struct InternalCallDenials;

impl DenialRenderer for InternalCallDenials {
    type Response = agent_error::Error;

    fn unknown(&self, id: &UnknownId) -> agent_error::Error {
        agent_error::Error::NotFound(ErrorBody {
            error: id.message(),
            code: id.code().to_string(),
        })
    }

    /// The gRPC error has no field for a grant, so the answer names none.
    fn forbidden(&self, _missing_grant: Option<&GrantText>) -> agent_error::Error {
        agent_error::Error::Unauthorized(ErrorBody {
            error: FORBIDDEN_MESSAGE.to_string(),
            code: error_code::AUTH_UNAUTHORIZED.to_string(),
        })
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> agent_error::Error {
        agent_error::Error::Unauthorized(ErrorBody {
            error: SIGN_IN_REQUIRED_MESSAGE.to_string(),
            code: error_code::AUTH_UNAUTHORIZED.to_string(),
        })
    }
}

/// MCP: JSON-RPC errors.
pub struct McpDenials;

impl DenialRenderer for McpDenials {
    type Response = rmcp::ErrorData;

    /// JSON-RPC "invalid params", as for an unknown tool, and the same for every MCP protocol
    /// version.
    fn unknown(&self, id: &UnknownId) -> rmcp::ErrorData {
        rmcp::ErrorData::invalid_params(id.message(), Some(serde_json::json!({"code": id.code()})))
    }

    fn forbidden(&self, missing_grant: Option<&GrantText>) -> rmcp::ErrorData {
        let mut data = serde_json::json!({"code": error_code::AUTH_FORBIDDEN});
        if let Some(grant) = missing_grant {
            data["missingGrant"] = serde_json::Value::String(grant.0.clone());
        }
        rmcp::ErrorData::invalid_request(FORBIDDEN_MESSAGE, Some(data))
    }

    fn sign_in_required(&self, _scheme: Option<&SecuritySchemeId>) -> rmcp::ErrorData {
        rmcp::ErrorData::invalid_request(
            SIGN_IN_REQUIRED_MESSAGE,
            Some(serde_json::json!({"code": error_code::AUTH_UNAUTHORIZED})),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_api::error::RequestHandlerError;
    use crate::service::auth::AuthServiceError;
    use crate::service::worker::{PublicAgentSessionStartError, WorkerServiceError};
    use golem_common::model::AgentId;
    use golem_common::model::account::AccountEmail;
    use golem_common::model::authority::{Denial, DesignatedKind};
    use golem_common::model::card::owner::AgentOwnerPattern;
    use golem_common::model::card::{
        AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
    };
    use golem_service_base::model::auth::AuthorizationError;
    use golem_service_base::model::auth::denial::FORBIDDEN_MESSAGE;
    use poem::IntoResponse;
    use test_r::test;

    fn agent() -> AgentId {
        AgentId::from_parts(7, 3)
    }

    fn unknown_agent() -> UnknownId {
        UnknownId::new(DesignatedKind::Agent, agent())
    }

    fn looks_unknown(kind: DesignatedKind) -> Denial {
        Denial::LooksUnknown { kind }
    }

    const ALL_KINDS: [DesignatedKind; 7] = [
        DesignatedKind::Agent,
        DesignatedKind::Invocation,
        DesignatedKind::Task,
        DesignatedKind::Stream,
        DesignatedKind::Promise,
        DesignatedKind::Webhook,
        DesignatedKind::Artifact,
    ];

    fn unknown_of(kind: DesignatedKind) -> UnknownId {
        UnknownId::new(kind, "id-7")
    }

    /// A 1.6 denial as the worker service sees it today.
    fn denied_permission() -> WorkerServiceError {
        WorkerServiceError::AuthError(AuthServiceError::Unauthorized(
            AuthorizationError::PermissionNotAllowed(Box::new(PermissionTarget::Agent(
                ClassPermissionTarget {
                    owner: AgentOwnerPattern::AccountAgents {
                        account: AccountEmail::new("owner@example.com"),
                    },
                    verb: Some(AgentVerb::Invoke),
                    resource: AgentResourcePattern::Any,
                },
            ))),
        ))
    }

    async fn wire(error: ApiEndpointError) -> (poem::http::StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        (status, response.into_body().into_string().await.unwrap())
    }

    #[test]
    async fn an_http_route_denial_answers_as_a_route_call_to_an_unknown_agent() {
        let real = ApiEndpointError::from(RequestHandlerError::AgentInvocationFailed(
            WorkerServiceError::AgentNotFound(agent()),
        ));
        let rendered =
            HttpRouteDenials.render(&looks_unknown(DesignatedKind::Agent), &[unknown_agent()]);
        assert_eq!(wire(rendered).await, wire(real).await);
    }

    #[test]
    async fn every_designated_kind_answers_an_http_route_with_its_own_not_found() {
        for kind in ALL_KINDS {
            let id = unknown_of(kind);
            let (status, body) =
                wire(HttpRouteDenials.render(&looks_unknown(kind), std::slice::from_ref(&id)))
                    .await;
            assert_eq!(status, poem::http::StatusCode::NOT_FOUND, "{kind:?}");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&body).unwrap(),
                serde_json::json!({
                    "code": id.code(),
                    "error": format!("Invocation failed: {}", id.message()),
                }),
                "{kind:?}"
            );
        }
    }

    #[test]
    async fn an_http_route_forbidden_answer_names_no_target() {
        let (status, body) = wire(HttpRouteDenials.render(
            &Denial::Forbidden {
                missing_grant: None,
            },
            &[],
        ))
        .await;
        assert_eq!(status, poem::http::StatusCode::FORBIDDEN);
        assert!(body.contains(FORBIDDEN_MESSAGE), "{body}");
        assert!(!body.contains("owner@example.com"), "{body}");
    }

    #[test]
    fn a_stream_read_denial_answers_as_a_read_of_an_unknown_stream() {
        let real = crate::custom_api::durable_streams::response(poem::http::StatusCode::NOT_FOUND);
        for kind in ALL_KINDS {
            let rendered = StreamReadDenials.render(&looks_unknown(kind), &[unknown_of(kind)]);
            assert_eq!(rendered.status, real.status, "{kind:?}");
            assert_eq!(rendered.headers, real.headers, "{kind:?}");
            assert!(
                matches!(
                    rendered.body,
                    crate::custom_api::model::ResponseBody::NoBody
                ),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_attach_denial_answers_as_an_attach_to_an_unknown_agent() {
        let real = crate::api::invocation_session::public_start_error(
            PublicAgentSessionStartError::Worker(WorkerServiceError::AgentNotFound(agent())),
        );
        assert_eq!(
            AttachDenials.render(&looks_unknown(DesignatedKind::Agent), &[unknown_agent()]),
            real
        );
    }

    #[test]
    fn every_designated_kind_answers_an_attach_with_its_own_not_found() {
        for kind in ALL_KINDS {
            let id = unknown_of(kind);
            assert_eq!(
                AttachDenials.render(&looks_unknown(kind), std::slice::from_ref(&id)),
                (PublicErrorCode::NotFound, id.message()),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_attach_forbidden_answer_is_todays_answer_to_a_denied_permission() {
        let real = crate::api::invocation_session::public_start_error(
            PublicAgentSessionStartError::Worker(denied_permission()),
        );
        assert_eq!(
            AttachDenials.render(
                &Denial::Forbidden {
                    missing_grant: None
                },
                &[]
            ),
            real
        );
        assert_eq!(real.1, FORBIDDEN_MESSAGE);
    }

    #[test]
    fn an_internal_call_denial_answers_as_a_call_to_an_unknown_agent() {
        let real = agent_error::Error::from(WorkerServiceError::AgentNotFound(agent()));
        assert_eq!(
            InternalCallDenials.render(&looks_unknown(DesignatedKind::Agent), &[unknown_agent()]),
            real
        );
    }

    #[test]
    fn every_designated_kind_answers_an_internal_call_with_its_own_not_found() {
        for kind in ALL_KINDS {
            let id = unknown_of(kind);
            assert_eq!(
                InternalCallDenials.render(&looks_unknown(kind), std::slice::from_ref(&id)),
                agent_error::Error::NotFound(ErrorBody {
                    error: id.message(),
                    code: id.code().to_string(),
                }),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_internal_call_forbidden_answer_is_todays_answer_to_a_denied_permission() {
        let real = agent_error::Error::from(denied_permission());
        assert_eq!(
            InternalCallDenials.render(
                &Denial::Forbidden {
                    missing_grant: None
                },
                &[]
            ),
            real
        );
    }

    /// JSON-RPC "invalid params", as MCP answers an unknown tool, with the text and the code of
    /// the other surfaces.
    #[test]
    fn an_mcp_denial_is_an_invalid_params_error_with_the_shared_text() {
        let error = McpDenials.render(&looks_unknown(DesignatedKind::Agent), &[unknown_agent()]);
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert_eq!(error.message, unknown_agent().message());
        assert_eq!(
            error.data,
            Some(serde_json::json!({"code": unknown_agent().code()}))
        );
    }

    #[test]
    fn every_designated_kind_answers_mcp_with_its_own_not_found() {
        for kind in ALL_KINDS {
            let id = unknown_of(kind);
            let error = McpDenials.render(&looks_unknown(kind), std::slice::from_ref(&id));
            assert_eq!(
                error.code,
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "{kind:?}"
            );
            assert_eq!(error.message, id.message(), "{kind:?}");
            assert_eq!(
                error.data,
                Some(serde_json::json!({"code": id.code()})),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn an_mcp_forbidden_answer_names_a_grant_only_when_the_authorizer_gives_one() {
        let without = McpDenials.render(
            &Denial::Forbidden {
                missing_grant: None,
            },
            &[],
        );
        assert_eq!(without.code, rmcp::model::ErrorCode::INVALID_REQUEST);
        assert_eq!(without.message, FORBIDDEN_MESSAGE);
        assert_eq!(
            without.data,
            Some(serde_json::json!({"code": "AUTH_FORBIDDEN"}))
        );

        let with = McpDenials.render(
            &Denial::Forbidden {
                missing_grant: Some(GrantText("tool(E/grep) : invoke".to_string())),
            },
            &[],
        );
        assert_eq!(
            with.data,
            Some(serde_json::json!({
                "code": "AUTH_FORBIDDEN",
                "missingGrant": "tool(E/grep) : invoke",
            }))
        );
    }
}
