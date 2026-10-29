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

impl NamespaceCoordinator {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(NamespaceCoordinatorState {
                next_id: 0,
                active: Vec::new(),
            }),
            changed: tokio::sync::Notify::new(),
        }
    }

    pub(super) async fn coordinate(
        self: &Arc<Self>,
        kind: NamespaceCoordinationKind,
        namespace: Vec<SandboxNameComparisonKey>,
    ) -> NamespaceCoordination {
        let namespace = deduplicate(namespace);
        loop {
            let notified = self.changed.notified();
            let admitted = {
                let mut state = self.state.lock().unwrap();
                let keys = NamespaceCoordinationKeys {
                    namespace: namespace.clone(),
                    directories: Vec::new(),
                };
                if state
                    .active
                    .iter()
                    .any(|active| coordination_conflicts(kind, &keys, active))
                {
                    None
                } else {
                    let id = state.next_id;
                    state.next_id = state.next_id.wrapping_add(1);
                    state.active.push(ActiveNamespaceEdit {
                        id,
                        kind,
                        keys,
                        directories_installed: false,
                    });
                    Some(id)
                }
            };
            if let Some(id) = admitted {
                return NamespaceCoordination {
                    coordinator: Arc::clone(self),
                    id,
                    active: true,
                };
            }
            notified.await;
        }
    }
}

impl NamespaceCoordination {
    pub(super) fn extend(&mut self, directories: Vec<SandboxDirectoryId>) -> bool {
        let directories = deduplicate(directories);
        let mut state = self.coordinator.state.lock().unwrap();
        let index = state
            .active
            .iter()
            .position(|active| active.id == self.id)
            .expect("active namespace coordination lease missing");
        let mut extended = state.active[index].keys.clone();
        extended.directories = directories;
        let kind = state.active[index].kind;
        let blocked = state.active.iter().any(|active| {
            active.id != self.id
                && active.directories_installed
                && coordination_conflicts(kind, &extended, active)
        });
        let result = if blocked {
            state.active.swap_remove(index);
            self.active = false;
            false
        } else {
            state.active[index].keys = extended;
            state.active[index].directories_installed = true;
            true
        };
        drop(state);
        self.coordinator.changed.notify_waiters();
        result
    }
}

impl Drop for NamespaceCoordination {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.coordinator.state.lock().unwrap();
        let index = state
            .active
            .iter()
            .position(|active| active.id == self.id)
            .expect("active namespace coordination lease missing");
        state.active.swap_remove(index);
        drop(state);
        self.coordinator.changed.notify_waiters();
    }
}

fn coordination_conflicts(
    kind: NamespaceCoordinationKind,
    keys: &NamespaceCoordinationKeys,
    active: &ActiveNamespaceEdit,
) -> bool {
    if kind == NamespaceCoordinationKind::Observe
        && active.kind == NamespaceCoordinationKind::Observe
    {
        return false;
    }
    keys.namespace.iter().any(|left| {
        active
            .keys
            .namespace
            .iter()
            .any(|right| left.may_conflict_with(right))
            || active.keys.directories.iter().any(|right| left == right)
    }) || keys.directories.iter().any(|left| {
        active.keys.namespace.iter().any(|right| left == right)
            || active.keys.directories.iter().any(|right| left == right)
    })
}

fn deduplicate<T: Eq>(values: Vec<T>) -> Vec<T> {
    values.into_iter().fold(Vec::new(), |mut unique, value| {
        if !unique.contains(&value) {
            unique.push(value);
        }
        unique
    })
}

/// Applies one generation-local namespace edit.
///
/// Targets are interpreted relative to their root or open directory. Every source and destination
/// must belong to the admitted generation and permit writes. Invalid lifecycle, generation, or
/// access state fails immediately; sandbox, quota, capacity, and uncertain-effect failures are
/// returned by the coordinated deferred call.
pub(crate) fn edit_namespace<Adapter: SandboxFilesystemAdapter>(
    generation_handle: &FilesystemGenerationHandle<Adapter>,
    edit: NamespaceEdit,
) -> Result<FilesystemCall<()>, AccessError> {
    let generation = admit(generation_handle)?;
    validate_namespace_generation(&generation, &edit)?;
    authorize_namespace_edit(&edit)?;
    let lease = generation_handle.lease_call(
        &generation.registry,
        match edit {
            NamespaceEdit::Insert { .. } | NamespaceEdit::Remove { .. } => CallEffect::Changes,
            NamespaceEdit::Link { .. } | NamespaceEdit::Move { .. } => CallEffect::ChoosesTimes,
        },
    )?;
    Ok(FilesystemCall::new(lease, async move {
        execute_namespace_edit(generation, edit).await
    }))
}

