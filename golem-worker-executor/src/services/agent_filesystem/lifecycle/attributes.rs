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

use super::*;

/// What a change of the times of an object with the modification time `before_modified` counts,
/// when `origin` asks for it. A snapshot keeps no access time, so only a change of the modification
/// time changes the tree. A given time is a chosen time when a call of the agent asks for it, and a
/// replayed time when it is a time that a stat recorded, put back live or in a replay.
pub(super) fn set_times_change(
    before_modified: Option<std::time::SystemTime>,
    requested: TimeChange,
    origin: TimeOrigin,
) -> Option<Counted> {
    if time_change_satisfied(before_modified, before_modified, requested, None) {
        return None;
    }
    Some(match (requested, origin) {
        (TimeChange::Set(_), TimeOrigin::Call) => Counted::Chosen,
        (TimeChange::Set(_), TimeOrigin::Replay) => Counted::Replayed,
        (TimeChange::Now | TimeChange::Keep, _) => Counted::Fresh,
    })
}

/// Applies timestamp changes, or file size plus timestamp changes, to a target.
///
/// Open targets address the opened object; path targets obey their final-symlink `Follow` setting.
/// The target must be writable and belong to the admitted generation, and size changes require an
/// open file. These checks return `AccessError` immediately; sandbox and capacity errors are deferred.
pub(crate) fn set_attributes<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
    changes: AttributeChanges,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    authorize_attribute_target(&target, changes)?;
    let coordinated_path = match &target {
        Target::Path(target, follow) => Some((target.sandbox.clone(), *follow)),
        Target::Open(_) => None,
    };
    let target = sandbox_target(&generation, target)?;
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Decides)?;
    Ok(FilesystemCall::new(lease, async move {
        match coordinated_path {
            Some((target, follow)) => {
                execute_coordinated_path_attribute_changes(generation, target, follow, changes)
                    .await
            }
            None => execute_attribute_changes(generation, target, changes).await,
        }
    }))
}

/// Puts back the times that a stat recorded on an open or path target, live or in a replay.
///
/// Replay adapters call this with a reconstruction handle after replaying the matching metadata
/// operation. A live stat calls it too, and then the times already match, so nothing changes. Path targets obey `Follow`; `Keep` leaves a timestamp unchanged. Generation and
/// admission errors are immediate, while restoration or invalidation errors are deferred.
pub(crate) fn restore_times<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    target: Target<'_>,
    times: TimeChanges,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    let target = sandbox_target(&generation, target)?;
    let lease = generation_handle.lease_call(&generation.registry, CallEffect::Decides)?;
    Ok(FilesystemCall::new(lease, async move {
        execute_set_times(generation, target, times, TimeOrigin::Replay).await
    }))
}

async fn execute_attribute_changes<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: AttributeTarget,
    changes: AttributeChanges,
) -> Result<(), Error> {
    match changes {
        AttributeChanges::Times(times) => {
            execute_set_times(generation, target, times, TimeOrigin::Call).await
        }
        AttributeChanges::File { size, times } => {
            let AttributeTarget::Open(SandboxHandle::File(file)) = target else {
                return Err(Error::Access(AccessError::WrongGeneration));
            };
            execute_set_size(Arc::clone(&generation), file.clone(), size).await?;
            execute_set_times(
                generation,
                AttributeTarget::Open(SandboxHandle::File(file)),
                times,
                TimeOrigin::Call,
            )
            .await
        }
    }
}

