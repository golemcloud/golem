// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::error::GolemError;
use crate::services::oplog::{Oplog, OplogService};
use async_trait::async_trait;
use golem_common::model::oplog::{OplogEntry, OplogIndex};
use golem_common::model::{AccountId, ComponentId, OwnedWorkerId, ScanCursor};
use std::collections::BTreeMap;
use std::sync::Arc;

#[cfg(any(feature = "mocks", test))]
#[derive(Debug)]
pub struct OplogServiceMock {}

#[cfg(any(feature = "mocks", test))]
impl Default for OplogServiceMock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "mocks", test))]
impl OplogServiceMock {
    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(any(feature = "mocks", test))]
#[async_trait]
impl OplogService for OplogServiceMock {
    async fn create(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _initial_entry: OplogEntry,
    ) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn open(&self, _owned_worker_id: &OwnedWorkerId) -> Arc<dyn Oplog + Send + Sync> {
        unimplemented!()
    }

    async fn get_first_index(&self, _owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        unimplemented!()
    }

    async fn get_last_index(&self, _owned_worker_id: &OwnedWorkerId) -> OplogIndex {
        unimplemented!()
    }

    async fn delete(&self, _owned_worker_id: &OwnedWorkerId) {
        unimplemented!()
    }

    async fn read(
        &self,
        _owned_worker_id: &OwnedWorkerId,
        _idx: OplogIndex,
        _n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        unimplemented!()
    }

    async fn exists(&self, _owned_worker_id: &OwnedWorkerId) -> bool {
        unimplemented!()
    }

    async fn scan_for_component(
        &self,
        _account_id: &AccountId,
        _component_id: &ComponentId,
        _cursor: ScanCursor,
        _count: u64,
    ) -> Result<(ScanCursor, Vec<OwnedWorkerId>), GolemError> {
        unimplemented!()
    }
}
