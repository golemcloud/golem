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

//! The rules of the service, as pure functions over plain values: the transitions of the jobs
//! and the scope deletes, the decisions of a job, the plan of a start, and the admission of a
//! manual update. Nothing here waits, reads a clock or calls the store.

use super::{ConfirmOutcome, JobDecision, SnapshotKind, SnapshotSkip};
use crate::filesystem_snapshot::{SnapshotInfo, SnapshotScope, SnapshotStoreError};
use crate::sandbox_filesystem::FilesystemSpace;
use crate::services::golem_config::{
    FilesystemPressureConfig, FilesystemSnapshotStoreConfig, FilesystemSnapshotsConfig,
};
use golem_common::model::RetryConfig;
use golem_common::model::oplog::FilesystemSnapshotName;
use golem_common::retries::get_delay;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// The number of a job. It is unique for the life of the process.
pub(super) type JobId = u64;

/// The jobs of the scopes, the scope deletes, and the decisions of ended jobs that a start waits
/// for.
#[derive(Debug, Default)]
pub(super) struct Scopes {
    jobs: HashMap<SnapshotScope, Job>,
    /// The number of queued or running deletes of each scope.
    deleting: HashMap<SnapshotScope, NonZeroU32>,
    /// The last ended job of each scope that a start still waits for.
    ended: HashMap<SnapshotScope, Ended>,
    /// The number of the last admitted job.
    last_job: JobId,
}

/// The job of one scope, from its admission to its end.
#[derive(Debug)]
struct Job {
    id: JobId,
    name: FilesystemSnapshotName,
    phase: JobPhase,
    /// Stops the job. It is a child of the shutdown token.
    stop: CancellationToken,
    /// The number of starts that wait for the decision of the job.
    waiters: u32,
}

/// The decision of an ended job, while starts wait for it.
#[derive(Debug)]
struct Ended {
    id: JobId,
    decision: JobDecision,
    waiters: NonZeroU32,
}

/// Where a job is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum JobPhase {
    /// The job has its admission and waits for its capture or for a slot of the uploads.
    Admitted,
    /// The save of the job holds a slot of the uploads.
    Saving,
    /// The job knows how it ended its work on the snapshot.
    Decided(JobDecision),
}

/// A transition of [`Scopes`]. The registry wakes its waiters after a transition when
/// [`wakes`] says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Transition {
    Admit,
    Saving,
    Decide,
    End,
    StartWait,
    Unwatch,
    ForgetScope,
    ScopeDeleted,
}

/// Whether the waiters of the registry must see `transition`. A waiter waits for a decision, for
/// the end of a job, or for the end of a scope delete.
pub(super) fn wakes(transition: Transition) -> bool {
    match transition {
        Transition::Decide
        | Transition::End
        | Transition::ForgetScope
        | Transition::ScopeDeleted => true,
        Transition::Admit | Transition::Saving | Transition::StartWait | Transition::Unwatch => {
            false
        }
    }
}

/// Why an admission gives no job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Refusal {
    pub(super) skip: SnapshotSkip,
    /// The job that runs for the scope, when one runs.
    pub(super) running: Option<JobId>,
}

/// Admits a job with `name` for `scope`, in the order room, scope delete, running job. `stop`
/// stops the job, and `room` tells whether the volume has room for a capture.
pub(super) fn admit(
    scopes: &mut Scopes,
    scope: &SnapshotScope,
    name: &FilesystemSnapshotName,
    stop: CancellationToken,
    room: bool,
) -> Result<JobId, Refusal> {
    let running = scopes.jobs.get(scope).map(|job| job.id);
    let refused = |skip| Err(Refusal { skip, running });
    if !room {
        return refused(SnapshotSkip::VolumeUnderPressure);
    }
    if scopes.deleting.contains_key(scope) {
        return refused(SnapshotSkip::ScopeDeleting);
    }
    if running.is_some() {
        return refused(SnapshotSkip::UploadInFlight);
    }
    scopes.last_job += 1;
    let id = scopes.last_job;
    scopes.jobs.insert(
        scope.clone(),
        Job {
            id,
            name: name.clone(),
            phase: JobPhase::Admitted,
            stop,
            waiters: 0,
        },
    );
    Ok(id)
}

