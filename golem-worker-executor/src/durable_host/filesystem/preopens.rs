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
use crate::durable_host::authorization::targets::CanonicalGuestPath;
use crate::durable_host::filesystem::remember_path;
use crate::workerctx::WorkerCtx;
use wasmtime_wasi::filesystem::WasiFilesystemView as _;
use wasmtime_wasi::p2::bindings::filesystem::preopens::{Descriptor, Host};

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        let mut view = self.as_wasi_view();
        let current_dirs = Host::get_directories(&mut view.filesystem()).await?;
        for (descriptor, guest_path) in &current_dirs {
            let path = CanonicalGuestPath::new(if guest_path == "." { "/" } else { guest_path })
                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
            remember_path(self, descriptor.rep(), path);
        }
        Ok(current_dirs)
    }
}
