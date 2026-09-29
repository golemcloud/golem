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

//! The clean-up queue of `forget` and `forget_scope`. Nothing on it can stop the executor: each
//! job catches its own error, and a failure after the retries is logged and counted.

use super::registry::DeleteTicket;
use super::{job::retrying, store_name};
use crate::filesystem_snapshot::{FilesystemSnapshotStore, SnapshotScope};
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
    /// Deletes the names of a scope.
    Delete {
        scope: SnapshotScope,
        names: Box<[FilesystemSnapshotName]>,
    },
    /// Deletes a scope, after the job of the scope ended. The ticket goes away with the
    /// clean-up, on each exit.
    DeleteScope {
        scope: SnapshotScope,
        ticket: DeleteTicket,
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

    pub(super) fn delete(&self, scope: SnapshotScope, names: Box<[FilesystemSnapshotName]>) {
        self.send(Cleanup::Delete { scope, names }, "delete");
    }

    pub(super) fn delete_scope(&self, scope: SnapshotScope, ticket: DeleteTicket) {
        self.send(Cleanup::DeleteScope { scope, ticket }, "delete_scope");
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
            Cleanup::Delete { scope, names } => {
                futures::stream::iter(names.iter())
                    .for_each(|name| self.delete(&scope, name))
                    .await
            }
            Cleanup::DeleteScope { scope, ticket } => {
                ticket.jobs_ended().await;
                let deleted = self
                    .with_slot(|| retrying(&self.retry, || self.store.delete_scope(&scope)))
                    .await;
                if let Err(error) = deleted {
                    tracing::warn!(
                        error = %error,
                        "Failed to delete the filesystem snapshots of a deleted agent after the retries"
                    );
                    crate::metrics::filesystem_snapshots::record_leaked_cleanup("delete_scope");
                }
            }
        }
    }

    async fn delete(&self, scope: &SnapshotScope, name: &FilesystemSnapshotName) {
        let Ok(store_name) = store_name(name) else {
            return;
        };
        let deleted = self
            .with_slot(|| retrying(&self.retry, || self.store.delete(scope, &store_name)))
            .await;
        if let Err(error) = deleted {
            tracing::warn!(
                error = %error,
                name = %name,
                "Failed to delete a filesystem snapshot after the retries"
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