/// The save of the job `id` holds a slot of the uploads. The phase only moves forward.
pub(super) fn saving(scopes: &mut Scopes, scope: &SnapshotScope, id: JobId) {
    if let Some(job) = live(scopes, scope, id)
        && job.phase == JobPhase::Admitted
    {
        job.phase = JobPhase::Saving;
    }
}

/// The job `id` decided. The first decision stays.
pub(super) fn decide(scopes: &mut Scopes, scope: &SnapshotScope, id: JobId, decision: JobDecision) {
    if let Some(job) = live(scopes, scope, id)
        && !matches!(job.phase, JobPhase::Decided(_))
    {
        job.phase = JobPhase::Decided(decision);
    }
}

/// A delete of `scope` is queued. Gives the stop of the job of the scope, when one runs.
pub(super) fn forget_scope(
    scopes: &mut Scopes,
    scope: &SnapshotScope,
) -> Option<CancellationToken> {
    let stop = scopes.jobs.get(scope).map(|job| job.stop.clone());
    scopes
        .deleting
        .entry(scope.clone())
        .and_modify(|count| *count = count.saturating_add(1))
        .or_insert(NonZeroU32::MIN);
    stop
}

/// A delete of `scope` ended. The last one frees the scope and the ended decision of the scope.
pub(super) fn scope_deleted(scopes: &mut Scopes, scope: &SnapshotScope) {
    let left = scopes
        .deleting
        .get(scope)
        .and_then(|count| NonZeroU32::new(count.get() - 1));
    match left {
        Some(left) => {
            scopes.deleting.insert(scope.clone(), left);
        }
        None => {
            scopes.deleting.remove(scope);
        }
    }
    scopes.ended.remove(scope);
}

/// The live job `id` of `scope`.
fn live<'a>(scopes: &'a mut Scopes, scope: &SnapshotScope, id: JobId) -> Option<&'a mut Job> {
    scopes.jobs.get_mut(scope).filter(|job| job.id == id)
}

/// Frees the scope of the job `id`, and keeps its decision while starts wait for it.
pub(super) fn end(scopes: &mut Scopes, scope: &SnapshotScope, id: JobId) {
    if scopes.jobs.get(scope).is_some_and(|job| job.id == id)
        && let Some(job) = scopes.jobs.remove(scope)
        && let Some(waiters) = NonZeroU32::new(job.waiters)
    {
        let decision = match job.phase {
            JobPhase::Decided(decision) => decision,
            JobPhase::Admitted | JobPhase::Saving => JobDecision::Stopped,
        };
        scopes.ended.insert(
            scope.clone(),
            Ended {
                id,
                decision,
                waiters,
            },
        );
    }
}

/// A start waits only for a job of the scope with the name that holds a slot of the uploads and
/// has not decided. It then registers as a waiter in the same transition and gets the job. A
/// start that does not wait gets the decision of the job with the name, when known.
pub(super) fn start_wait(
    scopes: &mut Scopes,
    scope: &SnapshotScope,
    name: &FilesystemSnapshotName,
) -> Result<JobId, Option<JobDecision>> {
    match scopes.jobs.get_mut(scope).filter(|job| &job.name == name) {
        Some(job) => match job.phase {
            JobPhase::Saving => {
                job.waiters += 1;
                Ok(job.id)
            }
            JobPhase::Decided(decision) => Err(Some(decision)),
            JobPhase::Admitted => Err(None),
        },
        None => Err(None),
    }
}

