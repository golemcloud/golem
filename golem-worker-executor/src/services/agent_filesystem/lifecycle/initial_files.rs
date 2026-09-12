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
use crate::sandbox_filesystem::HostPath;
use futures::{StreamExt as _, TryStreamExt as _};
use std::collections::BTreeSet;
use std::collections::hash_map::Entry;
use std::time::SystemTime;

/// The declarations of initial files, at their paths relative to the filesystem root.
pub(super) type Declarations = HashMap<Box<Path>, InitialAgentFile>;

/// The number of bytes that one read takes when an install reads the content of a file.
const CONTENT_READ_BYTES: usize = 64 * 1024;

/// The initial files of one generation.
///
/// `initial` holds the declarations of the component revision, and `provisioned` the declarations
/// of entity provisioning. `installed` holds, for read-only paths of the declarations, the file
/// that the lifecycle installed at the path.
#[derive(Clone, Default)]
pub(super) struct InitialFileState {
    pub(super) initial: Declarations,
    pub(super) provisioned: Declarations,
    pub(super) installed: HashMap<Box<Path>, InstalledFile>,
}

impl InitialFileState {
    /// Gives the initial and the provisioned declarations together.
    pub(super) fn declarations(&self) -> Declarations {
        merged_declarations(&self.initial, &self.provisioned)
    }
}

/// A file that the lifecycle installed at a path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InstalledFile {
    object: SandboxObjectId,
    created: Option<SystemTime>,
}

impl InstalledFile {
    /// Records the object that `attributes` describe.
    pub(super) fn of(attributes: &SandboxAttributes) -> Self {
        Self {
            object: attributes.object.clone(),
            created: attributes.created,
        }
    }

    /// Tells whether `attributes` describe this file.
    ///
    /// The object must be a regular file with the same identity. Where the filesystem records
    /// creation times, the creation times must also be equal, so that a new file that got the
    /// identity of a deleted file does not match. Where the filesystem does not record them, the
    /// object must have no write permission.
    pub(super) fn matches(&self, attributes: &SandboxAttributes) -> bool {
        attributes.kind == SandboxObjectKind::File
            && self.object == attributes.object
            && match (self.created, attributes.created) {
                (Some(recorded), Some(observed)) => recorded == observed,
                (None, None) => attributes.read_only,
                (Some(_), None) | (None, Some(_)) => false,
            }
    }
}

/// What an install finds at a declared path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PathState {
    /// Nothing is at the path.
    Absent,
    /// The path holds Golem's file: the file of the old declaration, as the lifecycle installed it.
    Golem,
    /// The path is empty because a capture left out the bytes of Golem's file there.
    LeftOut,
    /// Another object is at the path.
    Other,
}

/// One change of an install.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Step<'a> {
    /// Puts the declared file at the path. `existing` is `Fail` where the path holds nothing, and
    /// `Replace` where it holds Golem's file.
    Seed {
        path: &'a Path,
        file: &'a InitialAgentFile,
        existing: OnExisting,
    },
    /// Removes Golem's file from the path.
    Unlink { path: &'a Path },
}

impl<'a> Step<'a> {
    fn path(&self) -> &'a Path {
        match self {
            Self::Seed { path, .. } | Self::Unlink { path } => path,
        }
    }
}

