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

use golem_wasm_rpc::ValueAndType;
use std::mem;

include!(concat!(env!("OUT_DIR"), "/preview2_mod.rs"));

pub type InputStream = wasmtime_wasi::InputStream;
pub type OutputStream = wasmtime_wasi::OutputStream;

pub type Pollable = wasmtime_wasi::Pollable;

impl From<golem_wasm_rpc::WitValue> for golem::rpc::types::WitValue {
    fn from(value: golem_wasm_rpc::WitValue) -> Self {
        unsafe { mem::transmute(value) }
    }
}

impl From<golem_wasm_rpc::Value> for golem::rpc::types::WitValue {
    fn from(value: golem_wasm_rpc::Value) -> Self {
        let wit_value: golem_wasm_rpc::WitValue = value.into();
        wit_value.into()
    }
}

impl From<ValueAndType> for golem::rpc::types::WitValue {
    fn from(value: ValueAndType) -> Self {
        let wit_value: golem_wasm_rpc::WitValue = value.into();
        wit_value.into()
    }
}