/// Takes one waiter from the job `id`, live or ended.
pub(super) fn unwatch(scopes: &mut Scopes, scope: &SnapshotScope, id: JobId) {
    if let Some(job) = live(scopes, scope, id) {
        job.waiters = job.waiters.saturating_sub(1);
        return;
    }
    let left = scopes
        .ended
        .get(scope)
        .filter(|ended| ended.id == id)
        .map(|ended| NonZeroU32::new(ended.waiters.get() - 1));
    match left {
        Some(Some(waiters)) => {
            if let Some(ended) = scopes.ended.get_mut(scope) {
                ended.waiters = waiters;
            }
        }
        Some(None) => {
            scopes.ended.remove(scope);
        }
        None => {}
    }
}

/// The decision of the job `id` of `scope`, or `None` while it runs undecided. A job that ended
/// without a kept decision counts as stopped.
pub(super) fn decision_of(
    scopes: &Scopes,
    scope: &SnapshotScope,
    id: JobId,
) -> Option<JobDecision> {
    match scopes.jobs.get(scope).filter(|job| job.id == id) {
        Some(job) => match job.phase {
            JobPhase::Decided(decision) => Some(decision),
            JobPhase::Admitted | JobPhase::Saving => None,
        },
        None => Some(
            scopes
                .ended
                .get(scope)
                .filter(|ended| ended.id == id)
                .map_or(JobDecision::Stopped, |ended| ended.decision),
        ),
    }
}

/// Whether the job `id` of `scope` ended.
pub(super) fn has_ended(scopes: &Scopes, scope: &SnapshotScope, id: JobId) -> bool {
    scopes.jobs.get(scope).is_none_or(|job| job.id != id)
}

/// Whether no job runs for `scope`.
pub(super) fn is_free(scopes: &Scopes, scope: &SnapshotScope) -> bool {
    !scopes.jobs.contains_key(scope)
}

/// What one save attempt gave.
#[derive(Debug)]
pub(super) enum SaveAttempt {
    /// The store holds the snapshot.
    Saved(SnapshotInfo),
    /// The store already holds the name. Each name belongs to one capture, so it is the tree of
    /// an earlier attempt of the same job, and a `stat` of the name gives its info.
    StatOwn,
    /// The attempt failed.
    Failed(SnapshotStoreError),
}

/// Classifies the result of a save of the own name of a job.
pub(super) fn save_attempt(result: Result<SnapshotInfo, SnapshotStoreError>) -> SaveAttempt {
    match result {
        Ok(info) => SaveAttempt::Saved(info),
        Err(SnapshotStoreError::AlreadyExists) => SaveAttempt::StatOwn,
        Err(error) => SaveAttempt::Failed(error),
    }
}

/// The delay before the next attempt after attempt number `attempt` failed with `error`, or
/// `None` when no attempt follows.
pub(super) fn retry_delay(
    retry: &RetryConfig,
    attempt: u32,
    error: &SnapshotStoreError,
) -> Option<Duration> {
    matches!(
        error,
        SnapshotStoreError::Storage {
            retryable: true,
            ..
        }
    )
    .then(|| get_delay(retry, attempt))
    .flatten()
}

/// What a job does after its confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FollowUp {
    /// Applies retention.
    Retain,
    /// Deletes the own snapshot, which no confirmation record names.
    DeleteOwn,
    /// Keeps the snapshot and does nothing more.
    Keep,
}

/// The follow-up of a job of `kind` whose confirmation gave `outcome`. Only a confirmed periodic
/// snapshot applies retention, and only a superseded one is deleted.
pub(super) fn follow_up(kind: SnapshotKind, outcome: ConfirmOutcome) -> FollowUp {
    match (kind, outcome) {
        (SnapshotKind::Periodic, ConfirmOutcome::Confirmed) => FollowUp::Retain,
        (_, ConfirmOutcome::Superseded) => FollowUp::DeleteOwn,
        (SnapshotKind::Update, ConfirmOutcome::Confirmed) | (_, ConfirmOutcome::Deferred) => {
            FollowUp::Keep
        }
    }
}