/// Decides the changes of one install of initial files.
///
/// `old` holds the declarations before the install, and `new` the declarations after it. `state`
/// gives what is at a path.
///
/// A path whose declarations in `old` and `new` are equal keeps what is at it. Two declarations
/// are equal when their content hash, path, permissions and size are equal. A left-out file at
/// such a path is seeded again. Every other path follows one of three rules:
///
/// 1. A path that is read-only in `new` gets the new file if it holds nothing or Golem's file.
///    Anything else at the path is a conflict.
/// 2. A path that is read-write in `new` and in `old` stays as it is. Another path that is
///    read-write in `new` gets the new file if it holds nothing or Golem's file. Anything else at
///    the path stays.
/// 3. A path that `new` does not declare loses Golem's file if `old` declared it read-only.
///    Anything else at the path stays.
///
/// The result gives the steps in path order, or the first path in path order that has a conflict.
pub(super) fn plan<'a>(
    old: &'a Declarations,
    new: &'a Declarations,
    state: impl Fn(&Path) -> PathState,
) -> Result<Box<[Step<'a>]>, &'a Path> {
    old.keys()
        .chain(new.keys())
        .map(AsRef::as_ref)
        .collect::<BTreeSet<&Path>>()
        .into_iter()
        .try_fold(Vec::new(), |mut steps, path| {
            match rule(old.get(path), new.get(path), state(path)) {
                Decision::Keep => {}
                Decision::Seed(file, existing) => steps.push(Step::Seed {
                    path,
                    file,
                    existing,
                }),
                Decision::Unlink => steps.push(Step::Unlink { path }),
                Decision::Conflict => return Err(path),
            }
            Ok(steps)
        })
        .map(Vec::into_boxed_slice)
}

/// What the rule of [`plan`] decides for one path.
#[derive(Debug, Eq, PartialEq)]
enum Decision<'a> {
    Keep,
    Seed(&'a InitialAgentFile, OnExisting),
    Unlink,
    Conflict,
}

/// Applies the rule of [`plan`] to one path.
fn rule<'a>(
    old: Option<&InitialAgentFile>,
    new: Option<&'a InitialAgentFile>,
    state: PathState,
) -> Decision<'a> {
    match (old, new) {
        (Some(old), Some(new)) if old == new => match state {
            PathState::LeftOut => Decision::Seed(new, OnExisting::Fail),
            PathState::Absent | PathState::Golem | PathState::Other => Decision::Keep,
        },
        (_, Some(new)) if new.permissions == AgentFilePermissions::ReadOnly => match state {
            PathState::Absent | PathState::LeftOut => Decision::Seed(new, OnExisting::Fail),
            PathState::Golem => Decision::Seed(new, OnExisting::Replace),
            PathState::Other => Decision::Conflict,
        },
        (Some(old), Some(_)) if old.permissions == AgentFilePermissions::ReadWrite => {
            Decision::Keep
        }
        (_, Some(new)) => match state {
            PathState::Absent | PathState::LeftOut => Decision::Seed(new, OnExisting::Fail),
            PathState::Golem => Decision::Seed(new, OnExisting::Replace),
            PathState::Other => Decision::Keep,
        },
        (Some(old), None)
            if old.permissions == AgentFilePermissions::ReadOnly && state == PathState::Golem =>
        {
            Decision::Unlink
        }
        (_, None) => Decision::Keep,
    }
}

/// Installs the initial files of `new` in place of the files of `old`.
///
/// `installed` holds the files that the lifecycle installed for `old`. `states` gives what is at
/// the paths, as [`plan`] needs it. A path without a state holds nothing. The install loads every
/// source before its first change, so a conflict or a failed load changes nothing. A failure after
/// the first change invalidates the generation. The result gives the files that the lifecycle
/// installed for `new`.
pub(super) async fn install<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    sources: InitialFileSources,
    old: &Declarations,
    installed: &HashMap<Box<Path>, InstalledFile>,
    new: &Declarations,
    states: &HashMap<Box<Path>, PathState>,
) -> Result<HashMap<Box<Path>, InstalledFile>, Error> {
    let steps = plan(old, new, |path| {
        states.get(path).copied().unwrap_or(PathState::Absent)
    })
    .map_err(|path| {
        Error::Sandbox(FilesystemStorageError::verification(
            "install a read-only initial file over other data at",
            path,
        ))
    })?;
    let sources = sources.load(&steps).await?;
    match apply(generation, sandbox, &sources, &steps).await {
        Ok(recorded) => Ok(installed_after(installed, new, &steps, recorded)),
        Err(error) => {
            generation.invalidate();
            Err(error)
        }
    }
}

/// Makes the changes of `steps`, the removals first, and gives the read-only files that it seeded.
async fn apply<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    sources: &InitialFileSources,
    steps: &[Step<'_>],
) -> Result<Vec<(Box<Path>, InstalledFile)>, Error> {
    let (unlinks, seeds) = steps.iter().fold(
        (Vec::new(), Vec::new()),
        |(mut unlinks, mut seeds), step| {
            match *step {
                Step::Unlink { path } => unlinks.push(path),
                Step::Seed {
                    path,
                    file,
                    existing,
                } => seeds.push((path, file, existing)),
            }
            (unlinks, seeds)
        },
    );
    futures::stream::iter(unlinks)
        .map(Ok)
        .try_for_each(|path| async move {
            sandbox
                .unlink_file(SandboxPath::at_root(path))
                .await
                .map_err(Error::Sandbox)
        })
        .await?;
    futures::stream::iter(seeds)
        .map(Ok)
        .try_fold(
            Vec::new(),
            |mut recorded, (path, file, existing)| async move {
                let read_only = file.permissions == AgentFilePermissions::ReadOnly;
                let entry = SeedEntry {
                    source: sources.path(file, path)?,
                    target: SandboxPath::at_root(path),
                    access: if read_only {
                        SeedAccess::ReadOnly
                    } else {
                        SeedAccess::ReadWrite
                    },
                    existing,
                };
                seed_with_retry(generation, sandbox, entry).await?;
                if read_only {
                    let attributes = sandbox
                        .get_path_attributes(SandboxPath::at_root(path), SandboxFollow::No)
                        .await
                        .map_err(Error::Sandbox)?;
                    recorded.push((Box::from(path), InstalledFile::of(&attributes)));
                }
                Ok(recorded)
            },
        )
        .await
}

