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

//! Host-side binding for the opaque `golem:core/types.permission-card`
//! resource and the [`PermissionCardResolver`] boundary bridge.
//!
//! The resource itself is defined in `golem:core/types` (so it can travel
//! inside a `schema-value-tree` as an opaque owned handle) and is bound to the
//! opaque [`PermissionCardHandleRep`] by golem-schema. The only operation the
//! core interface declares for it is `drop`, which releases the handle from the
//! resource table.
//!
//! The resolver stores the trusted [`PermissionCardValuePayload`] snapshot
//! directly inside the handle rep. Lifting a handle consumes it and returns the
//! snapshot; lowering a snapshot materializes a fresh handle. No durable oplog
//! entry is written here; the durable/cross-executor representation is the
//! snapshot embedded in the surrounding value, never the live handle.

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::schema_value::PermissionCardValuePayload;
use golem_schema::schema::wit::wire::HostPermissionCard;
use golem_schema::schema::wit::{PermissionCardHandleRep, PermissionCardResolver};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::Resource;

impl<Ctx: WorkerCtx> HostPermissionCard for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<PermissionCardHandleRep>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::core::permission-card", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> PermissionCardResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_permission_card_handle(
        &mut self,
        handle: Resource<PermissionCardHandleRep>,
    ) -> Result<PermissionCardValuePayload, Self::Error> {
        let rep = self.table().delete(handle).map_err(|e| {
            WorkerExecutorError::runtime(format!("invalid permission-card handle: {e}"))
        })?;
        rep.into_payload::<PermissionCardValuePayload>()
            .map_err(|_| {
                WorkerExecutorError::runtime("permission-card resource had unexpected payload type")
            })
    }

    fn permission_card_handle_from_snapshot(
        &mut self,
        snapshot: &PermissionCardValuePayload,
    ) -> Result<Resource<PermissionCardHandleRep>, Self::Error> {
        self.table()
            .push(PermissionCardHandleRep::new(snapshot.clone()))
            .map_err(|e| {
                WorkerExecutorError::runtime(format!(
                    "failed to create permission-card handle: {e}"
                ))
            })
    }

    fn drop_permission_card_handle(&mut self, handle: Resource<PermissionCardHandleRep>) {
        let _ = self.table().delete(handle);
    }
}
