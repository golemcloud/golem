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

use std::sync::Mutex;
use tokio_util::task::{TaskTracker, task_tracker::TaskTrackerToken};

/// Registers work before shutdown and joins its leases without holding an admission lock.
#[derive(Debug)]
pub(crate) struct WorkAdmission {
    accepting: Mutex<bool>,
    tracker: TaskTracker,
}

impl Default for WorkAdmission {
    fn default() -> Self {
        Self {
            accepting: Mutex::new(true),
            tracker: TaskTracker::new(),
        }
    }
}

impl WorkAdmission {
    pub(crate) fn enter(&self) -> Option<TaskTrackerToken> {
        let accepting = self.accepting.lock().expect("work admission lock poisoned");
        accepting.then(|| self.tracker.token())
    }

    pub(crate) fn close(&self) {
        let mut accepting = self.accepting.lock().expect("work admission lock poisoned");
        *accepting = false;
        self.tracker.close();
    }

    pub(crate) async fn wait(&self) {
        self.tracker.wait().await;
    }
}

#[cfg(test)]
mod tests;
