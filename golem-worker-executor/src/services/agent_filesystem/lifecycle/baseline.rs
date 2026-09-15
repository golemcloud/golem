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

use super::initial_files::{
    DeclarationView, Declarations, InitialFileSources, PathLookup, PathReader, RetryPreparation,
    declaration_view, declarations_of, holds_golem_file, install, observe, sandbox_path,
    seed_with_retry, validate_compatible,
};
use super::*;
use crate::sandbox_filesystem::HostPath;
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::ffi::OsStr;
use std::time::Duration;

/// The name of the directory that holds the copied tree in a capture directory.
const TREE_DIRECTORY: &str = "tree";
/// The name of the file that holds the record in a capture directory.
const RECORD_FILE: &str = "record.json";

/// Fills an empty host directory with the contents of a capture directory: `tree/` and the record.
pub(crate) trait RestoreTree: Send {
    /// Fills the empty directory `into`. A failure can leave a part of the contents in `into`.
    fn restore(self, into: &Path) -> impl Future<Output = Result<(), RestoreError>> + Send;
}

impl RestoreTree for Infallible {
    async fn restore(self, _into: &Path) -> Result<(), RestoreError> {
        match self {}
    }
}

/// Why a restore did not fill its directory.
#[derive(Debug)]
pub(crate) struct RestoreError {
    /// Whether a new attempt can succeed without a change.
    #[allow(dead_code)]
    pub(crate) retryable: bool,
    /// The cause of the failure.
    pub(crate) source: anyhow::Error,
}

impl Display for RestoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "failed to restore the agent filesystem baseline: {:#}",
            self.source
        )
    }
}

impl std::error::Error for RestoreError {}

/// A copy of the files of an agent filesystem, with its record, in one host directory.
///
/// The directory holds `tree/` and `record.json`. The capture does not depend on the generation
/// that made it. The directory goes away when the capture is discarded or dropped.
#[allow(dead_code)]
pub(crate) struct FilesystemCapture {
    directory: HostDirectory,
}

#[allow(dead_code)]
impl FilesystemCapture {
    /// The host directory that holds `tree/` and `record.json`.
    pub(crate) fn directory(&self) -> &Path {
        self.directory.path().as_path()
    }

    /// Removes the capture directory with all that is in it.
    pub(crate) async fn discard(self) -> Result<(), FilesystemStorageError> {
        self.directory.discard().await
    }
}

/// Why a capture did not copy a filesystem.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum CaptureError {
    /// The generation is sealed or invalidated.
    Invalidated,
    /// A filesystem call was still open when the wait ended.
    Busy,
    /// The copy or its record failed.
    Sandbox(FilesystemStorageError),
}

