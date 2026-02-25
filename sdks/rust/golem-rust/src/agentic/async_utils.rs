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

use crate::golem_agentic::golem::agent::common::DataValue;
use crate::golem_agentic::golem::agent::host::{FutureInvokeResult, RpcError};
use wstd::wasi::io::poll::Pollable;

pub async fn await_invoke_result(invoke_result: FutureInvokeResult) -> Result<DataValue, RpcError> {
    loop {
        let pollable = invoke_result.subscribe();
        await_pollable(pollable).await;

        if let Some(result) = invoke_result.get() {
            return result;
        }
    }
}

pub async fn await_pollable(pollable: Pollable) {
    wstd::runtime::AsyncPollable::new(pollable).wait_for().await;
}
