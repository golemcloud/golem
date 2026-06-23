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

use std::sync::{Arc, RwLock};

use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::wasi::keyvalue::types::{
    Error, Host, HostBucket, HostIncomingValue, HostOutgoingValue, IncomingValue,
};
use crate::workerctx::WorkerCtx;
use wasmtime::component::{Resource, StreamReader};
use wasmtime_wasi::IoView;

impl<Ctx: WorkerCtx> HostBucket for DurableWorkerCtx<Ctx> {
    async fn open_bucket(
        &mut self,
        name: String,
    ) -> anyhow::Result<Result<Resource<BucketEntry>, Resource<Error>>> {
        self.observe_function_call("keyvalue::types::bucket", "open");
        let bucket = self.as_wasi_view().table().push(BucketEntry::new(name))?;
        Ok(Ok(bucket))
    }

    async fn drop(&mut self, rep: Resource<BucketEntry>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::types::bucket", "drop");
        self.as_wasi_view().table().delete::<BucketEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostOutgoingValue for DurableWorkerCtx<Ctx> {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueEntry>> {
        self.observe_function_call("keyvalue::types::outgoing_value", "new_outgoing_value");
        let outgoing_value = self
            .as_wasi_view()
            .table()
            .push(OutgoingValueEntry::new())?;
        Ok(outgoing_value)
    }

    async fn outgoing_value_write_body_async(
        &mut self,
        _self_: Resource<OutgoingValueEntry>,
    ) -> anyhow::Result<Result<StreamReader<u8>, Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_async",
        );
        unimplemented!("outgoing_value_write_body_async is not yet implemented for WASI p3 streams")
    }

    async fn outgoing_value_write_body_sync(
        &mut self,
        self_: Resource<OutgoingValueEntry>,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::outgoing_value",
            "outgoing_value_write_body_sync",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<OutgoingValueEntry>(&self_)?
            .body
            .clone();
        body.write().unwrap().extend_from_slice(&value);
        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueEntry>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::types::outgoing_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<OutgoingValueEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostIncomingValue for DurableWorkerCtx<Ctx> {
    async fn incoming_value_consume_sync(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<Vec<u8>, Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::incoming_value",
            "incoming_value_consume_sync",
        );
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let value = body.write().unwrap().drain(..).collect();
        Ok(Ok(value))
    }

    async fn incoming_value_consume_async(
        &mut self,
        _self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<StreamReader<u8>, Resource<Error>>> {
        self.observe_function_call(
            "keyvalue::types::incoming_value",
            "incoming_value_consume_async",
        );
        unimplemented!("incoming_value_consume_async is not yet implemented for WASI p3 streams")
    }

    async fn incoming_value_size(
        &mut self,
        self_: Resource<IncomingValue>,
    ) -> anyhow::Result<Result<u64, Resource<Error>>> {
        self.observe_function_call("keyvalue::types::incoming_value", "size");
        let body = self
            .as_wasi_view()
            .table()
            .get::<IncomingValueEntry>(&self_)?
            .body
            .clone();
        let size = body.read().unwrap().len() as u64;
        Ok(Ok(size))
    }

    async fn drop(&mut self, rep: Resource<IncomingValue>) -> anyhow::Result<()> {
        self.observe_function_call("keyvalue::types::incoming_value", "drop");
        self.as_wasi_view()
            .table()
            .delete::<IncomingValueEntry>(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

pub struct BucketEntry {
    pub name: String,
}

impl BucketEntry {
    pub fn new(name: String) -> Self {
        Self { name }
    }
}

pub struct OutgoingValueEntry {
    pub body: Arc<RwLock<Vec<u8>>>,
}

impl Default for OutgoingValueEntry {
    fn default() -> Self {
        Self::new()
    }
}

impl OutgoingValueEntry {
    pub fn new() -> Self {
        Self {
            body: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

pub struct IncomingValueEntry {
    body: Arc<RwLock<Vec<u8>>>,
}

impl IncomingValueEntry {
    pub fn new(body: Vec<u8>) -> IncomingValueEntry {
        IncomingValueEntry {
            body: Arc::new(RwLock::new(body)),
        }
    }
}