impl Display for CaptureError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalidated => formatter.write_str("agent filesystem access is revoked"),
            Self::Busy => formatter.write_str("agent filesystem has an open call"),
            Self::Sandbox(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for CaptureError {}

/// The record of a capture, next to `tree/` in the capture directory.
#[derive(serde::Serialize, serde::Deserialize)]
struct CaptureRecord {
    /// The initial-file declarations of the component revision, in path order.
    initial_files: Box<[InitialAgentFile]>,
    /// The entity-provisioned file declarations, in path order.
    provisioned_files: Box<[InitialAgentFile]>,
    /// The paths of the read-only files whose bytes the tree leaves out, in path order. A
    /// read-only declaration of the record is at each of these paths and gives the content.
    left_out: Box<[Box<Path>]>,
    /// The files with more than one name, as the copy found them.
    link_groups: Box<[LinkGroup]>,
}

impl CaptureRecord {
    /// Reads the record from the capture directory `directory`.
    async fn read(directory: &HostPath) -> Result<Self, Error> {
        let path = directory
            .child(OsStr::new(RECORD_FILE))
            .map_err(Error::Sandbox)?;
        let bytes = tokio::fs::read(path.as_path()).await.map_err(|error| {
            record_error(anyhow::Error::new(error).context("read the filesystem capture record"))
        })?;
        serde_json::from_slice(&bytes).map_err(|error| {
            record_error(anyhow::Error::new(error).context("decode the filesystem capture record"))
        })
    }
}

fn record_error(source: anyhow::Error) -> Error {
    Error::Baseline(Box::new(RestoreError {
        retryable: false,
        source,
    }))
}

/// Copies a resident filesystem into a new host directory, and leaves the filesystem resident.
///
/// Use this only at a boundary. The capture stops new filesystem calls (`begin_transition`),
/// waits for the calls that are open (`wait_for_calls`), copies the tree, and opens the filesystem
/// again (`finish_transition`). The capture directory holds `tree/` and `record.json`. The tree is
/// the whole filesystem minus each read-only initial or entity-provisioned file that has a single
/// name and is Golem's file at its declared path: a regular file with the declared content and
/// without write permission. The tree holds a file with more than one name once. The record gives
/// the paths whose bytes the tree leaves out, the hard-link groups and the declarations.
///
/// The wait for open calls ends at `wait`. A call that is still open then gives `Busy`, and the
/// filesystem opens again at once. A guest that keeps such a call can never be captured, but the
/// capture never blocks it either.
///
/// A call can stay open at a boundary, when no invocation runs. `wasi:io/streams`
/// `output-stream.write` does not block by contract. The guest can call it and return from the
/// invocation without `flush` and without a poll of the stream. On the host,
/// `AgentFileOutputStream::write` (`wasi_filesystem/p2/types.rs`) calls
/// `route_output_stream_chunk`, which calls `route_agent_write`. That takes the generation call
/// lease at once (`lease_call`) and gives a `FilesystemCall` that has not started. The stream
/// keeps the call in `AgentFileOutputState::Waiting` until the guest polls the stream. The lease
/// goes away only when the call completes or is dropped. `wait_for_calls` counts leases, so
/// without a limit the capture waits for a write that nobody drives. Unload does not see this,
/// because it drops the wasm store, with the stream and the lease, before it waits. wasi-libc
/// always uses `blocking-write-and-flush`, so guests built on it do not do this. A hand-written
/// component or a faulty SDK can. The limit turns a stopped agent into a skipped capture. Keep
/// the limit also if the stream later takes its lease at the first poll.
#[allow(dead_code)]
pub(crate) fn capture<Adapter: SandboxFilesystemAdapter>(
    filesystem: &ResidentFilesystem<Adapter>,
    wait: Duration,
) -> impl Future<Output = Result<FilesystemCapture, CaptureError>> + Send + 'static {
    let generation = Arc::clone(
        filesystem
            .generation
            .as_ref()
            .expect("agent filesystem generation already consumed"),
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = complete_capture(&generation, wait).await;
        if let Err(Ok(unobserved)) = sender.send(result)
            && let Err(error) = unobserved.discard().await
        {
            tracing::warn!(error = %error, "Failed to discard an unobserved filesystem capture");
        }
    });
    async move {
        receiver
            .await
            .expect("module-owned filesystem capture stopped unexpectedly")
    }
}

async fn complete_capture<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    wait: Duration,
) -> Result<FilesystemCapture, CaptureError> {
    generation
        .registry
        .begin_transition()
        .map_err(|error| match error {
            AccessError::Transitioning => CaptureError::Busy,
            AccessError::Revoked | AccessError::WrongGeneration | AccessError::NotPermitted => {
                CaptureError::Invalidated
            }
        })?;
    let result = match tokio::time::timeout(wait, generation.registry.wait_for_calls()).await {
        Ok(()) => capture_into_scratch(generation).await,
        Err(_) => Err(CaptureError::Busy),
    };
    generation.registry.finish_transition();
    result
}

/// Copies the tree into a new directory in the scratch directory.
///
/// The read guard of the sandbox is held for the whole copy. The deletion of the filesystem takes
/// the write guard after the calls and nodes drain, and the copy holds no call or node lease, so
/// the guard is what makes the deletion wait for the copy.
async fn capture_into_scratch<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
) -> Result<FilesystemCapture, CaptureError> {
    let sandbox = generation.sandbox.read().await;
    let sandbox = sandbox.as_ref().ok_or(CaptureError::Invalidated)?;
    let state = Arc::clone(&generation.initial_files.lock().unwrap());
    let left_out = left_out_files(sandbox.as_ref(), &state)
        .await
        .map_err(CaptureError::Sandbox)?;
    let directory = HostDirectory::create_in(
        generation.scratch.path(),
        OsStr::new(&uuid::Uuid::new_v4().to_string()),
    )
    .await
    .map_err(CaptureError::Sandbox)?;
    match write_capture(sandbox.as_ref(), &state, left_out, directory.path()).await {
        Ok(()) => Ok(FilesystemCapture { directory }),
        Err(error) => {
            if let Err(cleanup) = directory.discard().await {
                tracing::warn!(error = %cleanup, "Failed to discard a failed filesystem capture");
            }
            Err(CaptureError::Sandbox(error))
        }
    }
}

