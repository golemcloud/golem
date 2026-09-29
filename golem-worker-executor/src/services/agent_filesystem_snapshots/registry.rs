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
//! The registry holds one [`Scopes`] value and changes it only through the transitions of
//! [`rules`], each with its own answer. Each ticket makes its transition in its constructor, and
//! its `Drop` only reports the end of what it holds.

use super::JobDecision;
use super::rules::{self, JobId, Refusal, Scopes, Transition};
use crate::filesystem_snapshot::SnapshotScope;
use golem_common::model::oplog::FilesystemSnapshotName;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// The state of the scopes of the service, and the signal that wakes its waiters.
#[derive(Default)]
pub(super) struct Registry {
    scopes: Mutex<Scopes>,
    changed: watch::Sender<()>,
}

impl Registry {
    /// Applies the transition `f`, which is the transition `transition` of [`rules`], and wakes
    /// the waiters when [`rules::wakes`] says so. No lock is held across an await, so the scopes
    /// of a poisoned lock are used as they are.
    fn apply<T>(&self, transition: Transition, f: impl FnOnce(&mut Scopes) -> T) -> T {
        let answer = f(&mut self.scopes.lock().unwrap_or_else(PoisonError::into_inner));
        if rules::wakes(transition) {
            self.changed.send_modify(|()| {});
        }
        answer
    }

    /// Waits until `found` gives a value for the scopes. Gives `None` when the registry is gone.
    async fn until<T>(&self, found: impl Fn(&Scopes) -> Option<T>) -> Option<T> {
        let mut value = None;
        self.changed
            .subscribe()
            .wait_for(|()| {
                value = found(&self.scopes.lock().unwrap_or_else(PoisonError::into_inner));
                value.is_some()
            })
            .await
            .ok()?;
        value
    }

    /// Reads the scopes now.
    #[cfg(test)]
    pub(super) fn read<T>(&self, read: impl FnOnce(&Scopes) -> T) -> T {
        read(&self.scopes.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits until the job `id` of `scope` is gone. Gives at once when the registry is gone.
    pub(super) async fn until_job_gone(&self, scope: &SnapshotScope, id: JobId) {
        self.until(|scopes| rules::has_ended(scopes, scope, id).then_some(()))
            .await;
    }

    /// Waits until no job runs for `scope`. Gives at once when the registry is gone.
    pub(super) async fn until_scope_free(&self, scope: &SnapshotScope) {
        self.until(|scopes| rules::is_free(scopes, scope).then_some(()))
            .await;
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
    /// volume has room for a capture.
    pub(super) fn admit(
        registry: &Arc<Registry>,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
        stop: CancellationToken,
        room: bool,
    ) -> Result<Self, Refusal> {
        let id = registry.apply(Transition::Admit, |scopes| {
            rules::admit(scopes, scope, name, stop.clone(), room)
        })?;
        Ok(Self {
            registry: Arc::clone(registry),
            scope: scope.clone(),
            id,
            stop,
        })
    }

    /// The save of the job holds a slot of the uploads.
    pub(super) fn saving(&self) {
        self.registry.apply(Transition::Saving, |scopes| {
            rules::saving(scopes, &self.scope, self.id)
        });
    }

    /// Records the decision of the job. The first decision stays.
    pub(super) fn decide(&self, decision: JobDecision) {
        self.registry.apply(Transition::Decide, |scopes| {
            rules::decide(scopes, &self.scope, self.id, decision)
        });
    }

    /// The stop of the job. A delete of the scope and the shutdown cancel it.
    pub(super) fn stop(&self) -> &CancellationToken {
        &self.stop
    }

    pub(super) fn scope(&self) -> &SnapshotScope {
        &self.scope
    }
}

impl Drop for JobTicket {
    fn drop(&mut self) {
        self.registry.apply(Transition::End, |scopes| {
            rules::end(scopes, &self.scope, self.id)
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
        let stop = registry.apply(Transition::ForgetScope, |scopes| {
            rules::forget_scope(scopes, scope)
        });
        (
            Self {
                registry: Arc::clone(registry),
                scope: scope.clone(),
            },
            stop,
        )
    }

    /// Waits until no job runs for the scope. A job cannot start while the delete is marked.
    pub(super) async fn until_scope_free(&self) {
        self.registry.until_scope_free(&self.scope).await;
    }
}

impl Drop for DeleteTicket {
    fn drop(&mut self) {
        self.registry.apply(Transition::ScopeDeleted, |scopes| {
            rules::scope_deleted(scopes, &self.scope)
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
        let id = registry.apply(Transition::StartWait, |scopes| {
            rules::start_wait(scopes, scope, name)
        })?;
        Ok(Self {
            registry: Arc::clone(registry),
            scope: scope.clone(),
            id,
        })
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
        self.registry.apply(Transition::Unwatch, |scopes| {
            rules::unwatch(scopes, &self.scope, self.id)
        });
    }
}
