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

//! This module is test code, and it compiles only for tests.
//!
//! A clock that a test moves, and that can hold one read until the test lets it go.

use crate::filesystem_snapshot::clock::Clock;
use golem_common::model::Timestamp;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// The wall clock of the host, moved by an offset that the test sets. A read can wait at a hold
/// that the test arms.
#[derive(Debug, Default)]
pub(in crate::filesystem_snapshot) struct TestClock {
    /// The milliseconds that this clock is ahead of the clock of the host, or behind it when the
    /// number is negative.
    ahead: AtomicI64,
    hold: Mutex<Hold>,
    changed: Condvar,
}

#[derive(Debug, Default, PartialEq, Eq)]
enum Hold {
    /// Each read gives the time at once.
    #[default]
    Off,
    /// The next read waits until the test lets it go.
    Armed,
    /// A read waits.
    Reached,
    /// The read that waited may go on.
    Released,
}

impl TestClock {
    /// Moves the clock forward by the duration.
    pub(in crate::filesystem_snapshot) fn advance(&self, by: Duration) {
        self.ahead.fetch_add(millis(by), Ordering::SeqCst);
    }

    /// Sets the clock to the duration ahead of the clock of the host.
    pub(in crate::filesystem_snapshot) fn set_ahead(&self, by: Duration) {
        self.ahead.store(millis(by), Ordering::SeqCst);
    }

    /// Makes the next read wait until [`TestClock::release`].
    pub(in crate::filesystem_snapshot) fn hold_next_read(&self) {
        *self.state() = Hold::Armed;
    }

    /// Tells whether a read waits at the hold, and waits for that for at most the limit.
    pub(in crate::filesystem_snapshot) fn wait_until_held(&self, limit: Duration) -> bool {
        let (state, _) = self
            .changed
            .wait_timeout_while(self.state(), limit, |state| *state != Hold::Reached)
            .unwrap_or_else(PoisonError::into_inner);
        *state == Hold::Reached
    }

    /// Lets the read that waits at the hold go on.
    pub(in crate::filesystem_snapshot) fn release(&self) {
        *self.state() = Hold::Released;
        self.changed.notify_all();
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Hold> {
        self.hold.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        let mut state = self.state();
        if *state == Hold::Armed {
            *state = Hold::Reached;
            self.changed.notify_all();
            state = self
                .changed
                .wait_while(state, |state| *state == Hold::Reached)
                .unwrap_or_else(PoisonError::into_inner);
            *state = Hold::Off;
        }
        drop(state);
        let now = i64::try_from(Timestamp::now_utc().to_millis()).unwrap_or(i64::MAX);
        Timestamp::from(u64::try_from(now + self.ahead.load(Ordering::SeqCst)).unwrap_or(0))
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
