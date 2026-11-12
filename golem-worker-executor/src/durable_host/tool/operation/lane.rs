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

use super::{
    AcquisitionControl, AcquisitionDrain, AcquisitionDrainGuard, Arc, BodyAdmissionState,
    LaneOwnership, OplogIndex, Ordering, OwnerInvocationId, OwnerLane, OwnerLaneWait,
    OwnerToolOperation, OwnerToolOperations, ToolOperationWinner, WorkerExecutorError,
};

impl AcquisitionDrain {
    pub(super) fn finish(&self) {
        self.done.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub(super) async fn wait(&self) {
        while !self.done.load(Ordering::Acquire) {
            let notified = self.notify.notified();
            if self.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
    }
}

impl Drop for AcquisitionDrainGuard {
    fn drop(&mut self) {
        drop(self.permit.take());
        self.drained.finish();
    }
}

impl OwnerToolOperations {
    pub(crate) fn register_ready_bodies(
        &self,
        lane: &OwnerLane,
        starts: &[OplogIndex],
        result_await_parent: Option<&OwnerInvocationId>,
    ) -> Result<(Vec<OwnerInvocationId>, Option<OwnerLaneWait>), WorkerExecutorError> {
        let mut state = self.state.lock().unwrap();
        if !state.phase.is_open() {
            return Err(WorkerExecutorError::runtime(
                "owner generation was fenced before tool lane registration",
            ));
        }
        let mut starts = starts.to_vec();
        starts.sort_unstable();
        let mut operations = state
            .operations
            .values_mut()
            .filter_map(|operation| {
                let start = operation.invocation_id.as_ref()?.start_index();
                starts.binary_search(&start).ok()?;
                Some((start, operation))
            })
            .collect::<Vec<_>>();
        operations.sort_unstable_by_key(|(start, _)| *start);
        if !operations.iter().map(|(start, _)| start).eq(starts.iter()) {
            return Err(WorkerExecutorError::runtime(
                "tool lane registration requires distinct registered operations",
            ));
        }
        let registrations = operations
            .into_iter()
            .map(|(start, operation)| {
                let invocation_id = operation.invocation_id.clone().ok_or_else(|| {
                    WorkerExecutorError::runtime("tool operation was not durably accepted")
                })?;
                let ready = operation.state.ready_body().ok_or_else(|| {
                    WorkerExecutorError::runtime(format!(
                        "tool operation at {start} is not ready for lane registration"
                    ))
                })?;
                Ok((
                    ready,
                    operation.context.parent.clone(),
                    invocation_id,
                    operation.context.call_mode,
                    operation.context.activation.filesystem(),
                ))
            })
            .collect::<Result<Vec<_>, WorkerExecutorError>>()?;
        let mut tickets = Vec::with_capacity(registrations.len());
        let mut invocations = Vec::with_capacity(registrations.len());
        for (ready, parent, invocation_id, call_mode, filesystem) in registrations {
            let ticket = lane
                .register_entity(parent, invocation_id.clone(), call_mode, filesystem)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            invocations.push(OwnerInvocationId::Entity(invocation_id));
            tickets.push((ready, ticket));
        }
        let wait = result_await_parent
            .filter(|_| !invocations.is_empty())
            .map(|parent| {
                lane.await_invocations(parent, invocations.clone())
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
            })
            .transpose()?;
        for (ready, ticket) in tickets {
            ready.queue(ticket);
        }
        drop(state);
        self.changed.notify_waiters();
        Ok((invocations, wait))
    }
}

impl OwnerToolOperation {
    /// Registers with the unchanged GOL-33 owner lane and moves the returned ticket under operation
    /// arbitration before returning. No await or externally visible intermediate ownership exists.
    #[cfg(test)]
    pub(crate) fn register_body(&self, lane: &OwnerLane) -> Result<bool, WorkerExecutorError> {
        let mut state = self.owner.state.lock().unwrap();
        if !state.phase.is_open() {
            return Ok(false);
        }
        let Some(operation) = state.operations.get_mut(&self.id) else {
            return Ok(false);
        };
        let invocation_id = operation.invocation_id.clone().ok_or_else(|| {
            WorkerExecutorError::runtime("tool operation was not durably accepted")
        })?;
        let Some(ready) = operation.state.ready_body() else {
            return Ok(false);
        };
        match lane.register_entity(
            self.context.parent.clone(),
            invocation_id,
            self.context.call_mode,
            self.context.activation.filesystem(),
        ) {
            Ok(ticket) => {
                ready.queue(ticket);
                Ok(true)
            }
            Err(error) => Err(WorkerExecutorError::runtime(error.to_string())),
        }
    }

    /// Acquires an already-owned ticket. If cancellation or owner fencing won while the lane grant
    /// was pending, the granted permit is completed here and no guest body may start.
    pub(crate) async fn acquire_registered_body(&self) -> Result<bool, WorkerExecutorError> {
        let drained = Arc::new(AcquisitionDrain::default());
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let task = {
            let mut state = self.owner.state.lock().unwrap();
            let Some(operation) = state.operations.get_mut(&self.id) else {
                return Ok(false);
            };
            if operation.state.admission() != BodyAdmissionState::Registered {
                return Ok(false);
            }
            let (ticket, acquisition) = operation.state.take_ticket()?;
            let owner = self.owner.clone();
            let operation_id = self.id;
            let task_drain = drained.clone();
            let drain = AcquisitionDrainGuard {
                drained: task_drain,
                permit: None,
            };
            let task = tokio::spawn(async move {
                let mut drain = drain;
                let _ = start_rx.await;
                match ticket.acquire().await {
                    Ok(permit) => drain.permit = Some(permit),
                    Err(error) => {
                        let mut state = owner.state.lock().unwrap();
                        let operation =
                            state.operations.get_mut(&operation_id).ok_or_else(|| {
                                WorkerExecutorError::runtime("acquiring tool operation was removed")
                            })?;
                        operation.state.fail_acquisition(error.to_string());
                        return Ok::<(), WorkerExecutorError>(());
                    }
                }

                loop {
                    let wait = owner.changed.notified();
                    let should_wait = {
                        let mut state = owner.state.lock().unwrap();
                        let owner_open = state.phase.is_open();
                        let operation =
                            state.operations.get_mut(&operation_id).ok_or_else(|| {
                                WorkerExecutorError::runtime("acquiring tool operation was removed")
                            })?;
                        if !matches!(operation.state.lane(), LaneOwnership::Acquiring(_)) {
                            return Ok(());
                        }
                        if owner_open
                            && matches!(operation.state.winner(), ToolOperationWinner::Open)
                        {
                            let acquiring = operation.state.acquiring_body().ok_or_else(|| {
                                WorkerExecutorError::runtime("tool body is not acquiring its lane")
                            })?;
                            let permit = drain.permit.take().ok_or_else(|| {
                                WorkerExecutorError::runtime(
                                    "acquired lane permit is no longer task-owned",
                                )
                            })?;
                            acquiring.install_grant(permit);
                            false
                        } else if owner_open && operation.state.winner().is_selecting() {
                            true
                        } else {
                            // A committed operation or owner failure will detach and abort this
                            // acquisition. If it won before this task observed the state change,
                            // release the permit here instead.
                            drop(drain.permit.take());
                            drop(operation.state.take_lane_for_drain());
                            false
                        }
                    };
                    if should_wait {
                        wait.await;
                    } else {
                        break;
                    }
                }
                Ok(())
            });
            acquisition.install(AcquisitionControl {
                abort: task.abort_handle(),
                drained: drained.clone(),
            });
            task
        };
        let start_failed = start_tx.send(()).is_err();
        drained.wait().await;
        match task.await {
            Ok(result) => result?,
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                return Err(WorkerExecutorError::runtime(format!(
                    "tool lane acquisition task failed: {error}"
                )));
            }
        }
        let mut state = self.owner.state.lock().unwrap();
        let owner_open = state.phase.is_open();
        let operation = state
            .operations
            .get_mut(&self.id)
            .ok_or_else(|| WorkerExecutorError::runtime("acquiring tool operation was removed"))?;
        if let Some(error) = operation.state.take_acquisition_error() {
            return Err(WorkerExecutorError::runtime(error));
        }
        if start_failed && !operation.state.winner().is_terminal() && owner_open {
            drop(operation.state.take_lane_for_drain());
            return Err(WorkerExecutorError::runtime(
                "tool lane acquisition failed to start",
            ));
        }
        Ok(operation.state.admission() == BodyAdmissionState::Running
            && matches!(operation.state.lane(), LaneOwnership::Permit { .. })
            && matches!(operation.state.winner(), ToolOperationWinner::Open))
    }

    pub(crate) async fn wait_until_registered(&self) -> bool {
        enum RegistrationStatus {
            Waiting,
            Registered,
            Terminal,
        }
        loop {
            let changed = self.owner.changed.notified();
            let status = {
                let state = self.owner.state.lock().unwrap();
                match state.operations.get(&self.id) {
                    None => RegistrationStatus::Terminal,
                    Some(operation) if operation.state.winner().is_terminal() => {
                        RegistrationStatus::Terminal
                    }
                    Some(operation)
                        if operation.state.admission() == BodyAdmissionState::Registered =>
                    {
                        RegistrationStatus::Registered
                    }
                    Some(_) => RegistrationStatus::Waiting,
                }
            };
            match status {
                RegistrationStatus::Registered => return true,
                RegistrationStatus::Terminal => return false,
                RegistrationStatus::Waiting => changed.await,
            }
        }
    }
}

pub(super) async fn drain_lane_ownerships(lanes: Vec<LaneOwnership>) {
    for lane in lanes {
        if let LaneOwnership::Acquiring(control) = lane {
            control.abort.abort();
            control.drained.wait().await;
        }
    }
}
