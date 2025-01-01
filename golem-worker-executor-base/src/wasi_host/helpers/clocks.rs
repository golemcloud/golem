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

use cap_std::time::SystemClock;
use cap_std::{ambient_authority, AmbientAuthority};
use cap_time_ext::SystemClockExt;
use wasmtime_wasi::HostMonotonicClock;

/// Using a SystemClock as a monotonic clock so instants are reusable between persisted executions
pub struct MonotonicClock {
    clock: cap_std::time::SystemClock,
}

impl MonotonicClock {
    pub fn new(ambient_authority: AmbientAuthority) -> Self {
        Self {
            clock: cap_std::time::SystemClock::new(ambient_authority),
        }
    }
}

impl HostMonotonicClock for MonotonicClock {
    fn resolution(&self) -> u64 {
        self.clock.resolution().as_nanos().try_into().unwrap()
    }

    fn now(&self) -> u64 {
        // Unwrap here and in `resolution` above; a `u64` is wide enough to
        // hold over 584 years of nanoseconds.
        self.clock
            .now()
            .duration_since(SystemClock::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .try_into()
            .unwrap()
    }
}

pub fn monotonic_clock() -> impl HostMonotonicClock {
    MonotonicClock::new(ambient_authority())
}
