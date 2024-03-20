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

use anyhow::anyhow;
use async_trait::async_trait;
use tracing::debug;
use uuid::Uuid;

use crate::durable_host::wasm_rpc::UriExtensions;
use crate::durable_host::DurableWorkerCtx;
use crate::metrics::wasm::record_host_function_call;
use crate::model::InterruptKind;
use crate::preview2::golem;
use crate::preview2::golem::api::host::OplogIndex;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::OplogEntry;
use golem_common::model::regions::OplogRegion;
use golem_common::model::{PromiseId, TemplateId, Timestamp, WorkerId};

#[async_trait]
impl<Ctx: WorkerCtx> golem::api::host::Host for DurableWorkerCtx<Ctx> {
    async fn golem_create_promise(&mut self) -> Result<golem::api::host::PromiseId, anyhow::Error> {
        record_host_function_call("golem::api", "golem_create_promise");
        Ok(
            DurableWorkerCtx::create_promise(self, self.private_state.oplog_idx)
                .await
                .into(),
        )
    }

    async fn golem_await_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
    ) -> Result<Vec<u8>, anyhow::Error> {
        record_host_function_call("golem::api", "golem_await_promise");
        let promise_id: PromiseId = promise_id.into();
        match DurableWorkerCtx::poll_promise(self, promise_id.clone()).await? {
            Some(result) => Ok(result),
            None => {
                debug!("Suspending worker until {} gets completed", promise_id);
                Err(InterruptKind::Suspend.into())
            }
        }
    }

    async fn golem_complete_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
        data: Vec<u8>,
    ) -> Result<bool, anyhow::Error> {
        record_host_function_call("golem::api", "golem_complete_promise");
        Ok(DurableWorkerCtx::complete_promise(self, promise_id.into(), data).await?)
    }

    async fn golem_delete_promise(
        &mut self,
        promise_id: golem::api::host::PromiseId,
    ) -> Result<(), anyhow::Error> {
        record_host_function_call("golem::api", "golem_delete_promise");
        self.public_state
            .promise_service
            .delete(promise_id.into())
            .await;
        Ok(())
    }

    async fn get_self_uri(
        &mut self,
        function_name: String,
    ) -> Result<golem::rpc::types::Uri, anyhow::Error> {
        record_host_function_call("golem::api", "get_self_uri");
        let uri = golem_wasm_rpc::golem::rpc::types::Uri::golem_uri(
            &self.private_state.worker_id,
            Some(&function_name),
        );
        Ok(golem::rpc::types::Uri { value: uri.value })
    }

    async fn get_oplog_index(&mut self) -> anyhow::Result<OplogIndex> {
        record_host_function_call("golem::api", "get_oplog_index");
        let result = self.private_state.oplog_idx;
        if self.is_live() {
            self.set_oplog_entry(OplogEntry::nop(Timestamp::now_utc()))
                .await;
        } else {
            let _ = self.get_oplog_entry_marker().await?;
        }
        Ok(result)
    }

    async fn set_oplog_index(&mut self, oplog_idx: OplogIndex) -> anyhow::Result<()> {
        record_host_function_call("golem::api", "set_oplog_index");
        let jump_source = self.private_state.oplog_idx;
        let jump_target = oplog_idx;
        if jump_target > jump_source {
            Err(anyhow!(
                "Attempted to jump forward in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self
            .private_state
            .deleted_regions
            .is_in_deleted_region(jump_target)
        {
            Err(anyhow!(
                "Attempted to jump to a deleted region in oplog to index {jump_target} from {jump_source}"
            ))
        } else if self.is_live() {
            let jump = OplogRegion {
                start: jump_target,
                end: jump_source,
            };

            // Write an oplog entry with the new jump and then restart the worker
            self.private_state.deleted_regions.add(jump.clone());
            self.set_oplog_entry(OplogEntry::jump(Timestamp::now_utc(), jump))
                .await;
            self.commit_oplog().await;

            debug!(
                "Interrupting live execution of {} for jumping from {jump_source} to {jump_target}",
                self.worker_id
            );
            Err(InterruptKind::Jump.into())
        } else {
            // In replay mode we never have to do anything here
            debug!("Ignoring replayed set_oplog_index for {}", self.worker_id);
            Ok(())
        }
    }
}

impl From<WorkerId> for golem::api::host::WorkerId {
    fn from(worker_id: WorkerId) -> Self {
        golem::api::host::WorkerId {
            template_id: worker_id.template_id.into(),
            worker_name: worker_id.worker_name,
        }
    }
}

impl From<golem::api::host::WorkerId> for WorkerId {
    fn from(host: golem::api::host::WorkerId) -> Self {
        Self {
            template_id: host.template_id.into(),
            worker_name: host.worker_name,
        }
    }
}

impl From<golem::api::host::TemplateId> for TemplateId {
    fn from(host: golem::api::host::TemplateId) -> Self {
        let high_bits = host.uuid.high_bits;
        let low_bits = host.uuid.low_bits;

        Self(Uuid::from_u64_pair(high_bits, low_bits))
    }
}

impl From<TemplateId> for golem::api::host::TemplateId {
    fn from(template_id: TemplateId) -> Self {
        let (high_bits, low_bits) = template_id.0.as_u64_pair();

        golem::api::host::TemplateId {
            uuid: golem::api::host::Uuid {
                high_bits,
                low_bits,
            },
        }
    }
}

impl From<PromiseId> for golem::api::host::PromiseId {
    fn from(promise_id: PromiseId) -> Self {
        golem::api::host::PromiseId {
            worker_id: promise_id.worker_id.into(),
            oplog_idx: promise_id.oplog_idx,
        }
    }
}

impl From<golem::api::host::PromiseId> for PromiseId {
    fn from(host: golem::api::host::PromiseId) -> Self {
        Self {
            worker_id: host.worker_id.into(),
            oplog_idx: host.oplog_idx,
        }
    }
}