/// Finds the read-only declared paths that hold Golem's file with a single name. A capture leaves
/// out the bytes of these files. The result is in path order.
async fn left_out_files<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    state: &InitialFileState,
) -> Result<Box<[Box<Path>]>, FilesystemStorageError> {
    let read_only = state
        .declarations()
        .into_iter()
        .filter(|(_, file)| file.permissions == AgentFilePermissions::ReadOnly)
        .collect::<BTreeMap<&Path, &InitialAgentFile>>();
    futures::stream::iter(read_only)
        .map(Ok)
        .try_fold(
            (PathReader::default(), Vec::new()),
            |(reader, mut paths), (path, declared)| async move {
                let (reader, lookup) = reader.read(sandbox, path).await?;
                if let PathLookup::Found(attributes) = lookup
                    && attributes.link_count == 1
                    && holds_golem_file(
                        sandbox,
                        path,
                        declared,
                        state.installed.get(path),
                        &attributes,
                    )
                    .await?
                {
                    paths.push(Box::from(path));
                }
                Ok((reader, paths))
            },
        )
        .await
        .map(|(_, paths)| paths.into_boxed_slice())
}

/// Copies the tree into `directory`, without the bytes of the files at the paths `left_out`, and
/// writes the record next to it.
async fn write_capture<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    state: &InitialFileState,
    left_out: Box<[Box<Path>]>,
    directory: &HostPath,
) -> Result<(), FilesystemStorageError> {
    let tree = directory.child(OsStr::new(TREE_DIRECTORY))?;
    tokio::fs::create_dir(tree.as_path())
        .await
        .map_err(|error| {
            FilesystemStorageError::io("create filesystem capture tree", tree.as_path(), error)
        })?;
    let excluded = Arc::new(TreeExclusions::new(left_out.iter()));
    let link_groups = sandbox
        .copy_contents(SandboxPath::at_root(""), excluded, &tree)
        .await?;
    let record = CaptureRecord {
        initial_files: sorted_declarations(&state.initial),
        provisioned_files: sorted_declarations(&state.provisioned),
        left_out,
        link_groups,
    };
    let record_path = directory.child(OsStr::new(RECORD_FILE))?;
    let bytes = serde_json::to_vec(&record).map_err(|error| {
        FilesystemStorageError::io(
            "encode filesystem capture record",
            record_path.as_path(),
            std::io::Error::other(error),
        )
    })?;
    tokio::fs::write(record_path.as_path(), bytes)
        .await
        .map_err(|error| {
            FilesystemStorageError::io(
                "write filesystem capture record",
                record_path.as_path(),
                error,
            )
        })
}

fn sorted_declarations(declarations: &Declarations) -> Box<[InitialAgentFile]> {
    declarations
        .iter()
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .cloned()
        .collect()
}

/// Puts the baseline of a `Reconstructing` filesystem in place. Reconstructing -> Reconstructing.
///
/// Without a restore, the function seeds the initial files of `prepared` on the empty tree. With a
/// restore, it seeds the restored tree, makes the other names of each hard-link group of its
/// record, and seeds each left-out file of the record as a read-only file with the content of its
/// recorded declaration. The tree then equals the tree at the capture. Then the initial-file rule
/// runs from the declarations of the record to the declarations of `prepared` and the provisioned
/// declarations of the record. With equal declarations, the rule changes nothing. The restore goes
/// into its own directory in the scratch directory, which is discarded at the end. Callers run this
/// once, before replay access is requested. Success enables replay access. Any failure seals the
/// returned filesystem for cleanup.
pub(crate) fn materialize_baseline<
    Adapter: SandboxFilesystemAdapter,
    Restore: RestoreTree + 'static,
>(
    filesystem: ReconstructingFilesystem<Adapter>,
    prepared: PreparedInitialFiles,
    restore: Option<Restore>,
) -> impl Future<Output = Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>>>
+ Send
+ 'static {
    let (generation, stage) = filesystem.into_parts();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    spawn_module_task(async move {
        let result = complete_baseline(generation, stage, prepared, restore).await;
        if let Err(unobserved) = sender.send(result) {
            drop(unobserved);
        }
    });
    async move {
        receiver
            .await
            .expect("module-owned baseline materialization stopped unexpectedly")
    }
}

