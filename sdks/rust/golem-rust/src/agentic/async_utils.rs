// Copyright 2024-2025 Golem Cloud
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

use crate::wasm_rpc::golem_rpc_0_2_x::types::Pollable;
use golem_wasm::{FutureInvokeResult, RpcError, WitValue};

pub async fn await_invoke_result(invoke_result: FutureInvokeResult) -> Result<WitValue, RpcError> {
    let golem_wasm_pollable = invoke_result.subscribe();

    await_pollable(golem_wasm_pollable).await;

    invoke_result.get().expect("RPC invoke failed")
}

pub async fn await_pollable(pollable: Pollable) {
    let pollable_wasi: wasi::io::poll::Pollable = unsafe { std::mem::transmute(pollable) };

    wstd::runtime::AsyncPollable::new(pollable_wasi)
        .wait_for()
        .await;
}