/// Gives the files that the lifecycle installed after an install of `steps`.
///
/// A path keeps its entry while `new` declares it read-only and no step changed it. `recorded`
/// holds the files that the install seeded. A recorded file removes an entry of the same object at
/// another path, because that path no longer holds the file that the lifecycle installed there.
fn installed_after(
    previous: &HashMap<Box<Path>, InstalledFile>,
    new: &Declarations,
    steps: &[Step<'_>],
    recorded: Vec<(Box<Path>, InstalledFile)>,
) -> HashMap<Box<Path>, InstalledFile> {
    let changed = steps.iter().map(Step::path).collect::<HashSet<&Path>>();
    previous
        .iter()
        .filter(|(path, _)| {
            !changed.contains(path.as_ref())
                && new
                    .get(*path)
                    .is_some_and(|file| file.permissions == AgentFilePermissions::ReadOnly)
        })
        .map(|(path, file)| (path.clone(), file.clone()))
        .chain(recorded)
        .fold(
            (HashMap::new(), HashMap::new()),
            |(mut installed, mut paths), (path, file)| {
                if let Some(older) = paths.insert(file.object.clone(), path.clone()) {
                    installed.remove(&older);
                }
                installed.insert(path, file);
                (installed, paths)
            },
        )
        .0
}

/// Seeds one entry, with the retry, capacity reclaim and classification of initial-file seeding.
pub(super) async fn seed_with_retry<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sandbox: &Adapter,
    entry: SeedEntry,
) -> Result<(), Error> {
    let entry = &entry;
    // An attempt that retries gives no outcome. The stream stops after the first outcome.
    let attempts = futures::stream::unfold(Some(RetryBudget::new(2)), |budget| async move {
        let mut budget = budget?;
        let outcome = match sandbox.seed(Box::new([entry.clone()])).await {
            Ok(()) => Some(Ok(())),
            Err(error) => {
                match decide_write_effect(generation, &error, EffectEvidence::NoEffect, budget)
                    .await
                {
                    EffectDecision::RetryAfterProvenNoEffect if budget.consume() => None,
                    EffectDecision::ReturnFailure(cause) => {
                        Some(Err(classified_error(cause, error)))
                    }
                    EffectDecision::ReclaimCapacityThenRetry => {
                        Some(Err(Error::PhysicalCapacity(error)))
                    }
                    EffectDecision::Invalidate
                    | EffectDecision::RetryAfterProvenNoEffect
                    | EffectDecision::RetryUnwrittenSuffix
                    | EffectDecision::Succeed => {
                        generation.invalidate();
                        Some(Err(Error::RuntimeInvalidated))
                    }
                }
            }
        };
        let next = outcome.is_none().then_some(budget);
        Some((outcome, next))
    });
    let outcomes = attempts.filter_map(std::future::ready);
    std::pin::pin!(outcomes)
        .next()
        .await
        .expect("seed attempts end with an outcome")
}

/// The sources of the files that an install seeds, loaded once for each content hash.
pub(super) struct InitialFileSources {
    loader: Arc<FileLoader>,
    environment_id: EnvironmentId,
    loaded: HashMap<AgentFileContentHash, InitialFileSource>,
}

impl InitialFileSources {
    pub(super) fn new(loader: Arc<FileLoader>, environment_id: EnvironmentId) -> Self {
        Self {
            loader,
            environment_id,
            loaded: HashMap::new(),
        }
    }

