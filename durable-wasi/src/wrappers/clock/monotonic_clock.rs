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

use crate::bindings::golem::api::durability::DurableFunctionType;
use crate::bindings::wasi::clocks::monotonic_clock::{
    now, resolution, subscribe_duration, subscribe_instant, Duration, Instant,
};
use crate::bindings::wasi::io::poll::Pollable;
use crate::durability::Durability;
use crate::wrappers::SerializableError;

impl crate::bindings::exports::wasi::clocks::monotonic_clock::Guest for crate::Component {
    fn now() -> Instant {
        let durability = Durability::<Instant, SerializableError>::new(
            "monotonic_clock",
            "now",
            DurableFunctionType::ReadLocal,
        );

        if durability.is_live() {
            let result = now();
            durability.persist_infallible((), result)
        } else {
            durability.replay_infallible()
        }
    }

    fn resolution() -> Duration {
        resolution()
    }

    fn subscribe_instant(when: Instant) -> Pollable {
        subscribe_instant(when)
    }

    fn subscribe_duration(when: Duration) -> Pollable {
        subscribe_duration(when)
    }
}
