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

use crate::bindings::exports::wasi::clocks::wall_clock::Datetime;
use crate::bindings::golem::durability::durability::DurableFunctionType;
use crate::bindings::wasi::clocks::wall_clock::{now, resolution};
use crate::durability::Durability;
use crate::wrappers::{SerializableDateTime, SerializableError};
use std::mem::transmute;

impl crate::bindings::exports::wasi::clocks::wall_clock::Guest for crate::Component {
    fn now() -> Datetime {
        let durability = Durability::<SerializableDateTime, SerializableError>::new(
            "wall_clock",
            "now",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = unsafe { transmute(now()) };
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }

    fn resolution() -> Datetime {
        let durability = Durability::<SerializableDateTime, SerializableError>::new(
            "wall_clock",
            "resolution",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = unsafe { transmute(resolution()) };
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