    /// Loads the source of each file that `steps` seed, where it is not loaded yet.
    async fn load(self, steps: &[Step<'_>]) -> Result<Self, Error> {
        let seeded = steps
            .iter()
            .filter_map(|step| match *step {
                Step::Seed { path, file, .. } => Some((path, file)),
                Step::Unlink { .. } => None,
            })
            .collect::<Vec<(&Path, &InitialAgentFile)>>();
        futures::stream::iter(seeded)
            .map(Ok)
            .try_fold(self, |mut sources, (path, file)| async move {
                match sources.loaded.get(&file.content_hash) {
                    Some(source) if source.size() == file.size => Ok(sources),
                    Some(_) => Err(Error::Sandbox(FilesystemStorageError::verification(
                        "verify consistent initial-file source size",
                        path,
                    ))),
                    None => {
                        let source = sources
                            .loader
                            .get_source(sources.environment_id, file.content_hash, file.size)
                            .await
                            .map_err(|error| {
                                Error::Sandbox(FilesystemStorageError::io(
                                    "load verified initial-file source",
                                    path,
                                    std::io::Error::other(error),
                                ))
                            })?;
                        sources.loaded.insert(file.content_hash, source);
                        Ok(sources)
                    }
                }
            })
            .await
    }

    /// Gives the host path of the loaded source of `file`, which an install seeds at `path`.
    fn path(&self, file: &InitialAgentFile, path: &Path) -> Result<HostPath, Error> {
        self.loaded
            .get(&file.content_hash)
            .map(|source| source.path().clone())
            .ok_or_else(|| {
                Error::Sandbox(FilesystemStorageError::verification(
                    "find the loaded initial-file source of",
                    path,
                ))
            })
    }
}

impl PreparedInitialFiles {
    /// Gives the declarations and their loaded sources.
    pub(super) fn into_parts(self) -> (Declarations, InitialFileSources) {
        let count = self.files.len();
        let (declarations, loaded) = self.files.into_iter().fold(
            (Declarations::with_capacity(count), HashMap::new()),
            |(mut declarations, mut loaded), file| {
                loaded
                    .entry(file.initial_file.content_hash)
                    .or_insert(file.source);
                declarations.insert(file.target.into_boxed_path(), file.initial_file);
                (declarations, loaded)
            },
        );
        (
            declarations,
            InitialFileSources {
                loader: self.loader,
                environment_id: self.environment_id,
                loaded,
            },
        )
    }
}

/// Finds what is at each path whose declaration differs between `old` and `new`.
///
/// `installed` holds the files that the lifecycle installed for `old`. The function reads the
/// content of a read-write file only where `new` makes the path read-only, because no other rule
/// depends on it.
pub(super) async fn observe<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    old: &Declarations,
    new: &Declarations,
    installed: &HashMap<Box<Path>, InstalledFile>,
) -> Result<HashMap<Box<Path>, PathState>, FilesystemStorageError> {
    let changed = old
        .keys()
        .chain(new.keys())
        .filter(|path| old.get(*path) != new.get(*path))
        .map(AsRef::as_ref)
        .collect::<BTreeSet<&Path>>();
    futures::stream::iter(changed)
        .map(Ok)
        .try_fold(
            (PathReader::default(), HashMap::new()),
            |(reader, mut states), path| async move {
                let (reader, lookup) = reader.read(sandbox, path).await?;
                let state = match lookup {
                    PathLookup::Absent => PathState::Absent,
                    PathLookup::Blocked => PathState::Other,
                    PathLookup::Found(attributes) => {
                        if holds_golem_file(
                            sandbox,
                            path,
                            old.get(path),
                            new.get(path),
                            installed.get(path),
                            &attributes,
                        )
                        .await?
                        {
                            PathState::Golem
                        } else {
                            PathState::Other
                        }
                    }
                };
                states.insert(Box::from(path), state);
                Ok((reader, states))
            },
        )
        .await
        .map(|(_, states)| states)
}

/// Tells whether the object at `path` is the file of the old declaration, as the lifecycle
/// installed it.
async fn holds_golem_file<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    path: &Path,
    old: Option<&InitialAgentFile>,
    new: Option<&InitialAgentFile>,
    installed: Option<&InstalledFile>,
    attributes: &SandboxAttributes,
) -> Result<bool, FilesystemStorageError> {
    match old {
        Some(old) if old.permissions == AgentFilePermissions::ReadOnly => {
            Ok(installed.is_some_and(|installed| installed.matches(attributes)))
        }
        Some(old)
            if new.is_some_and(|new| new.permissions == AgentFilePermissions::ReadOnly)
                && attributes.kind == SandboxObjectKind::File
                && attributes.size == old.size =>
        {
            content_hash(sandbox, path)
                .await
                .map(|hash| hash == *old.content_hash.0.as_blake3_hash())
        }
        Some(_) | None => Ok(false),
    }
}

