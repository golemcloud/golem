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

use super::QueuedWorkerInvocation;
use golem_common::model::filesystem::FileReadError;
use golem_common::model::{AgentStatusRecord, OplogIndex};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// The accepted durable work preceding an inspection. Oplog indices are monotonic even on
/// revert. Capture this from attached status while holding the worker instance mutex, which
/// also serializes durable invocation acceptance and inspection dequeue.
#[derive(Debug)]
pub struct InspectionOrder {
    id: Uuid,
    pending_through: Option<OplogIndex>,
}

impl InspectionOrder {
    pub(super) fn new(status: &AgentStatusRecord) -> Self {
        Self {
            id: Uuid::new_v4(),
            pending_through: status
                .pending_invocations
                .iter()
                .map(|p| p.oplog_index)
                .max(),
        }
    }

    fn ready(&self, status: &AgentStatusRecord) -> bool {
        status.pending_updates.is_empty()
            && !status.pending_invocations.iter().any(|pending| {
                self.pending_through
                    .is_some_and(|cutoff| pending.oplog_index <= cutoff)
            })
    }
}

/// Removing a cancelled request never waits for the invocation loop (which may be streaming
/// another read). The queue mutex only protects synchronous, bounded bookkeeping.
pub(super) struct QueuedInspectionGuard {
    queue: Arc<Mutex<VecDeque<QueuedWorkerInvocation>>>,
    id: Uuid,
}

impl QueuedInspectionGuard {
    pub(super) fn new(
        queue: Arc<Mutex<VecDeque<QueuedWorkerInvocation>>>,
        order: &InspectionOrder,
    ) -> Self {
        Self {
            queue,
            id: order.id,
        }
    }
}

impl Drop for QueuedInspectionGuard {
    fn drop(&mut self) {
        self.queue.lock().unwrap().retain(|item| {
            item.inspection_order()
                .is_none_or(|order| order.id != self.id)
        });
    }
}

impl QueuedWorkerInvocation {
    pub(super) fn inspection_order(&self) -> Option<&InspectionOrder> {
        match self {
            Self::ReadFile { order, .. } | Self::GetFileSystemNode { order, .. } => Some(order),
            _ => None,
        }
    }
}

pub(super) fn pop_ready(
    queue: &mut VecDeque<QueuedWorkerInvocation>,
    status: &AgentStatusRecord,
) -> Option<QueuedWorkerInvocation> {
    // Internal control work can pass a read waiting for initialization or older invocations.
    let position = queue.iter().position(|item| {
        item.inspection_order()
            .is_none_or(|order| order.ready(status))
    })?;
    queue.remove(position)
}