/// Changes the attributes of the object that the entry at `target` names, under the coordination on
/// the entry.
///
/// With `Follow::No` the change goes to the entry itself. The coordination stops an edit of the
/// entry until the change is done. So the check of the permission and the change see one object.
/// With `Follow::Yes` the change goes to the object that a final
/// symlink at the entry names. An edit of other entries can put another object at that name while
/// the change runs, and the coordination on the entry does not stop it. So the function opens the
/// named object, and the check of its permission and the change go through the open descriptor to
/// one object. That descriptor is not registered in the generation; it closes when the function
/// returns.
async fn execute_coordinated_path_attribute_changes<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    follow: Follow,
    changes: AttributeChanges,
) -> Result<(), Error> {
    // The open of the followed object expects the kind that a read gave. A change of the object
    // between the read and the open makes the open fail, and the operation starts again a bounded
    // number of times.
    let mut budget = RetryBudget::new(2);
    loop {
        let Some(CoordinatedTarget {
            coordination: _coordination,
            resolved,
        }) = coordinated_target(
            &generation,
            &target,
            NamespaceCoordinationKind::Observe,
            Some(sandbox_follow(follow)),
        )
        .await?
        else {
            continue;
        };
        let target = match follow {
            Follow::No => AttributeTarget::Path {
                target: resolved.target(),
                follow: SandboxFollow::No,
            },
            Follow::Yes => match open_followed_object(&generation, resolved.target()).await {
                Ok(node) => AttributeTarget::Open(node),
                Err(Error::Sandbox(error))
                    if error.io_kind() == Some(std::io::ErrorKind::InvalidInput)
                        && budget.consume() =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            },
        };
        return execute_attribute_changes(generation, target, changes).await;
    }
}

/// Opens the object that `target` names through a final symlink, for a change of its attributes,
/// and refuses a regular file without write permission. The adapter reports that fact from the
/// object that the open pinned.
///
/// The open expects the kind that a read of the object gives. When the open finds another kind,
/// the object changed between the read and the open, and the open fails with an `InvalidInput`
/// storage error.
async fn open_followed_object<Adapter: SandboxFilesystemAdapter>(
    generation: &Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
) -> Result<SandboxHandle, Error> {
    let read = mutation_attributes(
        generation,
        AttributeTarget::Path {
            target: target.clone(),
            follow: SandboxFollow::Yes,
        },
    )
    .await?;
    let options = OpenOptions::Existing {
        expected: agent_object_kind(read.kind),
        access: AccessMode::ReadAndSetTimes,
        follow: Follow::Yes,
    };
    let opened = execute_open(Arc::clone(generation), target, options).await?;
    // The node is not registered and is used only in this operation, so a refusal drops it, as
    // the end of the operation does.
    refuse_read_only_change(opened.is_read_only_file())?;
    Ok(opened)
}

async fn execute_set_size<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    file: SandboxFile,
    size: u64,
) -> Result<(), Error> {
    let target = AttributeTarget::Open(SandboxHandle::File(file.clone()));
    let before = mutation_attributes(&generation, target.clone()).await?;
    if before.size == size {
        return Ok(());
    }
    generation.registry.record(Counted::Fresh);
    let growing = size > before.size;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.set_size(&file, size).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match mutation_attributes_sandbox(&generation, target.clone()).await {
                Ok(observed) => resize_postcondition_evidence(before.size, observed.size, size),
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        if evidence != EffectEvidence::NoEffect {
            generation.invalidate();
            return Err(Error::RuntimeInvalidated);
        }
        match decide_resize_effect(&generation, &error, evidence, budget, growing).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry
            | EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix
            | EffectDecision::Succeed => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_set_times<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: AttributeTarget,
    times: TimeChanges,
    origin: TimeOrigin,
) -> Result<(), Error> {
    if time_changes_are_noop(times) {
        return Ok(());
    }
    let before = mutation_attributes(&generation, target.clone()).await?;
    if time_changes_satisfied(&before, &before, times, None) {
        return Ok(());
    }
    if let Some(counted) = set_times_change(before.modified, times.modified, origin) {
        generation.registry.record(counted);
    }
    let mut budget = RetryBudget::new(2);
    loop {
        let started = std::time::SystemTime::now();
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            set_sandbox_times(
                sandbox.as_ref(),
                target.clone(),
                sandbox_time_changes(times),
            )
            .await
        };
        let finished = std::time::SystemTime::now();
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match mutation_attributes_sandbox(&generation, target.clone()).await {
                Ok(observed) => {
                    timestamp_postcondition_evidence(&before, &observed, times, (started, finished))
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        if evidence != EffectEvidence::NoEffect {
            generation.invalidate();
            return Err(Error::RuntimeInvalidated);
        }
        match decide_generation_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry
            | EffectDecision::Invalidate
            | EffectDecision::RetryAfterProvenNoEffect
            | EffectDecision::RetryUnwrittenSuffix
            | EffectDecision::Succeed => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn mutation_attributes<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: AttributeTarget,
) -> Result<SandboxAttributes, Error> {
    mutation_attributes_sandbox(generation, target)
        .await
        .map_err(|source| classify_query_error(generation, source))
}

async fn mutation_attributes_sandbox<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: AttributeTarget,
) -> Result<SandboxAttributes, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification_without_path(
            "inspect deleted filesystem attribute postcondition",
        )
    })?;
    read_sandbox_attributes(sandbox.as_ref(), target).await
}

