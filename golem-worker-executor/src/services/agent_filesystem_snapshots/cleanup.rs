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

//! The clean-up queue of `delete_snapshots` and `delete_all_snapshots`. Nothing on it can stop the
//! executor: each job catches its own error, and a failure after the retries is logged and counted.

use super::registry::DeleteAllTicket;
use super::{job::retrying, store_name};
use crate::filesystem_snapshot::{AgentSnapshots, FilesystemSnapshotStore};
use futures::StreamExt as _;
use golem_common::model::RetryConfig;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// One clean-up.
enum Cleanup {
    /// Deletes some snapshots of an agent.
    Delete {
        agent: AgentSnapshots,
        names: Box<[FilesystemSnapshotName]>,
    },
    /// Deletes all snapshots of an agent, after the job of the agent ended. The ticket goes away
    /// with the clean-up, on each exit.
    DeleteAll {
        agent: AgentSnapshots,
        ticket: DeleteAllTicket,
    },
}

/// The sender side of the queue. A send never blocks and never fails the caller.
pub(super) struct CleanupQueue {
    sender: UnboundedSender<Cleanup>,
}

/// What a clean-up needs.
struct Cleaner {
    store: Arc<dyn FilesystemSnapshotStore>,
    /// The slots of the store operations that save or delete. Each delete holds one.
    uploads: Arc<Semaphore>,
    retry: RetryConfig,
}

impl CleanupQueue {
    /// Starts the task that runs the clean-ups, tracked by `jobs`. The task ends at `shutdown`.
    pub(super) fn start(
        store: Arc<dyn FilesystemSnapshotStore>,
        uploads: Arc<Semaphore>,
        retry: RetryConfig,
        shutdown: CancellationToken,
        jobs: &TaskTracker,
    ) -> Self {
        let (sender, receiver) = unbounded_channel();
        let cleaner = Arc::new(Cleaner {
            store,
            uploads,
            retry,
        });
        jobs.spawn(async move {
            let cleanups =
                UnboundedReceiverStream::new(receiver).for_each_concurrent(None, |cleanup| {
                    let cleaner = Arc::clone(&cleaner);
                    async move { cleaner.run(cleanup).await }
                });
            tokio::select! {
                () = cleanups => {}
                () = shutdown.cancelled() => {}
            }
        });
        Self { sender }
    }

    pub(super) fn delete(&self, agent: AgentSnapshots, names: Box<[FilesystemSnapshotName]>) {
        self.send(Cleanup::Delete { agent, names }, "delete");
    }

    pub(super) fn delete_all(&self, agent: AgentSnapshots, ticket: DeleteAllTicket) {
        self.send(Cleanup::DeleteAll { agent, ticket }, "delete_all");
    }

    fn send(&self, cleanup: Cleanup, operation: &'static str) {
        if self.sender.send(cleanup).is_err() {
            tracing::warn!(
                operation,
                "The clean-up queue of filesystem snapshots has stopped; the clean-up is lost"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup(operation);
        }
    }
}

impl Cleaner {
    async fn run(&self, cleanup: Cleanup) {
        match cleanup {
            Cleanup::Delete { agent, names } => self.delete(&agent, &names).await,
            Cleanup::DeleteAll { agent, ticket } => {
                ticket.until_agent_free().await;
                let deleted = self
                    .with_slot(|| retrying(&self.retry, || self.store.delete_all(&agent)))
                    .await;
                if let Err(error) = deleted {
                    tracing::warn!(
                        error = %error,
                        "Failed to delete the filesystem snapshots of a deleted agent after the retries"
                    );
                    crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete_all");
                }
            }
        }
    }

    /// Deletes the snapshots `names` of `agent` as one batch, under one slot and its retries.
    async fn delete(&self, agent: &AgentSnapshots, names: &[FilesystemSnapshotName]) {
        let store_names = names
            .iter()
            .filter_map(|name| store_name(name).ok())
            .collect::<Box<[_]>>();
        if store_names.is_empty() {
            return;
        }
        let deleted = self
            .with_slot(|| retrying(&self.retry, || self.store.delete(agent, &store_names)))
            .await;
        if let Err(error) = deleted {
            tracing::warn!(
                error = %error,
                names = ?store_names,
                "Failed to delete filesystem snapshots after the retries"
            );
            crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete");
        }
    }

    /// Runs `operation` while it holds a slot of the store operations that save or delete.
    async fn with_slot<T, Operation>(
        &self,
        operation: impl FnOnce() -> Operation,
    ) -> Result<T, crate::filesystem_snapshot::SnapshotStoreError>
    where
        Operation:
            std::future::Future<Output = Result<T, crate::filesystem_snapshot::SnapshotStoreError>>,
    {
        let _slot = Arc::clone(&self.uploads)
            .acquire_owned()
            .await
            .map_err(
                |error| crate::filesystem_snapshot::SnapshotStoreError::Storage {
                    retryable: false,
                    source: anyhow::Error::new(error),
                },
            )?;
        operation().await
    }
}
