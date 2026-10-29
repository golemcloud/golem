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

use std::fmt::{Display, Formatter};

#[cfg(test)]
tokio::task_local! {
    static RECORDED_TASKS: std::cell::RefCell<Vec<BlockingTask>>;
}

#[cfg(test)]
pub(crate) async fn record_tasks<F: Future>(future: F) -> (F::Output, Vec<BlockingTask>) {
    RECORDED_TASKS
        .scope(std::cell::RefCell::default(), async {
            let result = future.await;
            let modes = RECORDED_TASKS.with(|tasks| tasks.take());
            (result, modes)
        })
        .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BlockingTask {
    /// Runs on the caller's thread when the runtime supports blocking in place.
    InPlace,
    /// Runs on the runtime's blocking pool, even on a multi-thread runtime.
    Spawn,
}

impl BlockingTask {
    /// Catches task panics in either mode. In-place work falls back to the blocking pool on a
    /// current-thread runtime. Spawned work continues after the caller drops this future.
    pub(crate) async fn run<F, R>(self, task: F) -> Result<R, BlockingTaskError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(test)]
        let _ = RECORDED_TASKS.try_with(|tasks| tasks.borrow_mut().push(self));
        let task = move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
                .map_err(|_| BlockingTaskError::Panicked)
        };
        let can_block_in_place = tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        match self {
            Self::InPlace if can_block_in_place => tokio::task::block_in_place(task),
            Self::InPlace | Self::Spawn => tokio::task::spawn_blocking(task)
                .await
                .map_err(BlockingTaskError::Join)?,
        }
    }
}

#[derive(Debug)]
pub(crate) enum BlockingTaskError {
    Panicked,
    Join(tokio::task::JoinError),
}

impl Display for BlockingTaskError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Panicked => formatter.write_str("blocking task panicked"),
            Self::Join(error) => write!(formatter, "blocking task failed: {error}"),
        }
    }
}

impl std::error::Error for BlockingTaskError {}

#[cfg(test)]
mod tests;
