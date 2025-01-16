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
use crate::bindings::wasi::random::insecure_seed::insecure_seed;
use crate::durability::Durability;
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::random::insecure_seed::Guest for crate::Component {
    fn insecure_seed() -> (u64, u64) {
        let durability = Durability::<(u64, u64), SerializableError>::new(
            "golem random::insecure_seed",
            "insecure_seed",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = insecure_seed();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
