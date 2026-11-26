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

use crate::{TestWorkerExecutor, callers};
use golem_api_grpc::proto::golem::worker::{
    AgentInvocationMode, AgentTarget, ExternalToolInvocation, InvocationRequest,
    InvocationSessionResult, InvocationStart, agent_target, invocation_request,
    invocation_response, invocation_session_completion::Outcome,
};
use golem_common::model::component::ComponentDto;
use golem_common::model::tool::ToolName;
use golem_common::model::type_id::TypeId;
use golem_common::model::{
    AgentId, IdempotencyKey, InvocationId, InvocationRequestKey, OwnedAgentId,
    StandaloneToolInvocation,
};
use golem_common::schema::TypedSchemaValue;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

/// A standalone call and its derived public identity. The protobuf remains available for
/// tests that deliberately send a conflicting or invalid request.
pub struct StandaloneToolCall {
    pub owned: OwnedAgentId,
    pub invocation_id: InvocationId,
    pub request: InvocationStart,
}

impl TestWorkerExecutor {
    pub fn standalone_tool(
        &self,
        component: &ComponentDto,
        type_id: TypeId,
        tool: &str,
        command: &str,
        key: &IdempotencyKey,
        input: TypedSchemaValue,
    ) -> anyhow::Result<StandaloneToolCall> {
        let environment_id = component.environment_id;
        let account_id = component.account_id;
        let component_id = component.id;
        let tool_name = ToolName::try_from(tool).map_err(anyhow::Error::msg)?;
        let scope = self.auth_ctx().idempotency_scope();
        let invocation_id = StandaloneToolInvocation::unique_part(
            environment_id,
            &tool_name,
            scope,
            command,
            Some(key),
        );
        let owned = OwnedAgentId::new(
            environment_id,
            &AgentId::standalone_tool_invocation(environment_id, type_id, invocation_id),
        );
        let request = InvocationStart {
            target: Some(AgentTarget {
                target: Some(agent_target::Target::Id(owned.agent_id.into())),
            }),
            component_id: Some(component_id.into()),
            idempotency_key: Some(IdempotencyKey::new(invocation_id.to_string()).into()),
            auth_ctx: Some(self.auth_ctx().into()),
            caller: Some(callers::account(account_id).into()),
            environment_id: Some(environment_id.into()),
            component_owner_account_id: Some(account_id.into()),
            attempt_id: Some(Uuid::new_v4().into()),
            mode: AgentInvocationMode::Await as i32,
            external_tool: Some(ExternalToolInvocation {
                tool_name: tool.into(),
                command_path: vec![command.into()],
                input: Some(input.try_into().map_err(anyhow::Error::msg)?),
                fresh_owner: true,
                ..Default::default()
            }),
            request_key: Some(InvocationRequestKey::external(scope, Some(key)).into()),
            ..Default::default()
        };
        Ok(StandaloneToolCall {
            owned,
            invocation_id,
            request,
        })
    }
}

/// Keeps the request side alive while a concurrent caller observes the response side.
pub struct AttachedToolSession {
    _requests: mpsc::Sender<InvocationRequest>,
    responses: tonic::Streaming<golem_api_grpc::proto::golem::worker::InvocationResponse>,
}

impl StandaloneToolCall {
    pub async fn start(&self, executor: &TestWorkerExecutor) -> anyhow::Result<()> {
        let mut start = self.request.clone();
        start.mode = AgentInvocationMode::Schedule as i32;
        executor.invoke_agent_session(start).await?;
        Ok(())
    }

    pub async fn result(
        &self,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<InvocationSessionResult> {
        let mut start = self.request.clone();
        start.mode = AgentInvocationMode::Await as i32;
        start.schedule_at = None;
        executor.invoke_agent_session(start).await
    }

    pub async fn attach(
        &self,
        executor: &TestWorkerExecutor,
    ) -> anyhow::Result<AttachedToolSession> {
        let (requests, receiver) = mpsc::channel(1);
        let mut start = self.request.clone();
        start.attempt_id = Some(Uuid::new_v4().into());
        requests
            .send(InvocationRequest {
                request: Some(invocation_request::Request::Start(start)),
            })
            .await?;
        let mut responses = executor
            .client
            .clone()
            .invoke_agent_session(ReceiverStream::new(receiver))
            .await?
            .into_inner();
        let accepted = responses.message().await?.expect("same-key acceptance");
        assert!(
            matches!(
                accepted.response,
                Some(invocation_response::Response::Accepted(_))
            ),
            "{accepted:?}"
        );
        Ok(AttachedToolSession {
            _requests: requests,
            responses,
        })
    }
}

impl AttachedToolSession {
    pub async fn result(mut self) -> anyhow::Result<InvocationSessionResult> {
        let mut result = None;
        let mut finished = 0;
        while let Some(response) = self.responses.message().await? {
            match response.response {
                Some(invocation_response::Response::Result(value)) => {
                    assert!(
                        result.replace(value).is_none(),
                        "one result per attached session"
                    );
                }
                Some(invocation_response::Response::Finished(value)) => {
                    assert!(
                        matches!(value.outcome, Some(Outcome::Success(_))),
                        "attached session must finish successfully"
                    );
                    finished += 1;
                }
                Some(invocation_response::Response::Ready(_)) => {}
                other => anyhow::bail!("unexpected same-key response: {other:?}"),
            }
        }
        assert_eq!(finished, 1, "one Finished per attached session");
        Ok(result.expect("same-key result"))
    }
}