pub(super) async fn resolve_path_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
) -> Result<SandboxResolvedPathTarget, Error> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
    sandbox
        .resolve_path_target(target)
        .await
        .map_err(|source| classify_query_error(generation, source))
}

/// Coordinates the entry at `target` with `kind`, and gives a resolution of the entry.
///
/// `change` gives the follow of an operation that changes the contents or the times of the object
/// at the entry. The entry is then resolved again under the coordination, and the change is
/// authorized from that resolution: an edit of the entry can complete between the first
/// resolution and the coordination, so the first resolution can be stale. Without a change, the
/// first resolution is given. The result is `None` when the coordination could not be installed,
/// or when the key of the entry changed between the two resolutions; the caller starts again.
pub(super) async fn coordinated_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxPath,
    kind: NamespaceCoordinationKind,
    change: Option<SandboxFollow>,
) -> Result<Option<CoordinatedTarget>, Error> {
    let resolved = resolve_path_target(generation, target.clone()).await?;
    let key = resolved.name_comparison_key();
    let mut coordination = generation
        .namespace
        .coordinate(kind, vec![key.clone()])
        .await;
    if !coordination.extend(Vec::new()) {
        return Ok(None);
    }
    let Some(follow) = change else {
        return Ok(Some(CoordinatedTarget {
            coordination,
            resolved,
        }));
    };
    let resolved = resolve_path_target(generation, target.clone()).await?;
    if resolved.name_comparison_key() != key {
        return Ok(None);
    }
    authorize_writable_target(generation, &resolved, follow)?;
    Ok(Some(CoordinatedTarget {
        coordination,
        resolved,
    }))
}

/// Refuses a change to the contents or times of a regular file without write permission.
///
/// The check reads the permission of the file itself. It therefore follows the file after a
/// rename, a hard link, or a move of a directory above it.
fn authorize_writable_target<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: &SandboxResolvedPathTarget,
    follow: SandboxFollow,
) -> Result<(), Error> {
    match target.is_read_only_file(follow) {
        Ok(read_only) => refuse_read_only_change(read_only),
        Err(source) => Err(classify_query_error(generation, source)),
    }
}

/// Refuses a change to the contents or times of a regular file without write permission.
/// `read_only` tells whether the object is such a file.
pub(super) fn refuse_read_only_change(read_only: bool) -> Result<(), Error> {
    if read_only {
        Err(Error::Access(AccessError::NotPermitted))
    } else {
        Ok(())
    }
}

async fn execute_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    edit: NamespaceEdit,
) -> Result<(), Error> {
    loop {
        let resolved = resolve_namespace_edit(&generation, &edit).await?;
        let mut coordination = generation
            .namespace
            .coordinate(
                NamespaceCoordinationKind::Edit,
                resolved.coordination_keys(),
            )
            .await;
        let directory_keys = refresh_namespace_directory_keys(&generation, &resolved).await?;
        if !coordination.extend(directory_keys) {
            continue;
        }
        return execute_resolved_namespace_edit(generation, resolved).await;
    }
}

impl ResolvedNamespaceEdit {
    pub(super) fn coordination_keys(&self) -> Vec<SandboxNameComparisonKey> {
        match self {
            Self::Insert { destination, .. } => vec![destination.name_comparison_key()],
            Self::Link {
                source,
                destination,
            }
            | Self::Move {
                source,
                destination,
            } => vec![
                source.name_comparison_key(),
                destination.name_comparison_key(),
            ],
            Self::Remove { target, .. } => vec![target.name_comparison_key()],
        }
    }
}

async fn resolve_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    edit: &NamespaceEdit,
) -> Result<ResolvedNamespaceEdit, Error> {
    Ok(match edit {
        NamespaceEdit::Insert {
            destination,
            object,
        } => ResolvedNamespaceEdit::Insert {
            destination: resolve_path_target(generation, destination.sandbox.clone()).await?,
            object: object.clone(),
        },
        NamespaceEdit::Link {
            source,
            destination,
        } => ResolvedNamespaceEdit::Link {
            source: resolve_path_target(generation, source.sandbox.clone()).await?,
            destination: resolve_path_target(generation, destination.sandbox.clone()).await?,
        },
        NamespaceEdit::Move {
            source,
            destination,
        } => ResolvedNamespaceEdit::Move {
            source: resolve_path_target(generation, source.sandbox.clone()).await?,
            destination: resolve_path_target(generation, destination.sandbox.clone()).await?,
        },
        NamespaceEdit::Remove { target, expected } => ResolvedNamespaceEdit::Remove {
            target: resolve_path_target(generation, target.sandbox.clone()).await?,
            expected: *expected,
        },
    })
}

