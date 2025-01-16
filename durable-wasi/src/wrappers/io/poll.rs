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
use std::collections::HashMap;

pub enum WrappedPollable {
    Proxy(crate::bindings::wasi::io::poll::Pollable),
    Ready,
}

impl crate::bindings::exports::wasi::io::poll::GuestPollable for WrappedPollable {
    fn ready(&self) -> bool {
        observe_function_call("io::poll:pollable", "ready");
        match self {
            Self::Proxy(pollable) => pollable.ready(),
            Self::Ready => true,
        }
    }

    fn block(&self) {
        observe_function_call("io::poll:pollable", "block");
        match self {
            Self::Proxy(pollable) => pollable.block(),
            Self::Ready => {}
        }
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
            let mut mapping = HashMap::new();
            let mut readies = Vec::new();
            let mut pollables = Vec::new();

            for (idx, pollable) in in_.iter().enumerate() {
                match pollable.get::<WrappedPollable>() {
                    WrappedPollable::Proxy(pollable) => {
                        mapping.insert(pollables.len(), idx);
                        pollables.push(pollable);
                    }
                    WrappedPollable::Ready => {
                        readies.push(idx);
                    }
                }
            }

            let inner_result = poll(&pollables);
            let mut result = Vec::new();
            for idx in inner_result {
                result.push(*mapping.get(&(idx as usize)).unwrap() as u32);
            }
            for idx in readies {
                result.push(idx as u32);
            }
            result.sort();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }
}
