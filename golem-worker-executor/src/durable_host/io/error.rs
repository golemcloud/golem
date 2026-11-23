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
use crate::workerctx::WorkerCtx;
use wasmtime::component::Resource;
use wasmtime_wasi::p2::bindings::io::error::{Error, Host, HostError};

impl<Ctx: WorkerCtx> HostError for DurableWorkerCtx<Ctx> {
    fn to_debug_string(&mut self, self_: Resource<Error>) -> wasmtime::Result<String> {
        self.observe_function_call("io::error", "to_debug_string");
        let error = self.table().get(&self_)?;
        Ok(debug_string_without_backtrace(error))
    }

    fn drop(&mut self, rep: Resource<Error>) -> wasmtime::Result<()> {
        self.observe_function_call("io::error", "drop");
        HostError::drop(self.table(), rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

/// The `Debug` form of an error without its stack backtrace. A backtrace depends on the host
/// that captured it, so a guest that sees it would see different strings live and on replay.
fn debug_string_without_backtrace(error: &wasmtime::Error) -> String {
    let mut result = error.to_string();
    let causes: Vec<String> = error
        .chain()
        .skip(1)
        .map(|cause| cause.to_string())
        .collect();
    if !causes.is_empty() {
        result.push_str("\n\nCaused by:\n");
        for (i, cause) in causes.iter().enumerate() {
            if causes.len() > 1 {
                result.push_str(&format!("{i: >5}: {cause}\n"));
            } else {
                result.push_str(&format!("    {cause}\n"));
            }
        }
    }
    result
}
