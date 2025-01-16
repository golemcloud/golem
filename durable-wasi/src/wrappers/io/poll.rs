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

use crate::bindings::exports::wasi::io::poll::PollableBorrow;
use crate::bindings::golem::durability::durability::{observe_function_call, DurableFunctionType};
use crate::bindings::wasi::io::poll::poll;
use crate::durability::Durability;
use crate::wrappers::SerializableError;

pub struct WrappedPollable {
    pub pollable: crate::bindings::wasi::io::poll::Pollable,
}

impl crate::bindings::exports::wasi::io::poll::GuestPollable for WrappedPollable {
    fn ready(&self) -> bool {
        observe_function_call("io::poll:pollable", "ready");
        self.pollable.ready()
    }

    fn block(&self) {
        observe_function_call("io::poll:pollable", "block");
        self.pollable.block()
    }
}

impl Drop for WrappedPollable {
    fn drop(&mut self) {
        observe_function_call("io::poll:pollable", "drop");
    }
}

impl crate::bindings::exports::wasi::io::poll::Guest for crate::Component {
    type Pollable = WrappedPollable;

    fn poll(in_: Vec<PollableBorrow<'_>>) -> Vec<u32> {
        let durability = Durability::<Vec<u32>, SerializableError>::new(
            "golem io::poll",
            "poll",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let pollables = in_
                .iter()
                .map(|pollable| &pollable.get::<WrappedPollable>().pollable)
                .collect::<Vec<_>>();
            let result = poll(&pollables);
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
