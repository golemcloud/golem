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

//! Wall time for stored timestamps and monotonic time for store-call deadlines.

use golem_common::model::Timestamp;

/// The runtime's monotonic clock, which a test can pause and advance.
pub(super) fn monotonic_now() -> std::time::Instant {
    tokio::time::Instant::now().into_std()
}

/// Gives the wall time.
pub(super) trait Clock: Send + Sync {
    /// Gives the time now.
    fn now(&self) -> Timestamp;
}

/// The clock of the host.
pub(super) struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now_utc()
    }
}
