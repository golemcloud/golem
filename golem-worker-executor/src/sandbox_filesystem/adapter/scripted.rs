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

use super::namespace::split_namespace_target;
use super::*;
use std::collections::HashMap;
use std::collections::VecDeque;

#[derive(Clone)]
pub(crate) struct ScriptedSandboxFilesystem {
    state: Arc<Mutex<ScriptedState>>,
}

impl Debug for ScriptedSandboxFilesystem {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScriptedSandboxFilesystem")
            .finish_non_exhaustive()
    }
}

pub(crate) struct ScriptedSandboxFilesystemProvisioning {
    state: Arc<Mutex<ScriptedState>>,
}

#[derive(Clone)]
pub(crate) struct ScriptedSandboxFilesystemControl {
    state: Arc<Mutex<ScriptedState>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ScriptedSandboxPath {
    pub(crate) base: ScriptedSandboxPathBase,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScriptedSandboxPathBase {
    Root,
    Directory(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ScriptedSandboxPathCall {
    HardLink {
        source: ScriptedSandboxPath,
        destination: ScriptedSandboxPath,
    },
    Rename {
        source: ScriptedSandboxPath,
        destination: ScriptedSandboxPath,
    },
}

pub(crate) struct ScriptedSandboxFilesystemGate {
    started: Arc<tokio::sync::Semaphore>,
    released: Arc<tokio::sync::Semaphore>,
    completed: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
struct ScriptedGateState {
    started: Arc<tokio::sync::Semaphore>,
    released: Arc<tokio::sync::Semaphore>,
    completed: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct ScriptedState {
    calls: Vec<String>,
    sandbox_path_calls: Vec<ScriptedSandboxPathCall>,
    append_contents: HashMap<NativeFileIdentity, Vec<u8>>,
    gates: HashMap<String, ScriptedGateState>,
    creation: Option<Result<(), FilesystemStorageError>>,
    resolve_namespace_target: VecDeque<Result<ScriptedNamespaceResolution, FilesystemStorageError>>,
    open: VecDeque<Result<SandboxOpened, FilesystemStorageError>>,
    read: VecDeque<Result<Bytes, FilesystemStorageError>>,
    read_directory: VecDeque<Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>>,
    read_link: VecDeque<Result<SandboxSymlinkTarget, FilesystemStorageError>>,
    write: VecDeque<Result<SandboxWriteAttempt, FilesystemStorageError>>,
    get_attributes: VecDeque<Result<SandboxAttributes, FilesystemStorageError>>,
    set_size: VecDeque<Result<(), FilesystemStorageError>>,
    set_times: VecDeque<Result<(), FilesystemStorageError>>,
    create_directory: VecDeque<Result<(), FilesystemStorageError>>,
    create_symlink: VecDeque<Result<(), FilesystemStorageError>>,
    hard_link: VecDeque<Result<(), FilesystemStorageError>>,
    rename: VecDeque<Result<(), FilesystemStorageError>>,
    remove_directory: VecDeque<Result<(), FilesystemStorageError>>,
    unlink_file: VecDeque<Result<(), FilesystemStorageError>>,
    flush: VecDeque<Result<(), FilesystemStorageError>>,
    close: VecDeque<Result<(), FilesystemStorageError>>,
    seed: VecDeque<Result<(), FilesystemStorageError>>,
    observe_allocation: VecDeque<Result<FilesystemAllocation, FilesystemStorageError>>,
    install_limits: VecDeque<Result<InstalledLimits, FilesystemStorageError>>,
    copy_contents: VecDeque<Result<Box<[LinkGroup]>, FilesystemStorageError>>,
    delete_and_verify: VecDeque<Result<(), FilesystemStorageError>>,
}

struct ScriptedNamespaceResolution {
    parent_identity: u64,
    equivalent_name: OsString,
    final_directory_identity: Option<u64>,
    read_only_file: bool,
    followed_read_only_file: bool,
}

impl ScriptedSandboxFilesystemProvisioning {
    pub(crate) fn new() -> (Self, ScriptedSandboxFilesystemControl) {
        let state = Arc::new(Mutex::new(ScriptedState {
            creation: Some(Ok(())),
            ..ScriptedState::default()
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            ScriptedSandboxFilesystemControl { state },
        )
    }
}

impl ScriptedSandboxFilesystemControl {
    pub(crate) fn calls(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .calls
            .clone()
    }

    pub(crate) fn sandbox_path_calls(&self) -> Vec<ScriptedSandboxPathCall> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .clone()
    }

    pub(crate) fn programmed_append_contents(&self, identity: u64) -> Vec<u8> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .append_contents
            .get(&NativeFileIdentity::Scripted(format!(
                "programmed-file-{identity}"
            )))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_creation(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .creation = Some(outcome);
    }

    pub(crate) fn block(&self, operation: impl Into<String>) -> ScriptedSandboxFilesystemGate {
        let operation = operation.into();
        let gate = ScriptedGateState {
            started: Arc::new(tokio::sync::Semaphore::new(0)),
            released: Arc::new(tokio::sync::Semaphore::new(0)),
            completed: Arc::new(tokio::sync::Semaphore::new(0)),
        };
        self.state().gates.insert(operation, gate.clone());
        ScriptedSandboxFilesystemGate {
            started: gate.started,
            released: gate.released,
            completed: gate.completed,
        }
    }

    pub(crate) fn push_open(&self, outcome: Result<SandboxOpened, FilesystemStorageError>) {
        self.state().open.push_back(outcome);
    }

    pub(crate) fn push_namespace_resolution(
        &self,
        parent_identity: u64,
        equivalent_name: impl Into<OsString>,
        final_directory_identity: Option<u64>,
    ) {
        self.state()
            .resolve_namespace_target
            .push_back(Ok(ScriptedNamespaceResolution {
                parent_identity,
                equivalent_name: equivalent_name.into(),
                final_directory_identity,
                read_only_file: false,
                followed_read_only_file: false,
            }));
    }

    /// Programs the next namespace resolution of a target that is not a directory. The flags
    /// tell whether the target, and the object that a final symlink names, is a read-only
    /// file.
    pub(crate) fn push_read_only_resolution(
        &self,
        parent_identity: u64,
        equivalent_name: impl Into<OsString>,
        read_only_file: bool,
        followed_read_only_file: bool,
    ) {
        self.state()
            .resolve_namespace_target
            .push_back(Ok(ScriptedNamespaceResolution {
                parent_identity,
                equivalent_name: equivalent_name.into(),
                final_directory_identity: None,
                read_only_file,
                followed_read_only_file,
            }));
    }

    pub(crate) fn push_namespace_resolution_error(&self, error: FilesystemStorageError) {
        self.state().resolve_namespace_target.push_back(Err(error));
    }

    pub(crate) fn push_read(&self, outcome: Result<Bytes, FilesystemStorageError>) {
        self.state().read.push_back(outcome);
    }

    pub(crate) fn push_read_directory(
        &self,
        outcome: Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>,
    ) {
        self.state().read_directory.push_back(outcome);
    }

    pub(crate) fn push_read_link(
        &self,
        outcome: Result<SandboxSymlinkTarget, FilesystemStorageError>,
    ) {
        self.state().read_link.push_back(outcome);
    }

    pub(crate) fn push_write(&self, outcome: Result<SandboxWriteAttempt, FilesystemStorageError>) {
        self.state().write.push_back(outcome);
    }

    pub(crate) fn push_get_attributes(
        &self,
        outcome: Result<SandboxAttributes, FilesystemStorageError>,
    ) {
        self.state().get_attributes.push_back(outcome);
    }

    pub(crate) fn push_set_size(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().set_size.push_back(outcome);
    }

    pub(crate) fn push_set_times(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().set_times.push_back(outcome);
    }

    pub(crate) fn push_create_directory(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().create_directory.push_back(outcome);
    }

    pub(crate) fn push_create_symlink(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().create_symlink.push_back(outcome);
    }

    pub(crate) fn push_hard_link(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().hard_link.push_back(outcome);
    }

    pub(crate) fn push_rename(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().rename.push_back(outcome);
    }

    pub(crate) fn push_remove_directory(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().remove_directory.push_back(outcome);
    }

    pub(crate) fn push_unlink_file(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().unlink_file.push_back(outcome);
    }

    pub(crate) fn push_flush(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().flush.push_back(outcome);
    }

    pub(crate) fn push_close(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().close.push_back(outcome);
    }

    pub(crate) fn push_seed(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().seed.push_back(outcome);
    }

    pub(crate) fn push_observe_allocation(
        &self,
        outcome: Result<FilesystemAllocation, FilesystemStorageError>,
    ) {
        self.state().observe_allocation.push_back(outcome);
    }

    pub(crate) fn push_install_limits(
        &self,
        outcome: Result<InstalledLimits, FilesystemStorageError>,
    ) {
        self.state().install_limits.push_back(outcome);
    }

    pub(crate) fn push_copy_contents(
        &self,
        outcome: Result<Box<[LinkGroup]>, FilesystemStorageError>,
    ) {
        self.state().copy_contents.push_back(outcome);
    }

    pub(crate) fn push_delete_and_verify(&self, outcome: Result<(), FilesystemStorageError>) {
        self.state().delete_and_verify.push_back(outcome);
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ScriptedState> {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
    }
}

impl ScriptedSandboxFilesystemGate {
    pub(crate) async fn wait_started(&self) {
        self.started
            .acquire()
            .await
            .expect("scripted sandbox filesystem start gate closed")
            .forget();
    }

    pub(crate) fn release(&self) {
        self.released.add_permits(1);
    }

    pub(crate) async fn wait_completed(&self) {
        self.completed
            .acquire()
            .await
            .expect("scripted sandbox filesystem completion gate closed")
            .forget();
    }
}

impl SandboxFilesystemAdapter for ScriptedSandboxFilesystem {
    type Provisioning = ScriptedSandboxFilesystemProvisioning;
    type AllocationReader = ScriptedSandboxFilesystem;

    async fn create_fresh(
        provisioning: Self::Provisioning,
        name: SandboxFilesystemName,
        limits: Option<FilesystemLimits>,
    ) -> Result<Self, FilesystemStorageError> {
        let mut state = provisioning
            .state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned");
        state.calls.push(format!(
            "create_fresh(name={}, limits={limits:?})",
            name.relative_path().display()
        ));
        take_outcome(&mut state.creation, "create_fresh")?;
        drop(state);
        Ok(Self {
            state: provisioning.state,
        })
    }

    fn open(
        &self,
        target: SandboxPath,
        options: SandboxOpenOptions,
    ) -> impl Future<Output = Result<SandboxOpened, FilesystemStorageError>> + Send {
        self.outcome(
            format!("open(target={target:?}, options={options:?})"),
            |state| &mut state.open,
        )
    }

    fn resolve_namespace_target(
        &self,
        target: SandboxPath,
    ) -> impl Future<Output = Result<SandboxResolvedNamespaceTarget, FilesystemStorageError>> + Send
    {
        let state = Arc::clone(&self.state);
        async move {
            let call = format!("resolve_namespace_target(target={target:?})");
            let (gate, programmed) = {
                let mut state = state
                    .lock()
                    .expect("scripted sandbox filesystem lock poisoned");
                state.calls.push(call);
                (
                    state.gates.remove("resolve_namespace_target"),
                    state.resolve_namespace_target.pop_front(),
                )
            };
            if let Some(gate) = gate {
                gate.started.add_permits(1);
                gate.released
                    .acquire()
                    .await
                    .expect("scripted sandbox filesystem release gate closed")
                    .forget();
                gate.completed.add_permits(1);
            }
            match programmed {
                Some(Ok(programmed)) => scripted_namespace_resolution(target, programmed),
                Some(Err(error)) => Err(error),
                None => default_scripted_namespace_resolution(target),
            }
        }
    }

    fn read(
        &self,
        file: &SandboxFile,
        range: SandboxReadRange,
    ) -> impl Future<Output = Result<Bytes, FilesystemStorageError>> + Send {
        self.outcome(format!("read(file={file:?}, range={range:?})"), |state| {
            &mut state.read
        })
    }

    fn read_directory(
        &self,
        directory: &SandboxDirectory,
    ) -> impl Future<Output = Result<Vec<SandboxDirectoryEntry>, FilesystemStorageError>> + Send
    {
        self.outcome(
            format!("read_directory(directory={directory:?})"),
            |state| &mut state.read_directory,
        )
    }

    fn read_link(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<SandboxSymlinkTarget, FilesystemStorageError>> + Send {
        self.outcome(format!("read_link(path={path:?})"), |state| {
            &mut state.read_link
        })
    }

    fn write(
        &self,
        file: &SandboxFile,
        placement: SandboxWritePlacement,
        bytes: Bytes,
    ) -> impl Future<Output = Result<SandboxWriteAttempt, FilesystemStorageError>> + Send {
        let identity = file.append_coordination.identity.clone();
        let appended = bytes.clone();
        let outcome = self.outcome(
            format!("write(file={file:?}, placement={placement:?}, bytes={bytes:?})"),
            |state| &mut state.write,
        );
        let state = Arc::clone(&self.state);
        async move {
            let attempt = outcome.await?;
            if placement == SandboxWritePlacement::Append
                && let Ok(written) = usize::try_from(attempt.written)
                && written <= appended.len()
            {
                state
                    .lock()
                    .expect("scripted sandbox filesystem lock poisoned")
                    .append_contents
                    .entry(identity)
                    .or_default()
                    .extend_from_slice(&appended[..written]);
            }
            Ok(attempt)
        }
    }

    fn get_node_attributes(
        &self,
        node: SandboxNode,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        self.outcome(format!("get_node_attributes(node={node:?})"), |state| {
            &mut state.get_attributes
        })
    }

    fn get_path_attributes(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
    ) -> impl Future<Output = Result<SandboxAttributes, FilesystemStorageError>> + Send {
        self.outcome(
            format!("get_path_attributes(target={target:?}, follow={follow:?})"),
            |state| &mut state.get_attributes,
        )
    }

    fn is_same_open_object(
        &self,
        left: SandboxNode,
        right: SandboxNode,
    ) -> impl Future<Output = Result<bool, FilesystemStorageError>> + Send {
        let same = match (left, right) {
            (SandboxNode::File(left), SandboxNode::File(right)) => {
                match (&left.handle, &right.handle) {
                    (NativeFileHandle::Scripted(left), NativeFileHandle::Scripted(right)) => {
                        left == right
                    }
                    _ => false,
                }
            }
            (SandboxNode::Directory(left), SandboxNode::Directory(right)) => {
                left.coordination_key == right.coordination_key
            }
            _ => false,
        };
        async move { Ok(same) }
    }

    fn set_size(
        &self,
        file: &SandboxFile,
        size: u64,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("set_size(file={file:?}, size={size})"), |state| {
            &mut state.set_size
        })
    }

    fn set_node_times(
        &self,
        node: SandboxNode,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("set_node_times(node={node:?}, times={times:?})"),
            |state| &mut state.set_times,
        )
    }

    fn set_path_times(
        &self,
        target: SandboxPath,
        follow: SandboxFollow,
        times: SandboxTimeChanges,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("set_path_times(target={target:?}, follow={follow:?}, times={times:?})"),
            |state| &mut state.set_times,
        )
    }

    fn create_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("create_directory(path={path:?})"), |state| {
            &mut state.create_directory
        })
    }

    fn create_symlink(
        &self,
        path: SandboxPath,
        target: SandboxSymlinkTarget,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(
            format!("create_symlink(path={path:?}, target={target:?})"),
            |state| &mut state.create_symlink,
        )
    }

    fn hard_link(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .push(ScriptedSandboxPathCall::HardLink {
                source: scripted_sandbox_path(&source),
                destination: scripted_sandbox_path(&destination),
            });
        self.outcome(
            format!("hard_link(source={source:?}, destination={destination:?})"),
            |state| &mut state.hard_link,
        )
    }

    fn rename(
        &self,
        source: SandboxPath,
        destination: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned")
            .sandbox_path_calls
            .push(ScriptedSandboxPathCall::Rename {
                source: scripted_sandbox_path(&source),
                destination: scripted_sandbox_path(&destination),
            });
        self.outcome(
            format!("rename(source={source:?}, destination={destination:?})"),
            |state| &mut state.rename,
        )
    }

    fn remove_directory(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("remove_directory(path={path:?})"), |state| {
            &mut state.remove_directory
        })
    }

    fn unlink_file(
        &self,
        path: SandboxPath,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("unlink_file(path={path:?})"), |state| {
            &mut state.unlink_file
        })
    }

    fn flush(
        &self,
        node: &SandboxNode,
        level: SandboxFlushLevel,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("flush(node={node:?}, level={level:?})"), |state| {
            &mut state.flush
        })
    }

    fn close(
        &self,
        node: SandboxNode,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        self.outcome(format!("close(node={node:?})"), |state| &mut state.close)
    }

    fn seed(
        &self,
        entries: Box<[SeedEntry]>,
    ) -> impl Future<Output = Result<(), FilesystemStorageError>> + Send {
        let entries = entries
            .iter()
            .map(|entry| {
                format!(
                    "{{source={}, target={:?}, access={:?}, placement={:?}}}",
                    entry.source.as_path().display(),
                    entry.target,
                    entry.access,
                    entry.placement
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        self.outcome(format!("seed(entries=[{entries}])"), |state| {
            &mut state.seed
        })
    }

    fn observe_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send {
        self.outcome("observe_allocation()".to_string(), |state| {
            &mut state.observe_allocation
        })
    }

    fn allocation_reader(&self) -> Self::AllocationReader {
        self.clone()
    }

    fn install_limits(
        &self,
        limits: FilesystemLimits,
    ) -> impl Future<Output = Result<InstalledLimits, FilesystemStorageError>> + Send {
        self.outcome(format!("install_limits(limits={limits:?})"), |state| {
            &mut state.install_limits
        })
    }

    fn copy_contents(
        &self,
        source: SandboxPath,
        excluded: Arc<TreeExclusions>,
        target: &HostPath,
    ) -> impl Future<Output = Result<Box<[LinkGroup]>, FilesystemStorageError>> + Send {
        let mut excluded = excluded
            .paths()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        excluded.sort();
        self.outcome(
            format!(
                "copy_contents(source={source:?}, excluded={excluded:?}, target={})",
                target.as_path().display()
            ),
            |state| &mut state.copy_contents,
        )
    }

    async fn delete_and_verify(self) -> Result<(), DeleteError<Self>> {
        match self
            .outcome("delete_and_verify()".to_string(), |state| {
                &mut state.delete_and_verify
            })
            .await
        {
            Ok(()) => Ok(()),
            Err(source) => Err(DeleteError::new(self, source)),
        }
    }
}

impl SandboxFilesystemAllocationReader for ScriptedSandboxFilesystem {
    fn read_allocation(
        &self,
    ) -> impl Future<Output = Result<FilesystemAllocation, FilesystemStorageError>> + Send {
        self.outcome("observe_allocation()".to_string(), |state| {
            &mut state.observe_allocation
        })
    }
}

impl ScriptedSandboxFilesystem {
    fn outcome<T: Send + 'static>(
        &self,
        call: String,
        outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
    ) -> impl Future<Output = Result<T, FilesystemStorageError>> + Send + use<T> {
        scripted_outcome(Arc::clone(&self.state), call, outcomes)
    }
}

async fn scripted_outcome<T: Send + 'static>(
    state: Arc<Mutex<ScriptedState>>,
    call: String,
    outcomes: fn(&mut ScriptedState) -> &mut VecDeque<Result<T, FilesystemStorageError>>,
) -> Result<T, FilesystemStorageError> {
    let operation = call
        .split_once('(')
        .map_or_else(|| call.clone(), |(operation, _)| operation.to_string());
    let (gate, outcome) = {
        let mut state = state
            .lock()
            .expect("scripted sandbox filesystem lock poisoned");
        state.calls.push(call);
        let gate = state.gates.remove(&operation);
        let outcome = outcomes(&mut state).pop_front().unwrap_or_else(|| {
            Err(FilesystemStorageError::verification(
                "consume programmed scripted sandbox filesystem outcome",
                Path::new("<scripted-sandbox-filesystem>"),
            ))
        });
        (gate, outcome)
    };
    if let Some(gate) = gate {
        gate.started.add_permits(1);
        gate.released
            .acquire()
            .await
            .expect("scripted sandbox filesystem release gate closed")
            .forget();
        gate.completed.add_permits(1);
    }
    outcome
}

fn scripted_namespace_resolution(
    target: SandboxPath,
    programmed: ScriptedNamespaceResolution,
) -> Result<SandboxResolvedNamespaceTarget, FilesystemStorageError> {
    let (_, name) = split_namespace_target(&target.path).map_err(|error| {
        FilesystemStorageError::io(
            "resolve scripted sandbox filesystem namespace target",
            Path::new("<scripted-sandbox-filesystem>"),
            error,
        )
    })?;
    let parent_identity = NativeFileIdentity::Scripted(format!(
        "programmed-directory-{}",
        programmed.parent_identity
    ));
    let parent_key = SandboxDirectoryCoordinationKey(parent_identity);
    Ok(SandboxResolvedNamespaceTarget {
        parent: SandboxDirectory::scripted_with_identity(
            programmed.parent_identity,
            format!("programmed-directory-{}", programmed.parent_identity),
        ),
        name: name.into_boxed_os_str(),
        // A programmed canonical label proves equivalence, not byte-exact lookup.
        parent_byte_exact_lookup_proven: false,
        coordination_key: SandboxNamespaceCoordinationKey {
            parent: parent_key,
            name: NativeNameCoordinationKey {
                name: programmed.equivalent_name,
                mode: NativeNameComparisonMode::Exact,
            },
        },
        final_directory_key: programmed.final_directory_identity.map(|identity| {
            SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(format!(
                "programmed-directory-{identity}"
            )))
        }),
        read_only_file: programmed.read_only_file,
        followed_read_only_file: NativeReadOnlyResolution::Resolved(
            programmed.followed_read_only_file,
        ),
    })
}

fn scripted_sandbox_path(path: &SandboxPath) -> ScriptedSandboxPath {
    let base = match &path.base {
        SandboxPathBase::Root => ScriptedSandboxPathBase::Root,
        SandboxPathBase::Directory(directory) => {
            ScriptedSandboxPathBase::Directory(match &directory.handle {
                NativeDirectoryHandle::Scripted(id) => *id,
                NativeDirectoryHandle::Host(_) => {
                    panic!("scripted adapter received a production native directory")
                }
            })
        }
    };
    ScriptedSandboxPath {
        base,
        path: path.path.clone(),
    }
}

pub(super) fn default_scripted_namespace_resolution(
    target: SandboxPath,
) -> Result<SandboxResolvedNamespaceTarget, FilesystemStorageError> {
    let (relative_parent, name) = split_namespace_target(&target.path).map_err(|error| {
        FilesystemStorageError::io(
            "resolve scripted sandbox filesystem namespace target",
            Path::new("<scripted-sandbox-filesystem>"),
            error,
        )
    })?;
    let relative_parent = normalize_scripted_parent(&relative_parent);
    let (base_id, base_identity) = match target.base {
        SandboxPathBase::Directory(directory) => match directory.handle {
            NativeDirectoryHandle::Scripted(id) => (id, directory.coordination_key.0),
            NativeDirectoryHandle::Host(_) => {
                return Err(scripted_handle_error("resolve scripted namespace target"));
            }
        },
        SandboxPathBase::Root => (0, NativeFileIdentity::Scripted("scripted-root".to_string())),
    };
    let parent_identity = if relative_parent.as_os_str().is_empty() {
        base_identity
    } else {
        NativeFileIdentity::Scripted(format!("{base_identity:?}/{}", relative_parent.display()))
    };
    let parent_handle = if relative_parent.as_os_str().is_empty() {
        base_id
    } else {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        parent_identity.hash(&mut hasher);
        hasher.finish()
    };
    let parent_key = SandboxDirectoryCoordinationKey(parent_identity.clone());
    Ok(SandboxResolvedNamespaceTarget {
        parent: SandboxDirectory::scripted_with_identity(
            parent_handle,
            match parent_identity {
                NativeFileIdentity::Scripted(identity) => identity,
                #[allow(unreachable_patterns)]
                _ => unreachable!("scripted namespace parent identity must be scripted"),
            },
        ),
        parent_byte_exact_lookup_proven: true,
        coordination_key: SandboxNamespaceCoordinationKey {
            parent: parent_key,
            name: NativeNameCoordinationKey {
                name: name.clone(),
                mode: NativeNameComparisonMode::Exact,
            },
        },
        name: name.into_boxed_os_str(),
        final_directory_key: None,
        read_only_file: false,
        followed_read_only_file: NativeReadOnlyResolution::Resolved(false),
    })
}

fn normalize_scripted_parent(path: &Path) -> PathBuf {
    path.components()
        .fold(PathBuf::new(), |mut normalized, component| {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::Normal(component) => normalized.push(component),
                std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
                std::path::Component::RootDir => normalized.clear(),
            }
            normalized
        })
}

fn take_outcome(
    outcome: &mut Option<Result<(), FilesystemStorageError>>,
    operation: &'static str,
) -> Result<(), FilesystemStorageError> {
    outcome.take().unwrap_or_else(|| {
        Err(FilesystemStorageError::verification(
            operation,
            Path::new("<scripted-sandbox-filesystem>"),
        ))
    })
}
