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

//! An oplog stack with the retention layer over an in-memory primary oplog, for the tests of the
//! layer and of the readers that go by the retained start.

use super::{OplogRetention, RetentionOplogService, advance_oplog_retention};
use crate::model::ExecutionStatus;
use crate::services::oplog::{
    CommitLevel, Oplog, OplogError, OplogReadError, OplogService, PrimaryOplogService,
};
use crate::storage::indexed::memory::InMemoryIndexedStorage;
use crate::storage::keyvalue::KeyValueStorage;
use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
use futures::{StreamExt, TryStreamExt};
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::agent::{AgentMode, OwnerKind};
use golem_common::model::caller::Caller;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::oplog::{CreateRequest, CreateRequestKind, OplogEntry, OplogIndex};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AgentFingerprint, AgentId, AgentMetadata, AgentStatusRecord, OwnedAgentId, ShardEpoch,
    Timestamp,
};
use golem_common::read_only_lock;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

pub(crate) struct RetainedOplog {
    pub service: Arc<RetentionOplogService>,
    pub owned_agent_id: OwnedAgentId,
    pub component_id: ComponentId,
    key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
}

pub(crate) fn idx(value: u64) -> OplogIndex {
    OplogIndex::from_u64(value)
}

/// An entry with a fixed timestamp, because the oplog stores timestamps in milliseconds. The jump
/// names its own index, so that each entry is different.
pub(crate) fn jump(index: u64) -> OplogEntry {
    OplogEntry::Jump {
        timestamp: Timestamp::from(index),
        entity_parent_start_index: None,
        jump: OplogRegion {
            start: idx(index),
            end: idx(index),
        },
    }
}

/// The retention whose start is at `start`.
pub(crate) fn at(start: u64) -> OplogRetention {
    OplogRetention::FULL.advanced_to(idx(start))
}

pub(crate) fn expired(start: u64) -> OplogReadError {
    OplogReadError::Expired {
        retained_start: idx(start),
    }
}

fn last_known_status() -> read_only_lock::arc_swap::ReadOnlyView<AgentStatusRecord> {
    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(arc_swap::ArcSwap::from_pointee(
        AgentStatusRecord::default(),
    )))
}

fn execution_status() -> read_only_lock::std::ReadOnlyLock<ExecutionStatus> {
    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(ExecutionStatus::Suspended {
        agent_mode: AgentMode::Durable,
        timestamp: Timestamp::now_utc(),
    })))
}

impl RetainedOplog {
    pub async fn new() -> Self {
        Self::with_key_value_storage(Arc::new(InMemoryKeyValueStorage::new())).await
    }

    /// The stack with `key_value_storage` as the storage of the retention record.
    pub async fn with_key_value_storage(
        key_value_storage: Arc<dyn KeyValueStorage + Send + Sync>,
    ) -> Self {
        let primary = PrimaryOplogService::new(
            Arc::new(InMemoryIndexedStorage::new()),
            Arc::new(InMemoryBlobStorage::new()),
            1,
            1,
            4096,
            32,
            golem_common::model::RetryConfig::default(),
        )
        .await;
        let agent_id = AgentId::from_parts(7, 1);
        Self {
            service: Arc::new(RetentionOplogService::new(
                Arc::new(primary),
                key_value_storage.clone(),
            )),
            owned_agent_id: OwnedAgentId::new(EnvironmentId::new(), &agent_id),
            component_id: ComponentId::new(),
            key_value_storage,
        }
    }

    pub fn agent_id(&self) -> &AgentId {
        &self.owned_agent_id.agent_id
    }

    fn metadata(&self) -> AgentMetadata {
        AgentMetadata {
            component_id: self.component_id,
            agent_type_name: None,
            canonical_key: None,
            constructor_parameters: None,
            agent_id: *self.agent_id(),
            owner_kind: OwnerKind::ComponentAgent,
            env: vec![],
            environment_id: self.owned_agent_id.environment_id,
            created_by: AccountId::new(),
            created_by_email: AccountEmail::new("test@golem"),
            config: Vec::new(),
            created_at: Timestamp::now_utc(),
            parent: None,
            last_known_status: AgentStatusRecord::default(),
            fingerprint: AgentFingerprint::new(),
            agent_mode: AgentMode::Durable,
            create_request: CreateRequest::new(
                CreateRequestKind::GetOrCreate,
                None,
                Caller::anonymous(),
                &InvocationContextStack::fresh_rounded(),
            ),
        }
    }

    pub async fn open(&self) -> Arc<dyn Oplog> {
        self.service
            .open(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                None,
                self.metadata(),
                last_known_status(),
                execution_status(),
                None,
            )
            .await
            .unwrap()
    }

    /// A new oplog whose first entry is `first`, with `epoch` as its shard epoch.
    pub async fn create(&self, first: OplogEntry, epoch: ShardEpoch) -> Arc<dyn Oplog> {
        self.service
            .create(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                first,
                self.metadata(),
                last_known_status(),
                execution_status(),
                Some(epoch),
            )
            .await
            .unwrap()
    }

    /// Adds `entries` in their order after the last entry, and commits them.
    pub async fn add(&self, entries: impl IntoIterator<Item = OplogEntry>) {
        let oplog = self.open().await;
        futures::stream::iter(entries)
            .then(|entry| oplog.add(entry))
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();
    }

    /// An oplog with the entries 1 to `last`, each made by [`jump`].
    pub async fn with_jumps(&self, last: u64) {
        self.add((1..=last).map(jump)).await;
    }

    pub async fn advance(&self, to: u64) -> OplogRetention {
        advance_oplog_retention(
            self.key_value_storage.as_ref(),
            &self.owned_agent_id,
            AgentMode::Durable,
            idx(to),
        )
        .await
        .unwrap()
    }

    pub async fn start(&self) -> OplogIndex {
        self.service
            .retention(&self.owned_agent_id, AgentMode::Durable)
            .await
            .unwrap()
            .start()
    }

    pub async fn delete(&self, expected_epoch: Option<ShardEpoch>) -> Result<(), OplogError> {
        self.service
            .delete(
                &mut self.service.lock_lifecycle(self.agent_id()).await,
                &self.owned_agent_id,
                AgentMode::Durable,
                expected_epoch,
            )
            .await
    }

    pub async fn read(
        &self,
        start: u64,
        n: u64,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, OplogReadError> {
        self.service
            .read_exact(&self.owned_agent_id, AgentMode::Durable, idx(start), n)
            .await
            .map_err(|unreadable| unreadable.error)
    }
}
