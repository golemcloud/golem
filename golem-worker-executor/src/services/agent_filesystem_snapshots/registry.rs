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

//! The state of the scopes, and the tickets that report to it.
//!
//! The registry holds one [`Scopes`] value and changes it only through [`rules::step`]. Each
//! ticket makes its transition in its constructor, after the step returns, and its `Drop` only
//! reports the end of what it holds.

use super::JobDecision;
use super::SnapshotSkip;
use super::rules::{self, Answer, JobId, Request, Scopes};
use crate::filesystem_snapshot::SnapshotScope;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The state of the scopes of the service.
pub(super) struct Registry(watch::Sender<Scopes>);

impl Default for Registry {
    fn default() -> Self {
        Self(watch::channel(Scopes::default()).0)
    }
}

impl Registry {
    /// Applies `request`, and wakes the waiters when [`rules::wakes`] says so.
    pub(super) fn step(&self, request: Request) -> Answer {
        let wakes = rules::wakes(&request);
        let mut answer = Answer::Done;
        self.0.send_if_modified(|scopes| {
            answer = rules::step(scopes, request);
            wakes
        });
        answer
    }

    /// Waits until `found` gives a value for the scopes. Gives `None` when the registry is gone.
    pub(super) async fn until<T>(&self, found: impl Fn(&Scopes) -> Option<T>) -> Option<T> {
        let mut value = None;
        self.0
            .subscribe()
            .wait_for(|scopes| {
                value = found(scopes);
                value.is_some()
            })
            .await
            .ok()?;
        value
    }

    /// Reads the scopes now.
    #[cfg(test)]
    pub(super) fn read<T>(&self, read: impl FnOnce(&Scopes) -> T) -> T {
        read(&self.0.borrow())
    }
}

/// The admitted job of a scope. Dropping it ends the job.
pub(super) struct JobTicket {
    registry: Arc<Registry>,
    scope: SnapshotScope,
    id: JobId,
    stop: CancellationToken,
}

impl JobTicket {
    /// Admits a job with `name` for `scope`. `stop` stops the job, and `room` tells whether the
    /// volume has room for a capture. The error carries the job that runs for the scope.
    pub(super) fn admit(
        registry: &Arc<Registry>,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
        stop: CancellationToken,
        room: bool,
    ) -> Result<Self, (SnapshotSkip, Option<JobId>)> {
        match registry.step(Request::Admit {
            scope: scope.clone(),
            name: name.clone(),
            stop: stop.clone(),
            room,
        }) {
            Answer::Admitted(id) => Ok(Self {
                registry: Arc::clone(registry),
                scope: scope.clone(),
                id,
                stop,
            }),
            Answer::Refused { skip, running } => Err((skip, running)),
            Answer::Wait(_) | Answer::NoWait(_) | Answer::Stop(_) | Answer::Done => {
                Err((SnapshotSkip::UploadInFlight, None))
            }
        }
    }

    /// The save of the job holds a slot of the uploads.
    pub(super) fn saving(&self) {
        self.registry.step(Request::Saving {
            scope: self.scope.clone(),
            id: self.id,
        });
    }

    /// Records the decision of the job. The first decision stays.
    pub(super) fn decide(&self, decision: JobDecision) {
        self.registry.step(Request::Decide {
            scope: self.scope.clone(),
            id: self.id,
            decision,
        });
    }

    /// The stop of the job. `forget_scope` and the shutdown cancel it.
    pub(super) fn stop(&self) -> &CancellationToken {
        &self.stop
    }

    pub(super) fn scope(&self) -> &SnapshotScope {
        &self.scope
    }
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        self.registry.step(Request::End {
            scope: self.scope.clone(),
            id: self.id,
        });
    }
}

/// A queued or running delete of a scope. Dropping it ends the delete.
pub(super) struct DeleteTicket {
    registry: Arc<Registry>,
    scope: SnapshotScope,
}

impl DeleteTicket {
    /// Marks a delete of `scope`, and gives the stop of the job of the scope, when one runs.
    pub(super) fn forget_scope(
        registry: &Arc<Registry>,
        scope: &SnapshotScope,
    ) -> (Self, Option<CancellationToken>) {
        let stop = match registry.step(Request::ForgetScope {
            scope: scope.clone(),
        }) {
            Answer::Stop(stop) => stop,
            Answer::Admitted(_)
            | Answer::Refused { .. }
            | Answer::Wait(_)
            | Answer::NoWait(_)
            | Answer::Done => None,
        };
        (
            Self {
                registry: Arc::clone(registry),
                scope: scope.clone(),
            },
            stop,
        )
    }

    /// Waits until no job runs for the scope. A job cannot start while the delete is marked.
    pub(super) async fn jobs_ended(&self) {
        self.registry
            .until(|scopes| rules::is_free(scopes, &self.scope).then_some(()))
            .await;
    }
}

impl Drop for DeleteTicket {
    fn drop(&mut self) {
        self.registry.step(Request::ScopeDeleted {
            scope: self.scope.clone(),
        });
    }
}

/// A start that waits for the decision of a job. Dropping it ends the wait.
pub(super) struct WaitTicket {
    registry: Arc<Registry>,
    scope: SnapshotScope,
    id: JobId,
}

impl WaitTicket {
    /// Registers a wait for the job of `scope` with `name`, when the start waits for it. Gives
    /// the decision that the job has now otherwise.
    pub(super) fn start_wait(
        registry: &Arc<Registry>,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<Self, Option<JobDecision>> {
        match registry.step(Request::StartWait {
            scope: scope.clone(),
            name: name.clone(),
        }) {
            Answer::Wait(id) => Ok(Self {
                registry: Arc::clone(registry),
                scope: scope.clone(),
                id,
            }),
            Answer::NoWait(decision) => Err(decision),
            Answer::Admitted(_) | Answer::Refused { .. } | Answer::Stop(_) | Answer::Done => {
                Err(None)
            }
        }
    }

    /// Waits for the decision of the job. A job that ended without one, or a registry that is
    /// gone, gives `Stopped`.
    pub(super) async fn decided(&self) -> JobDecision {
        self.registry
            .until(|scopes| rules::decision_of(scopes, &self.scope, self.id))
            .await
            .unwrap_or(JobDecision::Stopped)
    }
}

impl Drop for WaitTicket {
    fn drop(&mut self) {
        self.registry.step(Request::Unwatch {
            scope: self.scope.clone(),
            id: self.id,
        });
    }
}

/// Waits until the job `id` of `scope` ended. Gives at once when the registry is gone.
pub(super) async fn job_ended(registry: &Registry, scope: &SnapshotScope, id: JobId) {
    registry
        .until(|scopes| rules::has_ended(scopes, scope, id).then_some(()))
        .await;
}
