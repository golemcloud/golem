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

use crate::bindings::golem::api::host::PromiseId;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub fn await_promise_json<T: DeserializeOwned>(
    promise_id: &PromiseId,
) -> Result<T, serde_json::Error> {
    let bytes = crate::bindings::golem::api::host::await_promise(promise_id);
    serde_json::from_slice(&bytes)
}

pub fn complete_promise_json<T: Serialize>(
    promise_id: &PromiseId,
    value: T,
) -> Result<bool, serde_json::Error> {
    let bytes = serde_json::to_vec(&value)?;
    Ok(crate::bindings::golem::api::host::complete_promise(
        promise_id, &bytes,
    ))
}