async fn refresh_namespace_directory_keys<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    edit: &ResolvedNamespaceEdit,
) -> Result<Vec<SandboxDirectoryId>, Error> {
    let targets = match edit {
        ResolvedNamespaceEdit::Move {
            source,
            destination,
        } => vec![source, destination],
        ResolvedNamespaceEdit::Remove { target, .. } => vec![target],
        ResolvedNamespaceEdit::Insert { .. } | ResolvedNamespaceEdit::Link { .. } => Vec::new(),
    };
    let mut keys = Vec::new();
    for target in targets {
        if let Some(key) = resolve_path_target(generation, target.target())
            .await?
            .target_directory_id()
        {
            keys.push(key);
        }
    }
    Ok(keys)
}

async fn execute_resolved_namespace_edit<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    edit: ResolvedNamespaceEdit,
) -> Result<(), Error> {
    match edit {
        ResolvedNamespaceEdit::Insert {
            destination,
            object,
        } => match object {
            NewObject::Directory => {
                execute_create_directory(generation, destination.target()).await
            }
            NewObject::Symlink(target) => {
                execute_create_symlink(
                    generation,
                    destination.target(),
                    SandboxSymlinkTarget(target.0),
                )
                .await
            }
        },
        ResolvedNamespaceEdit::Link {
            source,
            destination,
        } => execute_hard_link(generation, source.target(), destination.target()).await,
        ResolvedNamespaceEdit::Move {
            source,
            destination,
        } => execute_rename(generation, source.target(), destination.target()).await,
        ResolvedNamespaceEdit::Remove { target, expected } => {
            execute_remove(generation, target.target(), expected).await
        }
    }
}