pub(super) fn fail_inspections(
    queue: &mut VecDeque<QueuedWorkerInvocation>,
    error: &WorkerExecutorError,
) {
    let items = queue.drain(..).collect::<Vec<_>>();
    for item in items {
        match item {
            QueuedWorkerInvocation::ReadFile { sender, .. } => {
                let _ = sender.send(Err(FileReadError::Lifecycle));
            }
            QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => {
                let _ = sender.send(Err(error.clone()));
            }
            other => queue.push_back(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::file_read_admission::FileReadAdmission;
    use futures::channel::oneshot;
    use golem_common::model::component::{CanonicalFilePath, ComponentRevision};
    use golem_common::model::filesystem::{FileByteSelection, FileReadTarget};
    use golem_common::model::{IdempotencyKey, PendingInvocationRef, Timestamp};
    use golem_service_base::model::FileReadResponse;
    use test_r::test;

    fn status(indices: &[u64]) -> AgentStatusRecord {
        AgentStatusRecord {
            pending_invocations: indices
                .iter()
                .map(|index| PendingInvocationRef {
                    timestamp: Timestamp::now_utc(),
                    oplog_index: OplogIndex::from_u64(*index),
                    idempotency_key: Some(IdempotencyKey::fresh()),
                    manual_update_target_revision: None,
                })
                .collect(),
            ..Default::default()
        }
    }

    fn read(
        status: &AgentStatusRecord,
    ) -> (
        QueuedWorkerInvocation,
        tokio::sync::oneshot::Receiver<Result<FileReadResponse, FileReadError>>,
    ) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let agent = golem_common::model::OwnedAgentId::new(
            golem_common::model::environment::EnvironmentId::new(),
            &golem_common::model::AgentId::from_agent_name_string(
                golem_common::model::component::ComponentId::new(),
                "test",
            )
            .unwrap(),
        );
        let reservation = Arc::new(FileReadAdmission::default())
            .reserve(agent, tokio::time::Instant::now())
            .unwrap();
        (
            QueuedWorkerInvocation::ReadFile {
                target: FileReadTarget::Exact {
                    file_path: "/file".into(),
                },
                selection: FileByteSelection::Full,
                reservation,
                order: InspectionOrder::new(status),
                sender,
            },
            receiver,
        )
    }

    #[test]
    fn cutoff_waits_for_all_older_work_but_not_later_work() {
        let accepted = status(&[3, 11]);
        let (read, _receiver) = read(&accepted);
        let mut queue = VecDeque::from([read, QueuedWorkerInvocation::SaveSnapshot]);
        assert!(matches!(
            pop_ready(&mut queue, &accepted),
            Some(QueuedWorkerInvocation::SaveSnapshot)
        ));
        assert!(pop_ready(&mut queue, &status(&[3, 19])).is_none());
        assert!(pop_ready(&mut queue, &status(&[11, 19])).is_none());
        assert!(matches!(
            pop_ready(&mut queue, &status(&[19])),
            Some(QueuedWorkerInvocation::ReadFile { .. })
        ));
    }

    #[test]
    fn initialization_and_manual_update_are_both_older_work() {
        let mut accepted = status(&[2, 7]);
        accepted.pending_invocations[1].idempotency_key = None;
        accepted.pending_invocations[1].manual_update_target_revision =
            Some(ComponentRevision::INITIAL);
        let order = InspectionOrder::new(&accepted);
        assert!(!order.ready(&accepted));
        accepted.pending_invocations.remove(0);
        assert!(!order.ready(&accepted));
        accepted.pending_invocations.clear();
        assert!(order.ready(&accepted));
        assert!(InspectionOrder::new(&accepted).ready(&status(&[31])));
    }

    #[test]
    fn pending_update_blocks_reads_until_the_new_generation_is_ready() {
        use golem_common::model::{PendingUpdateKind, PendingUpdateRef};

        let mut current = status(&[]);
        let order = InspectionOrder::new(&current);
        for kind in [
            PendingUpdateKind::Automatic,
            PendingUpdateKind::SnapshotBased,
        ] {
            current.pending_updates.push_back(PendingUpdateRef {
                timestamp: Timestamp::now_utc(),
                oplog_index: OplogIndex::from_u64(23),
                target_revision: ComponentRevision::INITIAL,
                kind,
            });
            assert!(!order.ready(&current));
            current.pending_updates.clear();
            assert!(order.ready(&current));
        }
    }

    #[test]
    fn listing_uses_the_same_cutoff_as_reads() {
        let accepted = status(&[8]);
        let (sender, _receiver) = oneshot::channel();
        let mut queue = VecDeque::from([QueuedWorkerInvocation::GetFileSystemNode {
            path: CanonicalFilePath::from_abs_str("/").unwrap(),
            order: InspectionOrder::new(&accepted),
            sender,
        }]);
        assert!(pop_ready(&mut queue, &accepted).is_none());
        assert!(matches!(
            pop_ready(&mut queue, &status(&[])),
            Some(QueuedWorkerInvocation::GetFileSystemNode { .. })
        ));
    }

    #[test]
    fn cancellation_removes_only_its_request_without_loop_progress() {
        let (first, mut receiver) = read(&status(&[2]));
        let (second, _second_receiver) = read(&status(&[2]));
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let guard = QueuedInspectionGuard::new(queue.clone(), first.inspection_order().unwrap());
        queue
            .lock()
            .unwrap()
            .extend([first, second, QueuedWorkerInvocation::SaveSnapshot]);
        drop(guard);
        assert!(receiver.try_recv().is_err());
        assert_eq!(queue.lock().unwrap().len(), 2);
        assert!(matches!(
            queue.lock().unwrap().front(),
            Some(QueuedWorkerInvocation::ReadFile { .. })
        ));
    }

    #[test]
    fn failure_drains_read_and_listing_but_preserves_control_work() {
        let (read, mut read_receiver) = read(&status(&[2]));
        let (sender, mut list_receiver) = oneshot::channel();
        let mut queue = VecDeque::from([
            read,
            QueuedWorkerInvocation::GetFileSystemNode {
                path: CanonicalFilePath::from_abs_str("/").unwrap(),
                order: InspectionOrder::new(&status(&[2])),
                sender,
            },
            QueuedWorkerInvocation::SaveSnapshot,
        ]);
        fail_inspections(&mut queue, &WorkerExecutorError::PreviousInvocationExited);
        assert!(matches!(
            read_receiver.try_recv(),
            Ok(Err(FileReadError::Lifecycle))
        ));
        assert!(matches!(
            list_receiver.try_recv(),
            Ok(Some(Err(WorkerExecutorError::PreviousInvocationExited)))
        ));
        assert!(matches!(
            queue.pop_front(),
            Some(QueuedWorkerInvocation::SaveSnapshot)
        ));
        assert!(queue.is_empty());
    }
}