/// Computes the content hash of the regular file at `path`.
async fn content_hash<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    path: &Path,
) -> Result<blake3::Hash, FilesystemStorageError> {
    let node = sandbox
        .open(
            SandboxPath::at_root(path),
            SandboxOpenOptions::Existing {
                expected: SandboxObjectKind::File,
                access: SandboxAccessMode::Read,
                follow: SandboxFollow::No,
            },
        )
        .await?
        .into_node();
    let hashed = match &node {
        SandboxNode::File(file) => futures::stream::try_unfold(0u64, |offset| async move {
            let bytes = sandbox
                .read(
                    file,
                    SandboxReadRange {
                        offset,
                        length: CONTENT_READ_BYTES,
                    },
                )
                .await?;
            let next = offset + bytes.len() as u64;
            Ok::<_, FilesystemStorageError>((!bytes.is_empty()).then_some((bytes, next)))
        })
        .try_fold(blake3::Hasher::new(), |mut hasher, bytes| async move {
            hasher.update(&bytes);
            Ok(hasher)
        })
        .await
        .map(|hasher| hasher.finalize()),
        SandboxNode::Directory(_) => Err(FilesystemStorageError::verification(
            "hash the content of an initial file at",
            path,
        )),
    };
    let closed = sandbox.close(node).await;
    let hash = hashed?;
    closed.map(|()| hash)
}

/// What a read of a path found, without following a symlink anywhere on the path.
pub(super) enum PathLookup {
    /// Nothing is at the path, or a directory above it is missing.
    Absent,
    /// An object above the path is not a directory.
    Blocked,
    /// An object is at the path.
    Found(SandboxAttributes),
}

/// Reads paths of a sandbox for one operation, and reads each directory above them once.
#[derive(Default)]
pub(super) struct PathReader {
    directories: HashSet<Box<Path>>,
}

impl PathReader {
    /// Reads what is at the root-relative `path`, without following a symlink anywhere on the
    /// path, and gives the reader back.
    pub(super) async fn read<Adapter: SandboxFilesystemAdapter>(
        self,
        sandbox: &Adapter,
        path: &Path,
    ) -> Result<(Self, PathLookup), FilesystemStorageError> {
        let ancestors = path
            .ancestors()
            .skip(1)
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .collect::<Vec<_>>();
        let (reader, stopped) = futures::stream::iter(ancestors.into_iter().rev())
            .map(Ok)
            .try_fold((self, None), |(mut reader, stopped), ancestor| async move {
                if stopped.is_some() || reader.directories.contains(ancestor) {
                    return Ok((reader, stopped));
                }
                match read_path(sandbox, ancestor).await? {
                    Some(attributes) if attributes.kind == SandboxObjectKind::Directory => {
                        reader.directories.insert(Box::from(ancestor));
                        Ok((reader, None))
                    }
                    Some(_) => Ok((reader, Some(PathLookup::Blocked))),
                    None => Ok((reader, Some(PathLookup::Absent))),
                }
            })
            .await?;
        match stopped {
            Some(lookup) => Ok((reader, lookup)),
            None => read_path(sandbox, path).await.map(|attributes| {
                (
                    reader,
                    attributes.map_or(PathLookup::Absent, PathLookup::Found),
                )
            }),
        }
    }
}

