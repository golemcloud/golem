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

//! The promise service of a never-live owner. It refuses to create or complete a promise
//! and lets a poll through.

use super::*;

pub(crate) struct RefusingPromiseService {
    pub(crate) inner: Arc<dyn PromiseService>,
}

#[async_trait]
impl PromiseService for RefusingPromiseService {
    async fn create(
        &self,
        _agent_id: &AgentId,
        _fingerprint: AgentFingerprint,
        _oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal("promise::create"))
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
        self.inner.poll(promise_id).await
    }

    async fn complete(
        &self,
        _promise_id: PromiseId,
        _data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal(
            "promise::complete",
        ))
    }

    async fn complete_without_wakeup(
        &self,
        _promise_id: PromiseId,
        _data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        Err(crate::durable_host::never_live::refusal(
            "promise::complete_without_wakeup",
        ))
    }

    async fn cleanup(&self) {
        self.inner.cleanup().await
    }
}
