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

use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::random::insecure::{get_insecure_random_bytes, get_insecure_random_u64};
use crate::durability::Durability;
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::random::insecure::Guest for crate::Component {
    fn get_insecure_random_bytes(len: u64) -> Vec<u8> {
        let durability = Durability::<Vec<u8>, SerializableError>::new(
            "golem random::insecure",
            "get_insecure_random_bytes",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = get_insecure_random_bytes(len);
            durability.persist_infallible(len, result)
        } else {
            durability.replay_infallible()
        }
    }

    fn get_insecure_random_u64() -> u64 {
        let durability = Durability::<u64, SerializableError>::new(
            "golem random::insecure",
            "get_insecure_random_u64",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = get_insecure_random_u64();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
