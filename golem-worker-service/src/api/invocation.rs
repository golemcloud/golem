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

use crate::api::agents::NativeToolResult;
use crate::api::common::ApiEndpointError;
use crate::service::auth::AuthService;
use crate::service::worker::{InvocationListQuery, WorkerService};
use golem_common::base_model::api;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::poem::datetime::OpenApiTimestamp;
use golem_common::model::{AgentId, IdempotencyKey, OwnedInvocationId};
use golem_common::recorded_http_api_request;
use golem_common::schema::ExternalTypedSchemaValue;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::{Header, Path, Query};
use poem_openapi::payload::Json;
use poem_openapi_derive::{Enum, Object, OpenApi, Union};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::Instrument;

type Result<T> = std::result::Result<T, ApiEndpointError>;

/// The invocations of agents in one environment.
///
/// An invocation ID is `<agent ID>.<UUID>`. The ID resolves only in the environment of its
/// agent; in any other environment the invocation is not found.
///
/// Until invocation grants exist, `get`, `list` and `lookup` require the agent's `view`
/// permission, and `cancel` its `cancel-invocation` permission.
pub struct InvocationApi {
    worker_service: Arc<WorkerService>,
    auth_service: Arc<dyn AuthService>,
}

#[OpenApi(prefix_path = "/v1/runtime/envs", tag = ApiTags::Invocation)]
impl InvocationApi {
    pub fn new(worker_service: Arc<WorkerService>, auth_service: Arc<dyn AuthService>) -> Self {
        Self {
            worker_service,
            auth_service,
        }
    }

    /// List the invocations of one agent
    ///
    /// Newest change first, with the invocation ID as tie-breaker. Pass `nextCursor` of a page as
    /// `cursor` to get the next one; an invocation that changes while paging appears at most once.
    /// `since` and `until` are inclusive bounds on the last change.
    #[oai(
        path = "/:environment_id/invocations",
        method = "get",
        operation_id = "list_invocations"
    )]
    #[allow(clippy::too_many_arguments)]
    async fn list_invocations(
        &self,
        environment_id: Path<EnvironmentId>,
        /// The agent ID
        agent: Query<AgentId>,
        /// Only invocations of this method name or `tool:<tool>/<command path>`
        operation: Query<Option<String>>,
        /// Only invocations in one of these states
        #[oai(default)]
        state: Query<Vec<InvocationState>>,
        kind: Query<Option<InvocationKind>>,
        since: Query<Option<OpenApiTimestamp>>,
        until: Query<Option<OpenApiTimestamp>>,
        /// At most 1000; the default is 100
        #[oai(name = "pageSize")]
        page_size: Query<Option<u32>>,
        cursor: Query<Option<String>>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvocationPage>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = agent.0;
        let record = recorded_http_api_request!(
            "list_invocations",
            environment_id = %environment_id.0,
            agent_id = %agent_id,
        );
        let query = InvocationListQuery {
            operation: operation.0,
            states: state.0,
            kind: kind.0,
            since: since.0.map(|value| value.0),
            until: until.0.map(|value| value.0),
            page_size: page_size.0,
            cursor: cursor.0,
        };
        let response = self
            .worker_service
            .list_invocations(environment_id.0, &agent_id, query, auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(Into::into);
        record.result(response)
    }

    /// Find the invocations that the caller started on an agent with an idempotency key
    ///
    /// The key is matched in the caller's own idempotency scope, against every method and tool
    /// command the agent ran, including those a later revision removed. A removed method is
    /// found only while one of its invocations has the method name outside an external payload;
    /// otherwise pass `operation`. A caller without a scope, and a key used through an HTTP
    /// route, find nothing. No match is `404`.
    #[oai(
        path = "/:environment_id/invocations/lookup",
        method = "get",
        operation_id = "lookup_invocations"
    )]
    async fn lookup_invocations(
        &self,
        environment_id: Path<EnvironmentId>,
        /// The agent ID
        agent: Query<AgentId>,
        /// Only the invocation of this method name or `tool:<tool>/<command path>`
        operation: Query<Option<String>>,
        #[oai(name = "Idempotency-Key")] idempotency_key: Header<IdempotencyKey>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvocationLookupResult>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let agent_id = agent.0;
        let record = recorded_http_api_request!(
            "lookup_invocations",
            environment_id = %environment_id.0,
            agent_id = %agent_id,
        );
        let response = self
            .worker_service
            .lookup_invocations(
                environment_id.0,
                &agent_id,
                &idempotency_key.0,
                operation.0,
                auth,
            )
            .instrument(record.span.clone())
            .await
            .map(|invocations| Json(InvocationLookupResult { invocations }))
            .map_err(Into::into);
        record.result(response)
    }

    /// Get an invocation, with its result once it is terminal
    #[oai(
        path = "/:environment_id/invocations/:invocation_id",
        method = "get",
        operation_id = "get_invocation"
    )]
    async fn get_invocation(
        &self,
        environment_id: Path<EnvironmentId>,
        invocation_id: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Invocation>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let invocation_id = parse_invocation_id(&invocation_id.0)?;
        let record = recorded_http_api_request!(
            "get_invocation",
            environment_id = %environment_id.0,
            invocation_id = %invocation_id,
        );
        let response = self
            .worker_service
            .get_invocation(environment_id.0, &invocation_id, auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(Into::into);
        record.result(response)
    }

    /// Cancel an invocation
    ///
    /// A `pending` or `scheduled` invocation becomes `cancelled` at once and never runs. A
    /// terminal invocation keeps its state. Both answer `200` with the invocation's state.
    /// Cancelling an invocation that already started is not supported yet and answers `409`
    /// with the code `CANNOT_CANCEL_RUNNING_INVOCATION`.
    #[oai(
        path = "/:environment_id/invocations/:invocation_id/cancel",
        method = "post",
        operation_id = "cancel_invocation"
    )]
    async fn cancel_invocation(
        &self,
        environment_id: Path<EnvironmentId>,
        invocation_id: Path<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<InvocationSummary>> {
        let auth = self.auth_service.authenticate_token(token.secret()).await?;
        let invocation_id = parse_invocation_id(&invocation_id.0)?;
        let record = recorded_http_api_request!(
            "cancel_invocation",
            environment_id = %environment_id.0,
            invocation_id = %invocation_id,
        );
        let response = self
            .worker_service
            .cancel_invocation_by_id(environment_id.0, &invocation_id, auth)
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(Into::into);
        record.result(response)
    }
}