async fn complete_baseline<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: Arc<FilesystemGeneration<Adapter>>,
    mut stage: Reconstructing,
    prepared: PreparedInitialFiles,
    restore: Option<Restore>,
) -> Result<ReconstructingFilesystem<Adapter>, ReconstructionFailure<Adapter>> {
    if stage.initial_files_materialized || stage.replay_drained {
        return Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source: Error::RuntimeInvalidated,
        });
    }
    let sandbox = generation.sandbox.read().await.as_ref().cloned();
    let result = match (sandbox, restore) {
        (None, _) => Err(Error::RuntimeInvalidated),
        (Some(sandbox), None) => {
            install_initial_files(&generation, sandbox.as_ref(), prepared).await
        }
        (Some(sandbox), Some(restore)) => {
            restore_baseline(&generation, sandbox.as_ref(), prepared, restore).await
        }
    };
    match result {
        Ok(state) => {
            *generation.initial_files.lock().unwrap() = Arc::new(state);
            stage.initial_files_materialized = true;
            generation.registry.enable_replay_access();
            Ok(AgentFilesystem {
                generation: Some(generation),
                stage: Some(stage),
            })
        }
        Err(source) => Err(ReconstructionFailure {
            filesystem: sealed_filesystem(generation, stage.meter),
            source,
        }),
    }
}

/// Seeds the initial files of `prepared` on the empty tree.
async fn install_initial_files<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
) -> Result<InitialFileState, Error> {
    let (initial, sources) = prepared.into_parts();
    let installed = install(
        generation,
        sandbox,
        sources,
        &DeclarationView::new(),
        &HashMap::new(),
        &declaration_view([&initial]),
        &HashMap::new(),
    )
    .await?;
    Ok(InitialFileState {
        initial: Arc::new(initial),
        provisioned: Arc::default(),
        installed,
    })
}

/// Restores into a new scratch directory, puts the baseline in place, and discards the directory.
///
/// A failure to discard the directory is logged and does not change the result. The next startup
/// cleans the scratch directory.
async fn restore_baseline<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
    restore: Restore,
) -> Result<InitialFileState, Error> {
    let directory = HostDirectory::create_in(
        generation.scratch.path(),
        OsStr::new(&uuid::Uuid::new_v4().to_string()),
    )
    .await
    .map_err(Error::Sandbox)?;
    let restored = restore_from(generation, sandbox, prepared, restore, directory.path()).await;
    if let Err(cleanup) = directory.discard().await {
        tracing::warn!(error = %cleanup, "Failed to discard a filesystem restore directory");
    }
    restored
}