/// What a start does after its wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StoreCheck {
    /// It asks the store nothing.
    Skip,
    /// It asks the store once, for at most this time.
    Stat(Duration),
}

/// The limits of the store check of a start.
#[derive(Clone, Copy, Debug)]
pub(super) struct StoreCheckLimits {
    /// The wait for an upload and the check after it take at most this time together.
    pub(super) confirmation_wait: Duration,
    /// The check of a start that did not wait takes at most this time.
    pub(super) store_check_limit: Duration,
}

/// The store check of a start. It skips the check when the local job gave `Superseded` or a
/// terminal interrupt waits. A start that waited for `waited` checks for what is left of the
/// confirmation wait, and a start that did not wait for the store check limit.
pub(super) fn store_check(
    decision: Option<JobDecision>,
    terminal: bool,
    waited: Option<Duration>,
    limits: StoreCheckLimits,
) -> StoreCheck {
    if terminal || decision == Some(JobDecision::Confirmed(ConfirmOutcome::Superseded)) {
        return StoreCheck::Skip;
    }
    StoreCheck::Stat(waited.map_or(limits.store_check_limit, |waited| {
        limits.confirmation_wait.saturating_sub(waited)
    }))
}

/// What a manual update does after a refused admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UpdateAdmit {
    /// It fails with the refusal.
    Refuse,
    /// It waits for the end of the running job, then asks once more.
    WaitForEnd(JobId),
}

/// What a manual update does after its first admission gave `refusal`. Only an upload that runs
/// makes it wait. The admission after the wait fails with its refusal, so the update waits at
/// most once.
pub(super) fn update_admission(refusal: Refusal) -> UpdateAdmit {
    match (refusal.skip, refusal.running) {
        (SnapshotSkip::UploadInFlight, Some(id)) => UpdateAdmit::WaitForEnd(id),
        _ => UpdateAdmit::Refuse,
    }
}

/// What a service binds to.
#[derive(Debug)]
pub(super) enum Binding<'a> {
    /// The service keeps no filesystem snapshots.
    Disabled,
    /// The service keeps filesystem snapshots with the store and settings of `config`.
    Managed(&'a FilesystemSnapshotStoreConfig),
}

/// What `config` binds to. `Managed` needs a sandbox provisioning on managed XFS storage, and
/// `managed_storage` tells whether the executor has it.
pub(super) fn binding(
    config: &FilesystemSnapshotsConfig,
    managed_storage: bool,
) -> Result<Binding<'_>, String> {
    match config {
        FilesystemSnapshotsConfig::Disabled(_) => Ok(Binding::Disabled),
        FilesystemSnapshotsConfig::Managed(_) if !managed_storage => {
            Err("filesystem snapshots require managed XFS storage".to_string())
        }
        FilesystemSnapshotsConfig::Managed(config) => Ok(Binding::Managed(config)),
    }
}