fn parse_invocation_id(text: &str) -> Result<OwnedInvocationId> {
    text.parse().map_err(|error: String| {
        ApiEndpointError::bad_request(api::error_code::VALIDATION_ERROR, golem_common::safe(error))
    })
}

/// The lifecycle state of an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Enum)]
#[oai(rename_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum InvocationState {
    /// Accepted with a future start time
    Scheduled,
    /// Accepted, not yet started
    Pending,
    /// Executing or ready to execute
    Running,
    /// Blocked on a promise, a timer or a call
    Waiting,
    /// Waiting to retry under the retry policy
    Retrying,
    /// Paused by an operator
    Interrupted,
    /// Ended with its result (terminal)
    Completed,
    /// Ended by a failure (terminal)
    Failed,
    /// Ended by cancellation (terminal)
    Cancelled,
}

impl InvocationState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Enum)]
#[oai(rename_all = "kebab-case")]
#[serde(rename_all = "kebab-case")]
pub enum InvocationKind {
    Method,
    Constructor,
    Tool,
}

/// One invocation of an agent. `kind` and `operation` are absent when the invocation's request
/// is not known.
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct InvocationSummary {
    /// `<agent ID>.<UUID>`
    pub invocation_id: String,
    pub agent_id: AgentId,
    pub environment_id: EnvironmentId,
    pub state: InvocationState,
    pub kind: Option<InvocationKind>,
    /// The method name, or `tool:<tool>/<command path>`. Absent for the constructor run.
    pub operation: Option<String>,
    pub accepted_at: Option<OpenApiTimestamp>,
    pub changed_at: OpenApiTimestamp,
}

/// An invocation with its result. `result` is present once the invocation is `completed` or
/// `failed`; a completed constructor run has no result.
#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Invocation {
    pub summary: InvocationSummary,
    pub result: Option<InvocationOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Union)]
#[oai(discriminator_name = "type", one_of = true)]
#[serde(tag = "type")]
#[allow(clippy::large_enum_variant)]
pub enum InvocationOutcome {
    Method(InvocationMethodResult),
    Tool(InvocationToolResult),
    Failure(InvocationFailure),
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct InvocationMethodResult {
    pub value: ExternalTypedSchemaValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct InvocationToolResult {
    pub result: NativeToolResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct InvocationFailure {
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[oai(rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct InvocationPage {
    pub invocations: Vec<InvocationSummary>,
    /// Present when more invocations may follow
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
pub struct InvocationLookupResult {
    pub invocations: Vec<InvocationSummary>,
}
