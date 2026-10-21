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

use super::GetPromiseResultEntry;
use crate::services::promise::{
    DefaultPromiseService, PromiseHandle, PromiseService, PromiseWorkerAccess,
};
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use async_trait::async_trait;
use golem_common::base_model::component::ComponentId;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{AgentId, PromiseId};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use test_r::test;
use uuid::Uuid;

test_r::enable!();

struct NoopPromiseWorkerAccess;

#[async_trait]
impl PromiseWorkerAccess for NoopPromiseWorkerAccess {
    async fn activate_worker_if_needed(
        &self,
        _promise_id: &PromiseId,
    ) -> Result<(), WorkerExecutorError> {
        Ok(())
    }
}

/// Fails the first `remaining_failures` polls the way a key-value storage outage would, then
/// delegates to a working promise service.
struct FlakyPromiseService {
    inner: DefaultPromiseService,
    remaining_failures: AtomicU32,
    polls: AtomicU32,
}

impl FlakyPromiseService {
    fn new(failures: u32) -> Self {
        Self {
            inner: DefaultPromiseService::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                Arc::new(NoopPromiseWorkerAccess),
            ),
            remaining_failures: AtomicU32::new(failures),
            polls: AtomicU32::new(0),
        }
    }

    fn polls(&self) -> u32 {
        self.polls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PromiseService for FlakyPromiseService {
    async fn create(
        &self,
        agent_id: &AgentId,
        oplog_idx: OplogIndex,
    ) -> Result<PromiseId, WorkerExecutorError> {
        self.inner.create(agent_id, oplog_idx).await
    }

    async fn poll(&self, promise_id: PromiseId) -> Result<PromiseHandle, WorkerExecutorError> {
        self.polls.fetch_add(1, Ordering::SeqCst);
        if self
            .remaining_failures
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(WorkerExecutorError::runtime(
                "key-value storage unavailable",
            ));
        }
        self.inner.poll(promise_id).await
    }

    async fn complete(
        &self,
        promise_id: PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, WorkerExecutorError> {
        self.inner.complete(promise_id, data).await
    }

    async fn cleanup(&self) {
        self.inner.cleanup().await
    }
}

fn agent_id() -> AgentId {
    AgentId {
        component_id: ComponentId(Uuid::new_v4()),
        agent_id: "promise-pollable-test".to_string(),
    }
}

#[test]
async fn transient_poll_failure_is_not_memoized() {
    let promise_service = Arc::new(FlakyPromiseService::new(1));
    let promise_id = promise_service
        .create(&agent_id(), OplogIndex::from_u64(1))
        .await
        .unwrap();
    let entry = GetPromiseResultEntry::new(
        promise_id.clone(),
        promise_service.clone() as Arc<dyn PromiseService>,
    );

    assert!(entry.get_handle().await.is_err());
    // The failure must not be cached: the next call retries and resolves the handle.
    assert!(entry.get_handle().await.is_ok());
    assert_eq!(promise_service.polls(), 2);

    // And once resolved, the handle is memoized rather than re-polled.
    assert!(entry.get_handle().await.is_ok());
    assert_eq!(promise_service.polls(), 2);
}

#[test]
async fn resolved_handle_observes_completion() {
    let promise_service = Arc::new(FlakyPromiseService::new(1));
    let promise_id = promise_service
        .create(&agent_id(), OplogIndex::from_u64(1))
        .await
        .unwrap();
    let entry = GetPromiseResultEntry::new(
        promise_id.clone(),
        promise_service.clone() as Arc<dyn PromiseService>,
    );

    assert!(entry.get_handle().await.is_err());
    assert!(!entry.is_ready().await);

    assert!(
        promise_service
            .complete(promise_id, vec![42])
            .await
            .unwrap()
    );
    assert!(entry.is_ready().await);
    assert_eq!(
        entry.get_handle().await.unwrap().get().await,
        Some(vec![42])
    );
}
