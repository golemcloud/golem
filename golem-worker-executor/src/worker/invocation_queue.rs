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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ResidentWorkOrder {
    Control,
    After(OplogIndex),
}

#[derive(Debug)]
pub(super) struct ResidentWork {
    pub(super) order: ResidentWorkOrder,
    pub(super) invocation: QueuedWorkerInvocation,
}

impl ResidentWork {
    pub(super) fn control(invocation: QueuedWorkerInvocation) -> Self {
        Self {
            order: ResidentWorkOrder::Control,
            invocation,
        }
    }

    pub(super) fn ordered(position: OplogIndex, invocation: QueuedWorkerInvocation) -> Self {
        Self {
            order: ResidentWorkOrder::After(position),
            invocation,
        }
    }

    fn is_abandoned(&self) -> bool {
        match &self.invocation {
            QueuedWorkerInvocation::ReadFile { sender, .. } => sender.is_closed(),
            QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => sender.is_canceled(),
            QueuedWorkerInvocation::GetWalletCards { sender } => sender.is_canceled(),
            QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => sender.is_canceled(),
            QueuedWorkerInvocation::SaveSnapshot => false,
        }
    }
}

/// Removes disconnected resident requests. Durable invocations are represented only by status
/// references and are therefore never abandoned with their original caller.
pub(super) fn prune_abandoned(queue: &mut VecDeque<ResidentWork>) {
    queue.retain(|item| !item.is_abandoned());
}

/// Returns the earliest resident item that may run at this status. A control may pass blocked
/// ordered requests, but cannot pass an earlier ordered request that is already eligible.
pub(super) fn ready_position(
    queue: &VecDeque<ResidentWork>,
    status: &AgentStatusRecord,
) -> Option<usize> {
    queue.iter().position(|item| match item.order {
        ResidentWorkOrder::Control => true,
        ResidentWorkOrder::After(position) => {
            status.pending_updates.is_empty()
                && !status
                    .pending_invocations
                    .iter()
                    .any(|pending| pending.oplog_index <= position)
        }
    })
}

pub(super) fn resident_precedes_durable(
    resident: &ResidentWork,
    durable: Option<OplogIndex>,
) -> bool {
    match resident.order {
        ResidentWorkOrder::Control => true,
        ResidentWorkOrder::After(position) => durable.is_none_or(|index| index > position),
    }
}

pub(super) fn fail_resident(queue: &mut VecDeque<ResidentWork>, error: &WorkerExecutorError) {
    for item in queue.drain(..) {
        match item.invocation {
            QueuedWorkerInvocation::ReadFile { sender, .. } => {
                let _ = sender.send(Err(FileReadError::Lifecycle));
            }
            QueuedWorkerInvocation::GetFileSystemNode { sender, .. } => {
                let _ = sender.send(Err(error.clone()));
            }
            QueuedWorkerInvocation::GetWalletCards { sender } => {
                let _ = sender.send(Err(error.clone()));
            }
            QueuedWorkerInvocation::AwaitReadyToProcessCommands { sender } => {
                let _ = sender.send(Err(error.clone()));
            }
            QueuedWorkerInvocation::SaveSnapshot => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::CanonicalFilePath;
    use golem_common::model::filesystem::FileByteSelection;
    use golem_common::model::{IdempotencyKey, PendingInvocationRef, Timestamp};
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
        position: u64,
    ) -> (
        ResidentWork,
        tokio::sync::oneshot::Receiver<
            Result<golem_service_base::model::FileReadResponse, FileReadError>,
        >,
    ) {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        (
            ResidentWork::ordered(
                OplogIndex::from_u64(position),
                QueuedWorkerInvocation::ReadFile {
                    path: CanonicalFilePath::from_abs_str("/file").unwrap(),
                    selection: FileByteSelection::Full,
                    sender,
                },
            ),
            receiver,
        )
    }

    #[test]
    fn schedule_a_read_b_c_selects_by_one_shared_position() {
        let (read, _receiver) = read(10);
        let queue = VecDeque::from([read]);
        let durable = status(&[5, 15]);
        let resident = &queue[ready_position(&queue, &status(&[15])).unwrap()];
        assert!(!resident_precedes_durable(
            resident,
            Some(durable.pending_invocations[0].oplog_index)
        ));
        assert!(resident_precedes_durable(
            resident,
            Some(durable.pending_invocations[1].oplog_index)
        ));
    }

    #[test]
    fn control_passes_blocked_ordered_but_not_eligible_ordered() {
        let (read, _receiver) = read(10);
        let queue = VecDeque::from([
            read,
            ResidentWork::control(QueuedWorkerInvocation::SaveSnapshot),
        ]);
        assert_eq!(ready_position(&queue, &status(&[5])), Some(1));
        assert_eq!(ready_position(&queue, &status(&[15])), Some(0));
    }

    #[test]
    fn cancelled_resident_work_is_pruned() {
        let (read, receiver) = read(10);
        drop(receiver);
        let mut queue = VecDeque::from([read]);
        prune_abandoned(&mut queue);
        assert!(queue.is_empty());
    }
}
