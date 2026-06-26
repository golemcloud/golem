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

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::secrets::reveal;
use crate::preview2::golem::secrets::types::{self, SecretId, SecretMetadata};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::schema_value::SecretValuePayload;
use golem_schema::schema::wit::wire::{HostSecret, SchemaGraph, SchemaValueTree};
use golem_schema::schema::wit::{SecretHandleRep, SecretResolver};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use wasmtime::component::Resource;

impl<Ctx: WorkerCtx> HostSecret for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<SecretHandleRep>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::core::secret", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> SecretResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_secret_handle(
        &mut self,
        handle: Resource<SecretHandleRep>,
    ) -> Result<SecretValuePayload, Self::Error> {
        self.table()
            .delete(handle)
            .map_err(|e| WorkerExecutorError::runtime(format!("invalid secret handle: {e}")))?
            .into_payload::<SecretValuePayload>()
            .map_err(|_| {
                WorkerExecutorError::runtime("secret resource had unexpected payload type")
            })
    }

    fn secret_handle_from_snapshot(
        &mut self,
        snapshot: &SecretValuePayload,
    ) -> Result<Resource<SecretHandleRep>, Self::Error> {
        self.table()
            .push(SecretHandleRep::new(snapshot.clone()))
            .map_err(|e| {
                WorkerExecutorError::runtime(format!("failed to create secret handle: {e}"))
            })
    }

    fn drop_secret_handle(&mut self, handle: Resource<SecretHandleRep>) {
        let _ = self.table().delete(handle);
    }
}

impl<Ctx: WorkerCtx> types::Host for DurableWorkerCtx<Ctx> {
    async fn id(&mut self, _s: Resource<SecretHandleRep>) -> anyhow::Result<SecretId> {
        DurabilityHost::observe_function_call(self, "golem::secrets::types", "id");
        anyhow::bail!("golem:secrets/types.id is not yet implemented")
    }

    async fn metadata(&mut self, _s: Resource<SecretHandleRep>) -> anyhow::Result<SecretMetadata> {
        DurabilityHost::observe_function_call(self, "golem::secrets::types", "metadata");
        anyhow::bail!("golem:secrets/types.metadata is not yet implemented")
    }
}

impl<Ctx: WorkerCtx> reveal::Host for DurableWorkerCtx<Ctx> {
    async fn reveal(
        &mut self,
        _s: Resource<SecretHandleRep>,
        _expected: SchemaGraph,
    ) -> anyhow::Result<Result<SchemaValueTree, types::SecretError>> {
        DurabilityHost::observe_function_call(self, "golem::secrets::reveal", "reveal");
        anyhow::bail!("golem:secrets/reveal.reveal is not yet implemented")
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    #[test]
    fn secret_backed_config_accepts_secret_payload_type_for_secret_declaration_and_secret_handle_operations_are_implemented(
    ) {
        let source = include_str!("secrets.rs");

        assert!(
            !source.contains("golem:secrets/types.id is not yet implemented"),
            "secret-backed config returns opaque handles, so golem:secrets/types.id must work on those handles"
        );
        assert!(
            !source.contains("golem:secrets/types.metadata is not yet implemented"),
            "secret-backed config returns opaque handles, so golem:secrets/types.metadata must work on those handles"
        );
        assert!(
            !source.contains("golem:secrets/reveal.reveal is not yet implemented"),
            "secret-backed config returns opaque handles, so golem:secrets/reveal.reveal must reveal the stored value through the declared inner type"
        );
    }
}