async fn execute_create_directory<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
) -> Result<(), Error> {
    let before = path_kind_postcondition(&generation, target.clone(), SandboxObjectKind::Directory)
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.create_directory(target.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match path_kind_postcondition(&generation, target.clone(), SandboxObjectKind::Directory)
                .await
            {
                Ok(after) => insert_postcondition_evidence(&before, &after),
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_create_symlink<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    desired: SandboxSymlinkTarget,
) -> Result<(), Error> {
    let before = symlink_postcondition(&generation, target.clone(), &desired)
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox
                .create_symlink(target.clone(), desired.clone())
                .await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match symlink_postcondition(&generation, target.clone(), &desired).await {
                Ok(after) => insert_postcondition_evidence(&before, &after),
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_hard_link<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    source: SandboxPath,
    destination: SandboxPath,
) -> Result<(), Error> {
    let destination_before = namespace_path_state(&generation, destination.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.hard_link(source.clone(), destination.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if hard_link_source_proof_needed(
            error.io_kind(),
            error.is_sandbox_escape(),
            error.cleanup_failed(),
        ) {
            match hard_link_source_outcome(namespace_path_state(&generation, source.clone()).await)
            {
                Ok(Some(error)) => return Err(Error::Access(error)),
                Ok(None) => {}
                Err(probe_error) => {
                    return Err(classify_query_error(&generation, probe_error));
                }
            }
        }
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match namespace_path_state(&generation, destination.clone()).await {
                Ok(destination_after) => {
                    hard_link_postcondition_evidence(&destination_before, &destination_after)
                }
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_rename<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    source: SandboxPath,
    destination: SandboxPath,
) -> Result<(), Error> {
    let source_before = namespace_path_state(&generation, source.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            sandbox.rename(source.clone(), destination.clone()).await
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            let source_after = namespace_path_state(&generation, source.clone()).await;
            let destination_after = namespace_path_state(&generation, destination.clone()).await;
            match (source_after, destination_after) {
                (Ok(source_after), Ok(destination_after)) => {
                    move_postcondition_evidence(&source_before, &source_after, &destination_after)
                }
                _ => EffectEvidence::Unknown {
                    known_completed_prefix: 0,
                },
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_write_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReclaimCapacityThenRetry if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

async fn execute_remove<Adapter: SandboxFilesystemAdapter>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    target: SandboxPath,
    expected: ObjectKind,
) -> Result<(), Error> {
    let before = namespace_path_state(&generation, target.clone())
        .await
        .map_err(|source| classify_query_error(&generation, source))?;
    match &before {
        NamespacePathState::Absent => {}
        NamespacePathState::Present(attributes)
            if !remove_kind_matches(expected, attributes.kind) =>
        {
            return Err(Error::Sandbox(namespace_kind_mismatch_error(
                expected,
                attributes.kind,
            )));
        }
        NamespacePathState::Present(_) => {}
    }
    let mut budget = RetryBudget::new(2);
    loop {
        let result = {
            let sandbox = generation.sandbox.read().await;
            let sandbox = sandbox.as_ref().ok_or(Error::RuntimeInvalidated)?;
            match expected {
                ObjectKind::Directory => sandbox.remove_directory(target.clone()).await,
                ObjectKind::File | ObjectKind::Symlink => sandbox.unlink_file(target.clone()).await,
            }
        };
        let error = match result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let evidence = if error_proves_no_effect(&error) {
            EffectEvidence::NoEffect
        } else {
            match namespace_path_state(&generation, target.clone()).await {
                Ok(after) => remove_postcondition_evidence(&before, &after, expected),
                Err(_) => mutation_failure_evidence(&error),
            }
        };
        if evidence == EffectEvidence::DesiredPostconditionSatisfied {
            return Ok(());
        }
        match decide_generation_effect(&generation, &error, evidence, budget).await {
            EffectDecision::RetryAfterProvenNoEffect if budget.consume() => continue,
            EffectDecision::ReturnFailure(cause) => return Err(classified_error(cause, error)),
            EffectDecision::ReclaimCapacityThenRetry => {
                return Err(classified_error(FailureCause::PhysicalCapacity, error));
            }
            _ => {
                generation.invalidate();
                return Err(Error::RuntimeInvalidated);
            }
        }
    }
}

fn remove_kind_matches(expected: ObjectKind, actual: SandboxObjectKind) -> bool {
    match expected {
        ObjectKind::File => matches!(actual, SandboxObjectKind::File | SandboxObjectKind::Symlink),
        ObjectKind::Directory => actual == SandboxObjectKind::Directory,
        ObjectKind::Symlink => actual == SandboxObjectKind::Symlink,
    }
}

async fn path_kind_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
    expected: SandboxObjectKind,
) -> Result<InsertPostcondition, FilesystemStorageError> {
    let path = namespace_path_state(generation, target).await?;
    let desired = matches!(
        &path,
        NamespacePathState::Present(attributes) if attributes.kind == expected
    );
    Ok(InsertPostcondition { path, desired })
}

async fn symlink_postcondition<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
    expected: &SandboxSymlinkTarget,
) -> Result<InsertPostcondition, FilesystemStorageError> {
    let path_state = namespace_path_state(generation, target.clone()).await?;
    match &path_state {
        NamespacePathState::Absent => {
            return Ok(InsertPostcondition {
                path: path_state,
                desired: false,
            });
        }
        NamespacePathState::Present(attributes)
            if attributes.kind != SandboxObjectKind::Symlink =>
        {
            return Ok(InsertPostcondition {
                path: path_state,
                desired: false,
            });
        }
        NamespacePathState::Present(_) => {}
    }
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification_without_path(
            "inspect deleted filesystem symlink postcondition",
        )
    })?;
    let desired = sandbox
        .read_link(target)
        .await
        .map(|observed| observed == *expected)?;
    Ok(InsertPostcondition {
        path: path_state,
        desired,
    })
}

fn hard_link_source_proof_needed(
    kind: Option<std::io::ErrorKind>,
    sandbox_escape: bool,
    cleanup_failed: bool,
) -> bool {
    matches!(
        kind,
        Some(std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AlreadyExists)
    ) && !sandbox_escape
        && !cleanup_failed
}

fn hard_link_source_outcome(
    proof: Result<NamespacePathState, FilesystemStorageError>,
) -> Result<Option<AccessError>, FilesystemStorageError> {
    match proof {
        Ok(NamespacePathState::Present(attributes))
            if attributes.kind == SandboxObjectKind::Directory =>
        {
            Ok(Some(AccessError::NotPermitted))
        }
        Err(probe_error) if invalidates_generation(&probe_error) => Err(probe_error),
        _ => Ok(None),
    }
}

pub(super) fn insert_postcondition_evidence(
    before: &InsertPostcondition,
    after: &InsertPostcondition,
) -> EffectEvidence {
    if before == after {
        EffectEvidence::NoEffect
    } else if matches!(before.path, NamespacePathState::Absent) && after.desired {
        EffectEvidence::DesiredPostconditionSatisfied
    } else {
        EffectEvidence::Unknown {
            known_completed_prefix: 0,
        }
    }
}

pub(super) fn hard_link_postcondition_evidence(
    before: &NamespacePathState,
    after: &NamespacePathState,
) -> EffectEvidence {
    match (before, after) {
        (NamespacePathState::Absent, NamespacePathState::Absent)
        | (NamespacePathState::Present(_), NamespacePathState::Present(_)) => {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

pub(super) fn remove_postcondition_evidence(
    before: &NamespacePathState,
    after: &NamespacePathState,
    expected: ObjectKind,
) -> EffectEvidence {
    match (before, after) {
        (NamespacePathState::Present(attributes), NamespacePathState::Absent)
            if attributes.kind == sandbox_object_kind(expected) =>
        {
            EffectEvidence::DesiredPostconditionSatisfied
        }
        (NamespacePathState::Absent, NamespacePathState::Absent) => EffectEvidence::NoEffect,
        (NamespacePathState::Present(before), NamespacePathState::Present(after))
            if before.kind == sandbox_object_kind(expected)
                && after.kind == sandbox_object_kind(expected) =>
        {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

async fn namespace_path_state<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    target: SandboxPath,
) -> Result<NamespacePathState, FilesystemStorageError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or_else(|| {
        FilesystemStorageError::verification_without_path(
            "inspect deleted filesystem namespace postcondition",
        )
    })?;
    match sandbox.get_path_attributes(target, SandboxFollow::No).await {
        Ok(attributes) => Ok(NamespacePathState::Present(attributes)),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => {
            Ok(NamespacePathState::Absent)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn move_postcondition_evidence(
    source_before: &NamespacePathState,
    source_after: &NamespacePathState,
    destination_after: &NamespacePathState,
) -> EffectEvidence {
    match (source_before, source_after, destination_after) {
        (
            NamespacePathState::Present(_),
            NamespacePathState::Absent,
            NamespacePathState::Present(_),
        ) => EffectEvidence::DesiredPostconditionSatisfied,
        (NamespacePathState::Absent, _, _)
        | (NamespacePathState::Present(_), NamespacePathState::Present(_), _) => {
            EffectEvidence::NoEffect
        }
        _ => EffectEvidence::Unknown {
            known_completed_prefix: 0,
        },
    }
}

fn namespace_kind_mismatch_error(
    expected: ObjectKind,
    observed: SandboxObjectKind,
) -> FilesystemStorageError {
    let kind = if observed == SandboxObjectKind::Directory {
        std::io::ErrorKind::IsADirectory
    } else if expected == ObjectKind::Directory {
        std::io::ErrorKind::NotADirectory
    } else {
        std::io::ErrorKind::InvalidInput
    };
    FilesystemStorageError::io_without_path(
        "validate namespace object kind",
        std::io::Error::new(
            kind,
            format!("expected {expected:?}, observed {observed:?}"),
        ),
    )
}

fn validate_namespace_generation<Adapter: SandboxFilesystemAdapter>(
    generation: &Arc<FilesystemGeneration<Adapter>>,
    edit: &NamespaceEdit,
) -> Result<(), AccessError> {
    match edit {
        NamespaceEdit::Insert { destination, .. }
        | NamespaceEdit::Remove {
            target: destination,
            ..
        } => validate_path_generation(generation, destination),
        NamespaceEdit::Link {
            source,
            destination,
        }
        | NamespaceEdit::Move {
            source,
            destination,
        } => {
            validate_path_generation(generation, source)?;
            validate_path_generation(generation, destination)
        }
    }
}

fn authorize_namespace_edit(edit: &NamespaceEdit) -> Result<(), AccessError> {
    let writable = |target: &PathTarget| target.access.can_write();
    let permitted = match edit {
        NamespaceEdit::Insert { destination, .. } => writable(destination),
        NamespaceEdit::Link {
            source,
            destination,
        }
        | NamespaceEdit::Move {
            source,
            destination,
        } => writable(source) && writable(destination),
        NamespaceEdit::Remove { target, .. } => writable(target),
    };
    if permitted {
        Ok(())
    } else {
        Err(AccessError::ReadOnly)
    }
}
