// Copyright 2024 Golem Cloud
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

use crate::durable_host::serialized::SerializableError;
use crate::services::rpc::RpcError;
use bincode::{Decode, Encode};
use golem_wasm_rpc::{Value, WitValue};

#[derive(Debug, Clone, Encode, Decode)]
pub enum SerializableInvokeResult {
    Failed(SerializableError),
    Pending,
    Completed(Result<WitValue, RpcError>),
}

impl PartialEq for SerializableInvokeResult {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Failed(e1), Self::Failed(e2)) => e1 == e2,
            (Self::Pending, Self::Pending) => true,
            (Self::Completed(Ok(r1)), Self::Completed(Ok(r2))) => {
                let v1: Value = r1.clone().into();
                let v2: Value = r2.clone().into();
                v1 == v2
            }
            (Self::Completed(Err(e1)), Self::Completed(Err(e2))) => e1 == e2,
            _ => false,
        }
    }
}
