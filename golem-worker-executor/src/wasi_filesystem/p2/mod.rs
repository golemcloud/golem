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

mod preopens;
pub(crate) mod types;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;
use wasmtime::component::{HasSelf, Linker};

/// Registers the WASI P2 filesystem preopens and descriptor host interfaces.
/// Both interfaces use `DurableWorkerCtx` and route descriptor state through the shared agent-filesystem table.
pub(crate) fn add_to_linker<Ctx: WorkerCtx + Send + Sync>(
    linker: &mut Linker<Ctx>,
    get: fn(&mut Ctx) -> &mut DurableWorkerCtx<Ctx>,
) -> wasmtime::Result<()> {
    wasmtime_wasi::p2::bindings::filesystem::preopens::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(linker, get)?;
    wasmtime_wasi::p2::bindings::filesystem::types::add_to_linker::<
        _,
        HasSelf<DurableWorkerCtx<Ctx>>,
    >(linker, get)?;
    Ok(())
}