/// Reads the attributes of the object at the root-relative `path` without following a final
/// symlink, or gives `None` when nothing is at the path.
async fn read_path<Adapter: SandboxFilesystemAdapter>(
    sandbox: &Adapter,
    path: &Path,
) -> Result<Option<SandboxAttributes>, FilesystemStorageError> {
    match sandbox
        .get_path_attributes(SandboxPath::at_root(path), SandboxFollow::No)
        .await
    {
        Ok(attributes) => Ok(Some(attributes)),
        Err(error) if error.io_kind() == Some(std::io::ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Applies the initial files of a new component revision to a generation.
pub(super) async fn update<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sources: InitialFileSources,
    files: Vec<InitialAgentFile>,
) -> Result<(), Error> {
    let _update = generation.initial_file_updates.lock().await;
    let initial = declarations_of(files, "materialize unique initial-file update target")?;
    let state = generation.initial_files.lock().unwrap().clone();
    validate_compatible(&initial, &state.provisioned)?;
    let new = merged_declarations(&initial, &state.provisioned);
    let installed = install_resident(generation, sources, &state, &new).await?;
    *generation.initial_files.lock().unwrap() = InitialFileState {
        initial,
        provisioned: state.provisioned,
        installed,
    };
    Ok(())
}

/// Adds entity-provisioned files to the initial files of a generation.
pub(super) async fn provision<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sources: InitialFileSources,
    files: Vec<InitialAgentFile>,
) -> Result<(), Error> {
    let _update = generation.initial_file_updates.lock().await;
    let requested = declarations_of(files, "materialize unique entity-provisioned file target")?;
    let state = generation.initial_files.lock().unwrap().clone();
    validate_compatible(&requested, &state.declarations())?;
    let provisioned = merged_declarations(&state.provisioned, &requested);
    if provisioned == state.provisioned {
        return Ok(());
    }
    let new = merged_declarations(&state.initial, &provisioned);
    let installed = install_resident(generation, sources, &state, &new).await?;
    *generation.initial_files.lock().unwrap() = InitialFileState {
        initial: state.initial,
        provisioned,
        installed,
    };
    Ok(())
}

/// Installs `new` over the files of `state` in a generation that the agent uses.
async fn install_resident<Adapter: SandboxFilesystemAdapter>(
    generation: &FilesystemGeneration<Adapter>,
    sources: InitialFileSources,
    state: &InitialFileState,
    new: &Declarations,
) -> Result<HashMap<Box<Path>, InstalledFile>, Error> {
    let sandbox = generation
        .sandbox
        .read()
        .await
        .as_ref()
        .cloned()
        .ok_or(Error::RuntimeInvalidated)?;
    let old = state.declarations();
    let states = observe(sandbox.as_ref(), &old, new, &state.installed)
        .await
        .map_err(|source| classify_query_error(generation, source))?;
    install(
        generation,
        sandbox.as_ref(),
        sources,
        &old,
        &state.installed,
        new,
        &states,
    )
    .await
}

/// Makes declarations from `files`, and refuses two files at one path.
pub(super) fn declarations_of(
    files: Vec<InitialAgentFile>,
    duplicate_operation: &'static str,
) -> Result<Declarations, Error> {
    let count = files.len();
    files.into_iter().try_fold(
        Declarations::with_capacity(count),
        |mut declarations, file| {
            let path = PathBuf::from(file.path.to_rel_string()).into_boxed_path();
            match declarations.entry(path) {
                Entry::Occupied(entry) => Err(Error::Sandbox(
                    FilesystemStorageError::verification(duplicate_operation, entry.key()),
                )),
                Entry::Vacant(entry) => {
                    entry.insert(file);
                    Ok(declarations)
                }
            }
        },
    )
}

/// Refuses a declaration in `requested` that describes another file than `existing` at its path.
pub(super) fn validate_compatible(
    requested: &Declarations,
    existing: &Declarations,
) -> Result<(), Error> {
    requested
        .iter()
        .find(|(path, file)| {
            existing
                .get(*path)
                .is_some_and(|existing| existing != *file)
        })
        .map_or(Ok(()), |(path, _)| {
            Err(Error::Sandbox(FilesystemStorageError::verification(
                "resolve conflicting owner filesystem provision declarations at",
                path,
            )))
        })
}

/// Gives the declarations of `first` and `second` together.
pub(super) fn merged_declarations(first: &Declarations, second: &Declarations) -> Declarations {
    first
        .iter()
        .chain(second)
        .map(|(path, file)| (path.clone(), file.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::component::AgentFilePath;
    use test_r::test;

    fn declaration(permissions: AgentFilePermissions, size: u64) -> InitialAgentFile {
        InitialAgentFile {
            content_hash: AgentFileContentHash(golem_common::model::diff::Hash::empty()),
            path: AgentFilePath::from_abs_str("/file").unwrap(),
            permissions,
            size,
        }
    }

    fn read_only(size: u64) -> InitialAgentFile {
        declaration(AgentFilePermissions::ReadOnly, size)
    }

    fn read_write(size: u64) -> InitialAgentFile {
        declaration(AgentFilePermissions::ReadWrite, size)
    }

    const STATES: [PathState; 4] = [
        PathState::Absent,
        PathState::Golem,
        PathState::LeftOut,
        PathState::Other,
    ];

    #[test]
    fn rule_decides_each_branch_of_the_initial_file_rule() {
        use Decision::{Conflict, Keep, Unlink};
        use OnExisting::{Fail, Replace};
        use PathState::{Absent, Golem, LeftOut, Other};

        let (ro, ro_changed, rw, rw_changed) =
            (read_only(1), read_only(2), read_write(1), read_write(2));
        // A name, the old and the new declaration, the state of the path, and the decision.
        type RuleCase<'a> = (
            &'a str,
            Option<&'a InitialAgentFile>,
            Option<&'a InitialAgentFile>,
            PathState,
            Decision<'a>,
        );
        let cases: Vec<RuleCase> = vec![
            (
                "equal read-only, absent",
                Some(&ro),
                Some(&ro),
                Absent,
                Keep,
            ),
            ("equal read-only, golem", Some(&ro), Some(&ro), Golem, Keep),
            (
                "equal read-only, left out",
                Some(&ro),
                Some(&ro),
                LeftOut,
                Decision::Seed(&ro, Fail),
            ),
            ("equal read-only, other", Some(&ro), Some(&ro), Other, Keep),
            (
                "equal read-write, absent",
                Some(&rw),
                Some(&rw),
                Absent,
                Keep,
            ),
            ("equal read-write, other", Some(&rw), Some(&rw), Other, Keep),
            (
                "new read-only, absent",
                None,
                Some(&ro),
                Absent,
                Decision::Seed(&ro, Fail),
            ),
            ("new read-only, other", None, Some(&ro), Other, Conflict),
            (
                "changed read-only, absent",
                Some(&ro),
                Some(&ro_changed),
                Absent,
                Decision::Seed(&ro_changed, Fail),
            ),
            (
                "changed read-only, left out",
                Some(&ro),
                Some(&ro_changed),
                LeftOut,
                Decision::Seed(&ro_changed, Fail),
            ),
            (
                "changed read-only, golem",
                Some(&ro),
                Some(&ro_changed),
                Golem,
                Decision::Seed(&ro_changed, Replace),
            ),
            (
                "changed read-only, other",
                Some(&ro),
                Some(&ro_changed),
                Other,
                Conflict,
            ),
            (
                "read-write to read-only, absent",
                Some(&rw),
                Some(&ro),
                Absent,
                Decision::Seed(&ro, Fail),
            ),
            (
                "read-write to read-only, golem",
                Some(&rw),
                Some(&ro),
                Golem,
                Decision::Seed(&ro, Replace),
            ),
            (
                "read-write to read-only, other",
                Some(&rw),
                Some(&ro),
                Other,
                Conflict,
            ),
            (
                "changed read-write, absent",
                Some(&rw),
                Some(&rw_changed),
                Absent,
                Keep,
            ),
            (
                "changed read-write, golem",
                Some(&rw),
                Some(&rw_changed),
                Golem,
                Keep,
            ),
            (
                "changed read-write, other",
                Some(&rw),
                Some(&rw_changed),
                Other,
                Keep,
            ),
            (
                "new read-write, absent",
                None,
                Some(&rw),
                Absent,
                Decision::Seed(&rw, Fail),
            ),
            ("new read-write, other", None, Some(&rw), Other, Keep),
            (
                "read-only to read-write, absent",
                Some(&ro),
                Some(&rw),
                Absent,
                Decision::Seed(&rw, Fail),
            ),
            (
                "read-only to read-write, left out",
                Some(&ro),
                Some(&rw),
                LeftOut,
                Decision::Seed(&rw, Fail),
            ),
            (
                "read-only to read-write, golem",
                Some(&ro),
                Some(&rw),
                Golem,
                Decision::Seed(&rw, Replace),
            ),
            (
                "read-only to read-write, other",
                Some(&ro),
                Some(&rw),
                Other,
                Keep,
            ),
            ("dropped read-only, golem", Some(&ro), None, Golem, Unlink),
            ("dropped read-only, absent", Some(&ro), None, Absent, Keep),
            (
                "dropped read-only, left out",
                Some(&ro),
                None,
                LeftOut,
                Keep,
            ),
            ("dropped read-only, other", Some(&ro), None, Other, Keep),
            ("dropped read-write, golem", Some(&rw), None, Golem, Keep),
            ("dropped read-write, other", Some(&rw), None, Other, Keep),
        ];

        cases
            .into_iter()
            .for_each(|(name, old, new, state, expected)| {
                assert_eq!(rule(old, new, state), expected, "{name}");
            });
    }

    #[test]
    fn rule_replaces_or_removes_only_golem_files() {
        let files = [read_only(1), read_only(2), read_write(1), read_write(2)];
        let options = std::iter::once(None)
            .chain(files.iter().map(Some))
            .collect::<Vec<_>>();
        options.iter().for_each(|old| {
            options.iter().for_each(|new| {
                STATES.into_iter().for_each(|state| {
                    let decision = rule(*old, *new, state);
                    let allowed = match &decision {
                        Decision::Seed(_, OnExisting::Replace) | Decision::Unlink => {
                            state == PathState::Golem
                        }
                        Decision::Seed(_, OnExisting::Fail) => {
                            matches!(state, PathState::Absent | PathState::LeftOut)
                        }
                        Decision::Keep | Decision::Conflict => true,
                    };
                    assert!(allowed, "{old:?} -> {new:?} at {state:?} gave {decision:?}");
                });
            });
        });
    }

    #[test]
    fn plan_gives_steps_in_path_order_and_the_first_conflict() {
        let at = |path: &str, file: InitialAgentFile| (Box::<Path>::from(Path::new(path)), file);
        let old = Declarations::from([
            at("b", read_only(1)),
            at("d", read_only(1)),
            at("e", read_write(1)),
        ]);
        let new = Declarations::from([
            at("a", read_write(1)),
            at("b", read_only(2)),
            at("c", read_only(1)),
            at("e", read_write(1)),
        ]);
        let states = HashMap::from([
            (Path::new("b"), PathState::Golem),
            (Path::new("d"), PathState::Golem),
        ]);

        let steps = plan(&old, &new, |path| {
            states.get(path).copied().unwrap_or(PathState::Absent)
        })
        .unwrap();

        assert_eq!(
            steps.as_ref(),
            [
                Step::Seed {
                    path: Path::new("a"),
                    file: &new[Path::new("a")],
                    existing: OnExisting::Fail
                },
                Step::Seed {
                    path: Path::new("b"),
                    file: &new[Path::new("b")],
                    existing: OnExisting::Replace
                },
                Step::Seed {
                    path: Path::new("c"),
                    file: &new[Path::new("c")],
                    existing: OnExisting::Fail
                },
                Step::Unlink {
                    path: Path::new("d")
                },
            ]
        );
        let conflict = plan(&old, &new, |path| match path.to_str() {
            Some("b") | Some("c") => PathState::Other,
            _ => PathState::Absent,
        })
        .unwrap_err();
        assert_eq!(conflict, Path::new("b"));
    }

    #[test]
    fn installed_after_keeps_unchanged_read_only_paths_and_drops_an_older_path_of_one_object() {
        let file = |object| InstalledFile {
            object: SandboxObjectId::scripted(object),
            created: None,
        };
        let previous = HashMap::from([
            (Box::<Path>::from(Path::new("kept")), file(1)),
            (Box::<Path>::from(Path::new("reused")), file(2)),
            (Box::<Path>::from(Path::new("dropped")), file(3)),
            (Box::<Path>::from(Path::new("made-writable")), file(4)),
        ]);
        let new = Declarations::from([
            (Box::<Path>::from(Path::new("kept")), read_only(1)),
            (Box::<Path>::from(Path::new("reused")), read_only(1)),
            (Box::<Path>::from(Path::new("made-writable")), read_write(1)),
            (Box::<Path>::from(Path::new("seeded")), read_only(1)),
        ]);
        let seeded = new.get(Path::new("seeded")).unwrap();
        let steps = [Step::Seed {
            path: Path::new("seeded"),
            file: seeded,
            existing: OnExisting::Fail,
        }];

        let installed = installed_after(
            &previous,
            &new,
            &steps,
            vec![(Box::from(Path::new("seeded")), file(2))],
        );

        assert_eq!(
            installed,
            HashMap::from([
                (Box::<Path>::from(Path::new("kept")), file(1)),
                (Box::<Path>::from(Path::new("seeded")), file(2)),
            ])
        );
    }

    #[test]
    fn an_installed_file_matches_its_object_and_creation_time_or_no_write_permission() {
        let attributes = |object, created, read_only| SandboxAttributes {
            kind: SandboxObjectKind::File,
            link_count: 1,
            size: 0,
            accessed: None,
            modified: None,
            created,
            read_only,
            object: SandboxObjectId::scripted(object),
        };
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let later = time + std::time::Duration::from_secs(1);
        let with_time = InstalledFile::of(&attributes(1, Some(time), false));
        let without_time = InstalledFile::of(&attributes(1, None, true));

        assert!(with_time.matches(&attributes(1, Some(time), false)));
        assert!(!with_time.matches(&attributes(1, Some(later), false)));
        assert!(!with_time.matches(&attributes(2, Some(time), false)));
        assert!(!with_time.matches(&attributes(1, None, true)));
        assert!(without_time.matches(&attributes(1, None, true)));
        assert!(!without_time.matches(&attributes(1, None, false)));
        assert!(!without_time.matches(&attributes(1, Some(time), true)));
        assert!(!with_time.matches(&SandboxAttributes {
            kind: SandboxObjectKind::Directory,
            ..attributes(1, Some(time), false)
        }));
    }
}
