// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use async_trait::async_trait;
use golem_api_grpc::proto::golem::worker::{InvocationStart, ResumeAttach};
use golem_common::model::{AgentId, IdempotencyKey as ModelIdempotencyKey};
use golem_service_base::model::auth::AuthCtx;

use crate::service::worker::{
    InvocationRequestStream, InvocationResponseStream, WorkerService, WorkerServiceError,
};
#[async_trait]
pub(super) trait SessionTransport: Send + Sync {
    async fn start(
        &self,
        start: InvocationStart,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError>;
    async fn resume(
        &self,
        resume: ResumeAttach,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError>;
    async fn cleanup(&self, agent: AgentId, key: ModelIdempotencyKey) -> CleanupOutcome;
}

pub(super) enum CleanupOutcome {
    FinishedUnconfirmed,
    RpcFailure,
}

#[async_trait]
impl SessionTransport for WorkerService {
    async fn start(
        &self,
        start: InvocationStart,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.invoke_agent_session(start, tail, true, AuthCtx::System)
            .await
    }

    async fn resume(
        &self,
        resume: ResumeAttach,
        tail: InvocationRequestStream,
    ) -> Result<InvocationResponseStream, WorkerServiceError> {
        self.resume_agent_session(resume, tail, AuthCtx::System)
            .await
    }

    async fn cleanup(&self, agent: AgentId, key: ModelIdempotencyKey) -> CleanupOutcome {
        let cancel = self.cancel_invocation(&agent, &key, AuthCtx::System);
        let interrupt = self.interrupt(&agent, false, AuthCtx::System);
        let (cancel, interrupt) = tokio::join!(cancel, interrupt);
        if cancel.is_ok() && interrupt.is_ok() {
            CleanupOutcome::FinishedUnconfirmed
        } else {
            CleanupOutcome::RpcFailure
        }
    }
}