/// Restores into `directory` and puts the baseline in place.
///
/// The order is: the restore, the seed of the restored tree, the other names of each hard-link
/// group, a seed of each left-out file with the content of its recorded declaration, and the
/// modification times of the root and of each directory that got a name from a link or a left-out
/// seed, from the same directories under `tree/`. The tree then equals the tree at the capture,
/// except in the times of the left-out files. Then the initial-file rule runs from the
/// declarations of the record to the declarations of `prepared` and the provisioned declarations
/// of the record, as an automatic update runs it.
///
/// A retry of the seed of the restored tree first removes all that is under the root, because a
/// failed seed keeps the entries it made. When the seed fails, what its attempts made stays for the
/// cleanup of the sealed filesystem, as every failure of the baseline does.
async fn restore_from<Adapter: SandboxFilesystemAdapter, Restore: RestoreTree>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    prepared: PreparedInitialFiles,
    restore: Restore,
    directory: &HostPath,
) -> Result<InitialFileState, Error> {
    restore
        .restore(directory.as_path())
        .await
        .map_err(|error| Error::Baseline(Box::new(error)))?;
    let CaptureRecord {
        initial_files,
        provisioned_files,
        left_out,
        link_groups,
    } = CaptureRecord::read(directory).await?;
    let provisioned = declarations_of(
        provisioned_files.into_vec(),
        "restore unique entity-provisioned file declaration",
    )?;
    let recorded = declarations_of(
        initial_files.into_vec(),
        "restore unique initial-file declaration",
    )?;
    let old = declaration_view([&recorded, &provisioned]);
    let captured = captured_declarations(&old, &left_out)?;
    let left_out_sources =
        InitialFileSources::new(Arc::clone(&prepared.loader), prepared.environment_id);
    let (initial, sources) = prepared.into_parts();
    validate_compatible(
        &declaration_view([&provisioned]),
        &declaration_view([&initial]),
    )?;
    let tree = directory
        .child(OsStr::new(TREE_DIRECTORY))
        .map_err(Error::Sandbox)?;
    seed_with_retry(
        generation,
        sandbox,
        SeedEntry {
            source: tree.clone(),
            target: SandboxPath::at_root(""),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        },
        RetryPreparation::EmptyRoot,
    )
    .await?;
    link_other_names(sandbox, &link_groups).await?;
    // The captured declarations describe the restored tree, in which each left-out path is empty.
    // An install from them to the declarations of the record seeds each left-out file there.
    let seeded = install(
        generation,
        sandbox,
        left_out_sources,
        &captured,
        &HashMap::new(),
        &old,
        &HashMap::new(),
    )
    .await?;
    // The seed into the root keeps the time of the root, and a name that a link or a seed adds to
    // a directory changes the time of that directory.
    let changed_directories = std::iter::once(Path::new(""))
        .chain(
            link_groups
                .iter()
                .flat_map(|group| group.others.iter())
                .filter_map(|other| other.parent()),
        )
        .chain(left_out.iter().filter_map(|path| path.parent()))
        .collect::<BTreeSet<&Path>>();
    restore_directory_times(sandbox, &tree, changed_directories).await?;
    let new = declaration_view([&initial, &provisioned]);
    let states = observe(sandbox, &old, &new, &seeded)
        .await
        .map_err(|source| classify_query_error(generation, source))?;
    let installed = install(generation, sandbox, sources, &old, &seeded, &new, &states).await?;
    Ok(InitialFileState {
        initial: Arc::new(initial),
        provisioned: Arc::new(provisioned),
        installed,
    })
}

/// Gives `declarations` without the paths `left_out`. Refuses a left-out path that has no read-only
/// declaration, because a restore cannot seed a file at it.
fn captured_declarations<'a>(
    declarations: &DeclarationView<'a>,
    left_out: &[Box<Path>],
) -> Result<DeclarationView<'a>, Error> {
    let left_out = left_out
        .iter()
        .map(AsRef::as_ref)
        .collect::<HashSet<&Path>>();
    match left_out.iter().find(|path| {
        !declarations
            .get(**path)
            .is_some_and(|file| file.permissions == AgentFilePermissions::ReadOnly)
    }) {
        Some(path) => Err(record_error(anyhow::anyhow!(
            "the filesystem capture record leaves out {} without a read-only declaration",
            path.display()
        ))),
        None => Ok(declarations
            .iter()
            .filter(|(path, _)| !left_out.contains(*path))
            .map(|(path, file)| (*path, *file))
            .collect()),
    }
}

/// Gives each directory in `directories` the modification time of the same directory under
/// `tree`, in path order. The root is the empty path.
async fn restore_directory_times<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    tree: &HostPath,
    directories: BTreeSet<&Path>,
) -> Result<(), Error> {
    futures::stream::iter(directories)
        .map(Ok)
        .try_for_each(|directory| async move {
            let source = tree.as_path().join(directory);
            let modified = tokio::fs::symlink_metadata(&source)
                .await
                .and_then(|metadata| metadata.modified())
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "read the time of a restored directory",
                        &source,
                        error,
                    )
                })?;
            sandbox
                .set_path_times(
                    sandbox_path(directory),
                    SandboxFollow::No,
                    SandboxTimeChanges {
                        accessed: SandboxTimeChange::Keep,
                        modified: SandboxTimeChange::Set(modified),
                    },
                )
                .await
                .map_err(Error::Sandbox)
        })
        .await
}

/// Makes the other names of each group into hard links to its first name.
async fn link_other_names<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    groups: &[LinkGroup],
) -> Result<(), Error> {
    futures::stream::iter(groups)
        .map(Ok)
        .try_for_each(|group| async move {
            futures::stream::iter(group.others.iter())
                .map(Ok)
                .try_for_each(|other| async move {
                    sandbox
                        .hard_link(
                            SandboxPath::at_root(&*group.first),
                            SandboxPath::at_root(&**other),
                        )
                        .await
                        .map_err(Error::Sandbox)
                })
                .await
        })
        .await
}
