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

//! Invocation requests and raw invocation sessions on the test executor.
//!
//! [`TestWorkerExecutor::invocation`] builds an [`InvocationStart`] that names only what a test
//! varies. [`TestWorkerExecutor::open_session`] sends it and gives an [`InvocationSession`], which
//! validates every frame in both directions. Both types are shared with the integration-test DSL
//! in [`golem_test_framework::dsl::invocation_session`].

use crate::TestWorkerExecutor;
use golem_api_grpc::proto::golem::worker::{InvocationStart, ResumeAttach, invocation_request};
use golem_common::model::component::ComponentDto;
use golem_test_framework::dsl::invocation_session::{InvocationSession, InvocationStartBuilder};
use golem_worker_executor::services::agent_types::local_type_id;

impl TestWorkerExecutor {
    /// Starts an [`InvocationStartBuilder`] for `method` of an agent of `component`, with the
    /// executor's auth context and the component's owner. The type ID of an ephemeral agent
    /// type is derived locally, as the test executor registers agent types.
    pub fn invocation<'a>(
        &self,
        component: &'a ComponentDto,
        method: &str,
    ) -> InvocationStartBuilder<'a> {
        InvocationStartBuilder::new(component, method, self.auth_ctx())
            .component_owner()
            .local_type_ids(local_type_id)
    }

    /// Sends `start` and opens its session. Reads no response. The session validates every
    /// frame in both directions.
    pub async fn open_session(&self, start: InvocationStart) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::Start(start), true)
            .await
    }

    /// Sends `resume` and opens its session. Reads no response. The session validates every
    /// frame in both directions.
    pub async fn resume_session(&self, resume: ResumeAttach) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::ResumeAttach(resume), true)
            .await
    }

    /// Sends `start` and opens its session without client-side validation: the session sends
    /// and reads frames as they are, for a test of what the executor does with a frame that the
    /// protocol state would refuse.
    pub async fn open_unvalidated_session(
        &self,
        start: InvocationStart,
    ) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::Start(start), false)
            .await
    }

    /// As [`Self::open_unvalidated_session`], for a resume attach.
    pub async fn resume_unvalidated_session(
        &self,
        resume: ResumeAttach,
    ) -> anyhow::Result<InvocationSession> {
        self.open_session_with(invocation_request::Request::ResumeAttach(resume), false)
            .await
    }

    async fn open_session_with(
        &self,
        first: invocation_request::Request,
        validate: bool,
    ) -> anyhow::Result<InvocationSession> {
        let mut client = self.client.clone();
        InvocationSession::open(first, validate, |requests| async move {
            Ok(client.invoke_agent_session(requests).await?.into_inner())
        })
        .await
    }
}