/// Whether a volume with `space` has room for a new capture: its free space reaches the targets
/// of `pressure`. An unmanaged volume always has room, and a volume whose space is not known has
/// none.
pub(super) fn has_room(
    space: Option<&FilesystemSpace>,
    pressure: &FilesystemPressureConfig,
) -> bool {
    match space {
        Some(FilesystemSpace::Unlimited) => true,
        Some(FilesystemSpace::Observed {
            available_bytes,
            available_filesystem_objects,
            ..
        }) => {
            *available_bytes >= pressure.target_available_bytes()
                && *available_filesystem_objects >= pressure.target_available_filesystem_objects()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::ComponentId;
    use golem_common::model::environment::EnvironmentId;
    use golem_common::model::{AgentId, OwnedAgentId};
    use test_r::test;

    fn scope(name: &str) -> SnapshotScope {
        SnapshotScope::agent(&OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId {
                component_id: ComponentId::new(),
                agent_id: name.to_string(),
            },
        ))
    }

    fn try_admit(
        scopes: &mut Scopes,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
        room: bool,
    ) -> Result<JobId, Refusal> {
        admit(scopes, scope, name, CancellationToken::new(), room)
    }

    fn admitted(
        scopes: &mut Scopes,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> JobId {
        try_admit(scopes, scope, name, true).expect("admitted")
    }

    fn refusal(result: Result<JobId, Refusal>) -> Option<(SnapshotSkip, Option<JobId>)> {
        result.err().map(|refusal| (refusal.skip, refusal.running))
    }

    fn end_job(scopes: &mut Scopes, scope: &SnapshotScope, id: JobId) {
        end(scopes, scope, id);
    }

    fn wait(
        scopes: &mut Scopes,
        scope: &SnapshotScope,
        name: &FilesystemSnapshotName,
    ) -> Result<JobId, Option<JobDecision>> {
        start_wait(scopes, scope, name)
    }

    #[test]
    fn an_admission_checks_room_then_a_scope_delete_then_a_running_job() {
        let mut scopes = Scopes::default();
        let scope = scope("order");
        let other = self::scope("other");
        let name = FilesystemSnapshotName::periodic();

        forget_scope(&mut scopes, &scope);
        let deleting_without_room = refusal(try_admit(&mut scopes, &scope, &name, false));
        let deleting = refusal(try_admit(&mut scopes, &scope, &name, true));
        let first = admitted(&mut scopes, &other, &name);
        let running = refusal(try_admit(&mut scopes, &other, &name, true));
        let running_without_room = refusal(try_admit(&mut scopes, &other, &name, false));

        assert_eq!(
            deleting_without_room,
            Some((SnapshotSkip::VolumeUnderPressure, None))
        );
        assert_eq!(deleting, Some((SnapshotSkip::ScopeDeleting, None)));
        assert_eq!(running, Some((SnapshotSkip::UploadInFlight, Some(first))));
        assert_eq!(
            running_without_room,
            Some((SnapshotSkip::VolumeUnderPressure, Some(first)))
        );
    }

    #[test]
    fn a_job_number_is_never_given_twice_and_an_end_frees_only_its_own_job() {
        let mut scopes = Scopes::default();
        let scope = scope("numbers");
        let name = FilesystemSnapshotName::periodic();

        let first = admitted(&mut scopes, &scope, &name);
        end_job(&mut scopes, &scope, first);
        let second = admitted(&mut scopes, &scope, &name);
        end_job(&mut scopes, &scope, first);
        let still_running = !is_free(&scopes, &scope);
        end_job(&mut scopes, &scope, second);

        assert!(second > first);
        assert!(still_running);
        assert!(is_free(&scopes, &scope));
        assert!(has_ended(&scopes, &scope, second));
    }

    #[test]
    fn the_phase_only_moves_forward_and_the_first_decision_stays() {
        let mut scopes = Scopes::default();
        let scope = scope("phase");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut scopes, &scope, &name);

        let admitted_decision = decision_of(&scopes, &scope, id);
        saving(&mut scopes, &scope, id);
        let saving_decision = decision_of(&scopes, &scope, id);
        decide(&mut scopes, &scope, id, JobDecision::SaveFailed);
        decide(
            &mut scopes,
            &scope,
            id,
            JobDecision::Confirmed(ConfirmOutcome::Confirmed),
        );
        saving(&mut scopes, &scope, id);
        let decided = decision_of(&scopes, &scope, id);
        let phase = scopes.jobs.get(&scope).map(|job| job.phase);

        assert_eq!(
            (admitted_decision, saving_decision, decided, phase),
            (
                None,
                None,
                Some(JobDecision::SaveFailed),
                Some(JobPhase::Decided(JobDecision::SaveFailed))
            )
        );
    }

    #[test]
    fn a_start_waits_only_for_a_saving_undecided_job_with_its_name() {
        let mut scopes = Scopes::default();
        let scope = scope("start-wait");
        let name = FilesystemSnapshotName::periodic();
        let other = FilesystemSnapshotName::periodic();

        let no_job = wait(&mut scopes, &scope, &name);
        let id = admitted(&mut scopes, &scope, &name);
        let while_admitted = wait(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, id);
        let other_name = wait(&mut scopes, &scope, &other);
        let while_saving = wait(&mut scopes, &scope, &name);
        let waiters = scopes.jobs.get(&scope).map(|job| job.waiters);
        decide(
            &mut scopes,
            &scope,
            id,
            JobDecision::Confirmed(ConfirmOutcome::Deferred),
        );
        let decided = wait(&mut scopes, &scope, &name);

        assert_eq!(no_job, Err(None));
        assert_eq!(while_admitted, Err(None));
        assert_eq!(other_name, Err(None));
        assert_eq!(while_saving, Ok(id));
        assert_eq!(waiters, Some(1));
        assert_eq!(
            decided,
            Err(Some(JobDecision::Confirmed(ConfirmOutcome::Deferred)))
        );
    }

    #[test]
    fn an_ended_job_keeps_its_decision_only_while_a_start_waits_for_it() {
        let mut scopes = Scopes::default();
        let scope = scope("ended");
        let name = FilesystemSnapshotName::periodic();

        let unwatched = admitted(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, unwatched);
        decide(
            &mut scopes,
            &scope,
            unwatched,
            JobDecision::Confirmed(ConfirmOutcome::Superseded),
        );
        end_job(&mut scopes, &scope, unwatched);
        let without_waiter = (scopes.ended.len(), decision_of(&scopes, &scope, unwatched));

        let watched = admitted(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, watched);
        assert!(wait(&mut scopes, &scope, &name).is_ok());
        assert!(wait(&mut scopes, &scope, &name).is_ok());
        decide(
            &mut scopes,
            &scope,
            watched,
            JobDecision::Confirmed(ConfirmOutcome::Superseded),
        );
        end_job(&mut scopes, &scope, watched);
        let kept = decision_of(&scopes, &scope, watched);
        unwatch(&mut scopes, &scope, unwatched);
        unwatch(&mut scopes, &scope, watched);
        let after_one = scopes.ended.len();
        unwatch(&mut scopes, &scope, watched);

        assert_eq!(without_waiter, (0, Some(JobDecision::Stopped)));
        assert_eq!(
            kept,
            Some(JobDecision::Confirmed(ConfirmOutcome::Superseded))
        );
        assert_eq!(after_one, 1);
        assert!(scopes.ended.is_empty());
    }

    #[test]
    fn an_undecided_job_that_ends_with_a_waiter_keeps_stopped() {
        let mut scopes = Scopes::default();
        let scope = scope("stopped");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, id);
        assert!(wait(&mut scopes, &scope, &name).is_ok());

        end_job(&mut scopes, &scope, id);

        assert_eq!(
            scopes.ended.get(&scope).map(|ended| ended.decision),
            Some(JobDecision::Stopped)
        );
    }

    #[test]
    fn an_unwatch_of_a_live_job_takes_one_waiter_and_a_stale_one_does_nothing() {
        let mut scopes = Scopes::default();
        let scope = scope("unwatch");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, id);
        assert!(wait(&mut scopes, &scope, &name).is_ok());
        assert!(wait(&mut scopes, &scope, &name).is_ok());

        unwatch(&mut scopes, &scope, id + 1);
        let after_stale = scopes.jobs.get(&scope).map(|job| job.waiters);
        unwatch(&mut scopes, &scope, id);

        assert_eq!(after_stale, Some(2));
        assert_eq!(scopes.jobs.get(&scope).map(|job| job.waiters), Some(1));
    }

    #[test]
    fn scope_deletes_count_and_the_last_end_frees_the_scope_and_its_ended_decision() {
        let mut scopes = Scopes::default();
        let scope = scope("deletes");
        let name = FilesystemSnapshotName::periodic();
        let id = admitted(&mut scopes, &scope, &name);
        saving(&mut scopes, &scope, id);
        assert!(wait(&mut scopes, &scope, &name).is_ok());
        let stop = scopes.jobs.get(&scope).map(|job| job.stop.clone());

        let first = forget_scope(&mut scopes, &scope);
        end_job(&mut scopes, &scope, id);
        let second = forget_scope(&mut scopes, &scope);
        scope_deleted(&mut scopes, &scope);
        let after_one = (
            refusal(try_admit(&mut scopes, &scope, &name, true)),
            scopes.ended.len(),
        );
        scope_deleted(&mut scopes, &scope);
        scope_deleted(&mut scopes, &scope);
        let after_all = try_admit(&mut scopes, &scope, &name, true).is_ok();

        assert!(first.is_some_and(|token| Some(&token) == stop.as_ref()));
        assert!(second.is_none());
        assert_eq!(after_one, (Some((SnapshotSkip::ScopeDeleting, None)), 0));
        assert!(after_all);
        assert!(scopes.deleting.is_empty());
    }

    #[test]
    fn only_decisions_ends_and_scope_deletes_wake_the_waiters() {
        assert_eq!(
            [
                Transition::Admit,
                Transition::Saving,
                Transition::Decide,
                Transition::End,
                Transition::StartWait,
                Transition::Unwatch,
                Transition::ForgetScope,
                Transition::ScopeDeleted,
            ]
            .map(wakes),
            [false, false, true, true, false, false, true, true]
        );
    }

    fn storage(retryable: bool) -> SnapshotStoreError {
        SnapshotStoreError::Storage {
            retryable,
            source: anyhow::anyhow!("storage"),
        }
    }

    #[test]
    fn an_existing_own_name_is_checked_with_a_stat_and_other_errors_fail() {
        let info = SnapshotInfo {
            created_at: golem_common::model::Timestamp::from(1),
            files: 1,
            bytes: 2,
        };
        assert!(matches!(save_attempt(Ok(info)), SaveAttempt::Saved(saved) if saved == info));
        assert!(matches!(
            save_attempt(Err(SnapshotStoreError::AlreadyExists)),
            SaveAttempt::StatOwn
        ));
        assert!(matches!(
            save_attempt(Err(SnapshotStoreError::NotFound)),
            SaveAttempt::Failed(SnapshotStoreError::NotFound)
        ));
    }

    #[test]
    fn only_a_retryable_storage_error_within_the_budget_gets_a_delay() {
        let retry = RetryConfig {
            max_attempts: 3,
            min_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
            multiplier: 4.0,
            max_jitter_factor: None,
        };
        assert_eq!(
            [
                retry_delay(&retry, 1, &storage(true)),
                retry_delay(&retry, 2, &storage(true)),
                retry_delay(&retry, 3, &storage(true)),
                retry_delay(&retry, 1, &storage(false)),
                retry_delay(&retry, 1, &SnapshotStoreError::AlreadyExists),
            ],
            [
                Some(Duration::from_secs(2)),
                Some(Duration::from_secs(8)),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn only_a_confirmed_periodic_snapshot_is_retained_and_only_a_superseded_one_deleted() {
        assert_eq!(
            [
                (SnapshotKind::Periodic, ConfirmOutcome::Confirmed),
                (SnapshotKind::Periodic, ConfirmOutcome::Superseded),
                (SnapshotKind::Periodic, ConfirmOutcome::Deferred),
                (SnapshotKind::Update, ConfirmOutcome::Confirmed),
                (SnapshotKind::Update, ConfirmOutcome::Superseded),
                (SnapshotKind::Update, ConfirmOutcome::Deferred),
            ]
            .map(|(kind, outcome)| follow_up(kind, outcome)),
            [
                FollowUp::Retain,
                FollowUp::DeleteOwn,
                FollowUp::Keep,
                FollowUp::Keep,
                FollowUp::DeleteOwn,
                FollowUp::Keep,
            ]
        );
    }

    #[test]
    fn a_start_skips_the_check_after_superseded_or_a_terminal_interrupt_and_limits_it_otherwise() {
        let limits = StoreCheckLimits {
            confirmation_wait: Duration::from_secs(60),
            store_check_limit: Duration::from_secs(5),
        };
        let superseded = Some(JobDecision::Confirmed(ConfirmOutcome::Superseded));
        let deferred = Some(JobDecision::Confirmed(ConfirmOutcome::Deferred));
        assert_eq!(
            [
                store_check(superseded, false, None, limits),
                store_check(None, true, Some(Duration::from_secs(1)), limits),
                store_check(None, false, None, limits),
                store_check(deferred, false, Some(Duration::from_secs(20)), limits),
                store_check(
                    Some(JobDecision::Stopped),
                    false,
                    Some(Duration::from_secs(70)),
                    limits
                ),
            ],
            [
                StoreCheck::Skip,
                StoreCheck::Skip,
                StoreCheck::Stat(Duration::from_secs(5)),
                StoreCheck::Stat(Duration::from_secs(40)),
                StoreCheck::Stat(Duration::ZERO),
            ]
        );
    }

    #[test]
    fn a_manual_update_waits_only_for_a_running_upload() {
        assert_eq!(
            [
                update_admission(Refusal {
                    skip: SnapshotSkip::UploadInFlight,
                    running: Some(4)
                }),
                update_admission(Refusal {
                    skip: SnapshotSkip::ScopeDeleting,
                    running: Some(4)
                }),
                update_admission(Refusal {
                    skip: SnapshotSkip::VolumeUnderPressure,
                    running: Some(4)
                }),
                update_admission(Refusal {
                    skip: SnapshotSkip::Disabled,
                    running: None
                }),
                update_admission(Refusal {
                    skip: SnapshotSkip::UploadInFlight,
                    running: None
                }),
            ],
            [
                UpdateAdmit::WaitForEnd(4),
                UpdateAdmit::Refuse,
                UpdateAdmit::Refuse,
                UpdateAdmit::Refuse,
                UpdateAdmit::Refuse,
            ]
        );
    }

    #[test]
    fn managed_snapshots_bind_only_to_managed_storage() {
        let managed = FilesystemSnapshotsConfig::Managed(Box::new(
            FilesystemSnapshotStoreConfig::new(&"0".repeat(128), Duration::from_secs(30), 4, 3)
                .unwrap(),
        ));
        let disabled = FilesystemSnapshotsConfig::default();

        assert!(matches!(binding(&disabled, false), Ok(Binding::Disabled)));
        assert!(matches!(binding(&disabled, true), Ok(Binding::Disabled)));
        assert!(matches!(binding(&managed, true), Ok(Binding::Managed(_))));
        assert_eq!(
            binding(&managed, false).err(),
            Some("filesystem snapshots require managed XFS storage".to_string())
        );
    }

    #[test]
    fn a_volume_has_room_when_its_free_space_reaches_both_targets() {
        let pressure = FilesystemPressureConfig::default();
        let observed = |bytes_above: bool, objects_above: bool| FilesystemSpace::Observed {
            total_bytes: u64::MAX,
            available_bytes: if bytes_above {
                pressure.target_available_bytes()
            } else {
                pressure.target_available_bytes() - 1
            },
            total_filesystem_objects: u64::MAX,
            available_filesystem_objects: if objects_above {
                pressure.target_available_filesystem_objects()
            } else {
                pressure.target_available_filesystem_objects() - 1
            },
        };
        assert_eq!(
            [
                has_room(Some(&FilesystemSpace::Unlimited), &pressure),
                has_room(Some(&observed(true, true)), &pressure),
                has_room(Some(&observed(false, true)), &pressure),
                has_room(Some(&observed(true, false)), &pressure),
                has_room(None, &pressure),
            ],
            [true, true, false, false, false]
        );
    }
}
