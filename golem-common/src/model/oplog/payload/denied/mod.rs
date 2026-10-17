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

//! Denied responses of the host functions that the host can deny before they run. Each function
//! builds the response of a denied call from its request and the denial recorded in its `Start`
//! (see [`super::DeniablePair`]).

pub mod blobstore;
pub mod filesystem;
pub mod golem_api;
pub mod http;
pub mod keyvalue;
pub mod rdbms;
pub mod rpc;
pub mod sockets;

/// Defines denied-response functions for responses of the form `{ result: Result<_, String> }`,
/// whose error is the denial.
macro_rules! denied_with_message {
    ($($name:ident => $response:ident),* $(,)?) => {
        $(
            pub fn $name<R>(_request: &R, denial: &str) -> $crate::model::oplog::payload::$response {
                $crate::model::oplog::payload::$response {
                    result: Err(denial.to_string()),
                }
            }
        )*
    };
}

pub(crate) use denied_with_message;