pub(super) async fn read_sandbox_attributes<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    target: AttributeTarget,
) -> Result<SandboxAttributes, FilesystemStorageError> {
    match target {
        AttributeTarget::Open(node) => sandbox.get_handle_attributes(node).await,
        AttributeTarget::Path { target, follow } => {
            sandbox.get_path_attributes(target, follow).await
        }
    }
}

async fn set_sandbox_times<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    target: AttributeTarget,
    times: SandboxTimeChanges,
) -> Result<(), FilesystemStorageError> {
    match target {
        AttributeTarget::Open(node) => sandbox.set_handle_times(node, times).await,
        AttributeTarget::Path { target, follow } => {
            sandbox.set_path_times(target, follow, times).await
        }
    }
}

pub(super) fn resize_postcondition_evidence(
    before: u64,
    observed: u64,
    requested: u64,
) -> EffectEvidence {
    if observed == requested {
        EffectEvidence::DesiredPostconditionSatisfied
    } else if observed == before {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

pub(super) fn timestamp_postcondition_evidence(
    before: &SandboxAttributes,
    observed: &SandboxAttributes,
    requested: TimeChanges,
    now_range: (std::time::SystemTime, std::time::SystemTime),
) -> EffectEvidence {
    if time_changes_satisfied(before, observed, requested, Some(now_range)) {
        EffectEvidence::DesiredPostconditionSatisfied
    } else if observed.accessed == before.accessed && observed.modified == before.modified {
        EffectEvidence::NoEffect
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

fn time_changes_satisfied(
    before: &SandboxAttributes,
    observed: &SandboxAttributes,
    requested: TimeChanges,
    now_range: Option<(std::time::SystemTime, std::time::SystemTime)>,
) -> bool {
    time_change_satisfied(
        before.accessed,
        observed.accessed,
        requested.accessed,
        now_range,
    ) && time_change_satisfied(
        before.modified,
        observed.modified,
        requested.modified,
        now_range,
    )
}

fn time_change_satisfied(
    before: Option<std::time::SystemTime>,
    observed: Option<std::time::SystemTime>,
    requested: TimeChange,
    now_range: Option<(std::time::SystemTime, std::time::SystemTime)>,
) -> bool {
    match requested {
        TimeChange::Keep => observed == before,
        TimeChange::Now => now_range.is_some_and(|(started, finished)| {
            observed.is_some_and(|observed| observed >= started && observed <= finished)
        }),
        TimeChange::Set(requested) => observed == Some(requested),
    }
}

fn time_changes_are_noop(changes: TimeChanges) -> bool {
    changes.accessed == TimeChange::Keep && changes.modified == TimeChange::Keep
}

fn authorize_attribute_target(
    target: &Target<'_>,
    changes: AttributeChanges,
) -> Result<(), AccessError> {
    match target {
        Target::Open(node) if !node_ownership(node).access.can_set_times() => {
            return Err(if node.kind() == ObjectKind::Directory {
                AccessError::ReadOnly
            } else {
                AccessError::NotPermitted
            });
        }
        Target::Path(target, _) => target.require_mutable()?,
        Target::Open(_) => {}
    }
    if matches!(changes, AttributeChanges::File { .. })
        && !matches!(target, &Target::Open(OpenNode::File(_)))
    {
        return Err(AccessError::WrongGeneration);
    }
    Ok(())
}

fn sandbox_time_changes(times: TimeChanges) -> SandboxTimeChanges {
    SandboxTimeChanges {
        accessed: sandbox_time_change(times.accessed),
        modified: sandbox_time_change(times.modified),
    }
}

fn sandbox_time_change(change: TimeChange) -> SandboxTimeChange {
    match change {
        TimeChange::Keep => SandboxTimeChange::Keep,
        TimeChange::Now => SandboxTimeChange::Now,
        TimeChange::Set(time) => SandboxTimeChange::Set(time),
    }
}
