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

use wasmtime::component::Resource;

use crate::durable_host::DurableWorkerCtx;
use crate::wasi_filesystem::push_agent_descriptor;
use crate::workerctx::WorkerCtx;
use golem_common::model::entity::FilesystemCapability;
use wasmtime_wasi::p2::bindings::filesystem::preopens::{Descriptor, Host};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        if self.filesystem_capability() == FilesystemCapability::Incapable {
            return Ok(Vec::new());
        }
        let preopen = self.filesystem_preopen();
        Ok(vec![
            (
                push_agent_descriptor(self, preopen.clone())?,
                "/".to_string(),
            ),
            (push_agent_descriptor(self, preopen)?, ".".to_string()),
        ])
    }
}
