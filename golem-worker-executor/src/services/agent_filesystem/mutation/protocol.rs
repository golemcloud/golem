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
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NativeFilesystemError {
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
    message: String,
}

impl NativeFilesystemError {
    fn capture(error: &std::io::Error) -> Self {
        Self {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
            message: error.to_string(),
        }
    }

    pub(crate) fn kind(&self) -> std::io::ErrorKind {
        self.kind
    }

    pub(crate) fn raw_os_error(&self) -> Option<i32> {
        self.raw_os_error
    }

    pub(crate) fn into_io_error(self) -> std::io::Error {
        self.raw_os_error.map_or_else(
            || std::io::Error::new(self.kind, self.message),
            std::io::Error::from_raw_os_error,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AgentFilesystemMutationError {
    Guest(NativeMutationGuestError),
    Native {
        error: NativeFilesystemError,
        completed: u64,
    },
    QuotaExhausted {
        error: NativeFilesystemError,
        completed: u64,
    },
    InsufficientSpace {
        error: NativeFilesystemError,
        completed: u64,
    },
    Cancelled {
        completed: u64,
    },
    RuntimeInvalidated {
        error: Option<NativeFilesystemError>,
        completed: Option<u64>,
    },
}

pub(crate) type AgentFilesystemMutationResult = Result<u64, AgentFilesystemMutationError>;
pub(crate) type AgentFilesystemOperationResult<T = ()> = Result<T, AgentFilesystemMutationError>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentFilesystemWriteMode {
    Position(u64),
    Append,
}

#[derive(Clone)]
pub(crate) struct AgentFilesystemMutations {
    runtime: AgentFilesystemRuntime,
}

macro_rules! update_admission {
    ($($name:ident),+ $(,)?) => {
        $(pub(crate) struct $name {
            effect: Arc<AgentFilesystemUpdateEffectLease>,
        })+
    };
}

macro_rules! effect_admission {
    ($($name:ident),+ $(,)?) => {
        $(pub(crate) struct $name {
            effect: Arc<AgentFilesystemEffectLease>,
        })+
    };
}

update_admission!(
    AdmittedFilesystemResize,
    AdmittedFilesystemDescriptorTimes,
    AdmittedFilesystemPathTimes,
    AdmittedFilesystemMutatingOpen,
);

effect_admission!(
    AdmittedFilesystemSync,
    AdmittedFilesystemCreateDirectory,
    AdmittedFilesystemHardLink,
    AdmittedFilesystemRename,
    AdmittedFilesystemRemoveDirectory,
    AdmittedFilesystemSymlink,
    AdmittedFilesystemUnlinkFile,
);

pub(crate) struct PreparedFilesystemResize {
    native: Arc<dyn FilesystemResize>,
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    before: PathState,
    size: u64,
}

pub(crate) struct PolicyCheckedFilesystemResize {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    descriptor: Descriptor,
    size: u64,
}

pub(crate) struct PolicyCheckedFilesystemDescriptorTimes {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    descriptor: Descriptor,
}

pub(crate) struct ValidatedFilesystemDescriptorTimes {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    descriptor: Descriptor,
}

pub(crate) struct PreparedFilesystemDescriptorTimes {
    validated: ValidatedFilesystemDescriptorTimes,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
    requested_accessed: RequestedTime,
    requested_modified: RequestedTime,
}

pub(crate) struct PolicyCheckedFilesystemPathTimes {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    descriptor: Descriptor,
    path: String,
    follow: bool,
}

pub(crate) struct ValidatedFilesystemPathTimes {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    directory: Dir,
    path: String,
    follow: bool,
}

pub(crate) struct PreparedFilesystemPathTimes {
    validated: ValidatedFilesystemPathTimes,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
    requested_accessed: RequestedTime,
    requested_modified: RequestedTime,
}

pub(crate) struct PreparedFilesystemNamespaceMutation {
    effect: Arc<AgentFilesystemEffectLease>,
    mutation: NamespaceMutation,
}

pub(crate) struct PreparedFilesystemMutatingOpen {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    directory: Dir,
    path: String,
    options: NativeOpenOptions,
    unsupported_sync_flags: bool,
}

pub(crate) struct PolicyCheckedFilesystemMutatingOpen {
    effect: Arc<AgentFilesystemUpdateEffectLease>,
    descriptor: Option<Descriptor>,
    path: String,
    follow: bool,
}

pub(crate) struct PolicyCheckedFilesystemCreateDirectory {
    effect: Arc<AgentFilesystemEffectLease>,
    descriptor: Descriptor,
    path: String,
}

pub(crate) struct PolicyCheckedFilesystemRemoveDirectory {
    effect: Arc<AgentFilesystemEffectLease>,
    descriptor: Descriptor,
    path: String,
}

pub(crate) struct PolicyCheckedFilesystemSymlink {
    effect: Arc<AgentFilesystemEffectLease>,
    descriptor: Descriptor,
    target: String,
    path: String,
}

pub(crate) struct PolicyCheckedFilesystemUnlinkFile {
    effect: Arc<AgentFilesystemEffectLease>,
    descriptor: Descriptor,
    path: String,
}

pub(crate) struct SourceCheckedFilesystemHardLink {
    effect: Arc<AgentFilesystemEffectLease>,
    source: Descriptor,
    source_path: String,
    source_follow: bool,
}

pub(crate) struct DestinationCheckedFilesystemHardLink {
    source_checked: SourceCheckedFilesystemHardLink,
    destination: Descriptor,
    destination_path: String,
}

pub(crate) struct PathsCheckedFilesystemHardLink {
    destination_checked: DestinationCheckedFilesystemHardLink,
}

pub(crate) struct SourceCheckedFilesystemRename {
    effect: Arc<AgentFilesystemEffectLease>,
    source: Descriptor,
    source_path: String,
}

pub(crate) struct DestinationCheckedFilesystemRename {
    source_checked: SourceCheckedFilesystemRename,
    destination: Descriptor,
    destination_path: String,
}

pub(crate) struct PathsCheckedFilesystemRename {
    destination_checked: DestinationCheckedFilesystemRename,
}

impl AgentFilesystemMutations {
    pub(super) fn new(runtime: AgentFilesystemRuntime) -> Self {
        Self { runtime }
    }

    pub(crate) fn writer(
        &self,
        file: File,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        AgentFilesystemWriter::new(
            self.clone(),
            Arc::new(NativeFilesystemWriter { file }),
            mode,
            WriteCompletion::Fill,
        )
    }

    pub(crate) fn positioned_write(
        &self,
        file: File,
        offset: u64,
        contents: Bytes,
    ) -> Result<AgentFilesystemWriteCompletion, AgentFilesystemMutationError> {
        self.positioned_write_with_native(
            Arc::new(NativeFilesystemWriter { file }),
            offset,
            contents,
        )
    }

    fn positioned_write_with_native(
        &self,
        native: Arc<dyn FilesystemWriter>,
        offset: u64,
        contents: Bytes,
    ) -> Result<AgentFilesystemWriteCompletion, AgentFilesystemMutationError> {
        AgentFilesystemWriter::new(
            self.clone(),
            native,
            AgentFilesystemWriteMode::Position(offset),
            WriteCompletion::FirstSuccess,
        )
        .admit(contents)
        .map(|write| write.execute(tokio_util::sync::CancellationToken::new()))
    }

    pub(crate) async fn admit_sync(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemSync> {
        self.begin_effect()
            .await
            .map(|effect| AdmittedFilesystemSync { effect })
    }

    pub(crate) async fn admit_resize(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemResize> {
        self.begin_update_effect()
            .await
            .map(|effect| AdmittedFilesystemResize { effect })
    }

    pub(crate) async fn admit_descriptor_times(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemDescriptorTimes> {
        self.begin_update_effect()
            .await
            .map(|effect| AdmittedFilesystemDescriptorTimes { effect })
    }

    pub(crate) async fn admit_path_times(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemPathTimes> {
        self.begin_update_effect()
            .await
            .map(|effect| AdmittedFilesystemPathTimes { effect })
    }

    pub(crate) async fn admit_mutating_open(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemMutatingOpen> {
        self.begin_update_effect()
            .await
            .map(|effect| AdmittedFilesystemMutatingOpen { effect })
    }

    pub(crate) async fn admit_create_directory(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemCreateDirectory> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemCreateDirectory { effect })
    }

    pub(crate) async fn admit_hard_link(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemHardLink> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemHardLink { effect })
    }

    pub(crate) async fn admit_rename(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemRename> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemRename { effect })
    }

    pub(crate) async fn admit_remove_directory(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemRemoveDirectory> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemRemoveDirectory { effect })
    }

    pub(crate) async fn admit_symlink(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemSymlink> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemSymlink { effect })
    }

    pub(crate) async fn admit_unlink_file(
        &self,
    ) -> AgentFilesystemOperationResult<AdmittedFilesystemUnlinkFile> {
        self.begin_namespace_effect()
            .await
            .map(|effect| AdmittedFilesystemUnlinkFile { effect })
    }

    pub(crate) fn check_resize_policy(
        &self,
        admitted: AdmittedFilesystemResize,
        descriptor: Descriptor,
        size: u64,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemResize> {
        self.ensure_descriptor_writable(&descriptor)?;
        Ok(PolicyCheckedFilesystemResize {
            effect: admitted.effect,
            descriptor,
            size,
        })
    }

    pub(crate) async fn prepare_resize(
        &self,
        checked: PolicyCheckedFilesystemResize,
        file: File,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemResize> {
        self.ensure_file_matches_descriptor(&checked.descriptor, &file)?;
        validate_resize(&file).map_err(AgentFilesystemMutationError::Guest)?;
        self.prepare_resize_with_native(
            Arc::new(NativeFilesystemResize { file }),
            checked.effect,
            checked.size,
        )
        .await
    }

    pub(crate) async fn resize(
        &self,
        prepared: PreparedFilesystemResize,
    ) -> AgentFilesystemMutationResult {
        self.resize_with_prepared_native(prepared).await
    }

    pub(crate) async fn sync(
        &self,
        admitted: AdmittedFilesystemSync,
        descriptor: Descriptor,
        data_only: bool,
    ) -> AgentFilesystemOperationResult {
        match run_blocking_filesystem_mutation(admitted.effect, move || {
            sync_descriptor(&descriptor, data_only)
        })
        .await
        {
            Ok(()) => Ok(()),
            Err(error) => match self
                .resolve_non_prefix_failure(
                    FilesystemPressureOperation::Metadata,
                    error,
                    MutationPostcondition::Unknown,
                    FILESYSTEM_MUTATION_MAX_ATTEMPTS,
                    Instant::now(),
                )
                .await
            {
                NonPrefixResolution::Error(error) => Err(error),
                NonPrefixResolution::Retry | NonPrefixResolution::Success => {
                    unreachable!("sync failure has an unknown effect")
                }
            },
        }
    }

    pub(crate) fn check_descriptor_times_policy(
        &self,
        admitted: AdmittedFilesystemDescriptorTimes,
        descriptor: Descriptor,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemDescriptorTimes> {
        self.ensure_descriptor_writable(&descriptor)?;
        Ok(PolicyCheckedFilesystemDescriptorTimes {
            effect: admitted.effect,
            descriptor,
        })
    }

    pub(crate) fn prepare_descriptor_times(
        &self,
        checked: PolicyCheckedFilesystemDescriptorTimes,
        descriptor: Descriptor,
    ) -> AgentFilesystemOperationResult<ValidatedFilesystemDescriptorTimes> {
        self.ensure_descriptor_matches(&checked.descriptor, &descriptor)?;
        validate_descriptor_times(&descriptor).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(ValidatedFilesystemDescriptorTimes {
            effect: checked.effect,
            descriptor,
        })
    }

    pub(crate) fn bind_descriptor_times(
        &self,
        validated: ValidatedFilesystemDescriptorTimes,
        accessed: Option<std::time::SystemTime>,
        modified: Option<std::time::SystemTime>,
        requested_accessed: RequestedTime,
        requested_modified: RequestedTime,
    ) -> PreparedFilesystemDescriptorTimes {
        PreparedFilesystemDescriptorTimes {
            validated,
            accessed,
            modified,
            requested_accessed,
            requested_modified,
        }
    }

    pub(crate) async fn set_descriptor_times(
        &self,
        prepared: PreparedFilesystemDescriptorTimes,
    ) -> AgentFilesystemOperationResult {
        let PreparedFilesystemDescriptorTimes {
            validated,
            accessed,
            modified,
            requested_accessed,
            requested_modified,
        } = prepared;
        let ValidatedFilesystemDescriptorTimes { effect, descriptor } = validated;
        let before = self
            .initial_probe(
                FilesystemPressureOperation::Metadata,
                descriptor_times(&descriptor).await,
            )
            .await?;
        let started = Instant::now();
        let mut failures = 0;
        loop {
            let descriptor_for_attempt = descriptor.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                super::set_descriptor_times(&descriptor_for_attempt, accessed, modified)
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    failures += 1;
                    let postcondition = times_postcondition(
                        descriptor_times(&descriptor).await,
                        before,
                        requested_accessed,
                        requested_modified,
                        false,
                    );
                    match self
                        .resolve_non_prefix_failure(
                            FilesystemPressureOperation::Metadata,
                            error,
                            postcondition,
                            failures,
                            started,
                        )
                        .await
                    {
                        NonPrefixResolution::Retry => {}
                        NonPrefixResolution::Success => return Ok(()),
                        NonPrefixResolution::Error(error) => return Err(error),
                    }
                }
            }
        }
    }

    pub(crate) fn check_path_times_policy(
        &self,
        admitted: AdmittedFilesystemPathTimes,
        descriptor: Descriptor,
        path: String,
        follow: bool,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemPathTimes> {
        self.ensure_descriptor_writable(&descriptor)?;
        self.ensure_descriptor_path_writable(&descriptor, &path, follow, false)?;
        Ok(PolicyCheckedFilesystemPathTimes {
            effect: admitted.effect,
            descriptor,
            path,
            follow,
        })
    }

    pub(crate) fn prepare_path_times(
        &self,
        checked: PolicyCheckedFilesystemPathTimes,
        directory: Dir,
    ) -> AgentFilesystemOperationResult<ValidatedFilesystemPathTimes> {
        self.ensure_dir_matches_descriptor(&checked.descriptor, &directory)?;
        validate_directory_mutation(&directory).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(ValidatedFilesystemPathTimes {
            effect: checked.effect,
            directory,
            path: checked.path,
            follow: checked.follow,
        })
    }

    pub(crate) fn bind_path_times(
        &self,
        validated: ValidatedFilesystemPathTimes,
        accessed: Option<std::time::SystemTime>,
        modified: Option<std::time::SystemTime>,
        requested_accessed: RequestedTime,
        requested_modified: RequestedTime,
    ) -> PreparedFilesystemPathTimes {
        PreparedFilesystemPathTimes {
            validated,
            accessed,
            modified,
            requested_accessed,
            requested_modified,
        }
    }

    pub(crate) async fn set_path_times(
        &self,
        prepared: PreparedFilesystemPathTimes,
    ) -> AgentFilesystemOperationResult {
        let PreparedFilesystemPathTimes {
            validated,
            accessed,
            modified,
            requested_accessed,
            requested_modified,
        } = prepared;
        let ValidatedFilesystemPathTimes {
            effect,
            directory,
            path,
            follow,
        } = validated;
        let before = self
            .initial_probe(
                FilesystemPressureOperation::Metadata,
                path_times(&directory, &path, follow).await,
            )
            .await?;
        let started = Instant::now();
        let mut failures = 0;
        loop {
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                super::set_path_times(
                    &directory_for_attempt,
                    &path_for_attempt,
                    follow,
                    accessed,
                    modified,
                )
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    failures += 1;
                    let postcondition = times_postcondition(
                        path_times(&directory, &path, follow).await,
                        before,
                        requested_accessed,
                        requested_modified,
                        true,
                    );
                    match self
                        .resolve_non_prefix_failure(
                            FilesystemPressureOperation::Metadata,
                            error,
                            postcondition,
                            failures,
                            started,
                        )
                        .await
                    {
                        NonPrefixResolution::Retry => {}
                        NonPrefixResolution::Success => return Ok(()),
                        NonPrefixResolution::Error(error) => return Err(error),
                    }
                }
            }
        }
    }

    pub(crate) fn check_create_directory_policy(
        &self,
        admitted: AdmittedFilesystemCreateDirectory,
        descriptor: Descriptor,
        path: String,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemCreateDirectory> {
        self.ensure_descriptor_path_writable(&descriptor, &path, false, false)?;
        Ok(PolicyCheckedFilesystemCreateDirectory {
            effect: admitted.effect,
            descriptor,
            path,
        })
    }

    pub(crate) fn prepare_create_directory(
        &self,
        checked: PolicyCheckedFilesystemCreateDirectory,
        directory: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        self.ensure_dir_matches_descriptor(&checked.descriptor, &directory)?;
        validate_directory_mutation(&directory).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemNamespaceMutation {
            effect: checked.effect,
            mutation: NamespaceMutation::CreateDirectory {
                directory,
                path: checked.path,
            },
        })
    }

    pub(crate) fn check_hard_link_source_descriptor_policy(
        &self,
        admitted: AdmittedFilesystemHardLink,
        source: Descriptor,
        source_path: String,
        source_follow: bool,
    ) -> AgentFilesystemOperationResult<SourceCheckedFilesystemHardLink> {
        self.ensure_descriptor_writable(&source)?;
        Ok(SourceCheckedFilesystemHardLink {
            effect: admitted.effect,
            source,
            source_path,
            source_follow,
        })
    }

    pub(crate) fn check_hard_link_destination_descriptor_policy(
        &self,
        source_checked: SourceCheckedFilesystemHardLink,
        destination: Descriptor,
        destination_path: String,
    ) -> AgentFilesystemOperationResult<DestinationCheckedFilesystemHardLink> {
        self.ensure_descriptor_writable(&destination)?;
        Ok(DestinationCheckedFilesystemHardLink {
            source_checked,
            destination,
            destination_path,
        })
    }

    pub(crate) fn check_hard_link_path_policy(
        &self,
        destination_checked: DestinationCheckedFilesystemHardLink,
    ) -> AgentFilesystemOperationResult<PathsCheckedFilesystemHardLink> {
        let source = &destination_checked.source_checked;
        self.ensure_descriptor_path_writable(
            &source.source,
            &source.source_path,
            source.source_follow,
            false,
        )?;
        self.ensure_descriptor_path_writable(
            &destination_checked.destination,
            &destination_checked.destination_path,
            false,
            false,
        )?;
        Ok(PathsCheckedFilesystemHardLink {
            destination_checked,
        })
    }

    pub(crate) fn prepare_hard_link(
        &self,
        checked: PathsCheckedFilesystemHardLink,
        source: Dir,
        destination: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        let DestinationCheckedFilesystemHardLink {
            source_checked,
            destination: checked_destination,
            destination_path,
        } = checked.destination_checked;
        self.ensure_dir_matches_descriptor(&source_checked.source, &source)?;
        self.ensure_dir_matches_descriptor(&checked_destination, &destination)?;
        validate_two_directory_mutation(&source, &destination)
            .map_err(AgentFilesystemMutationError::Guest)?;
        if source_checked.source_follow {
            return Err(AgentFilesystemMutationError::Guest(
                NativeMutationGuestError::Invalid,
            ));
        }
        Ok(PreparedFilesystemNamespaceMutation {
            effect: source_checked.effect,
            mutation: NamespaceMutation::HardLink {
                source,
                source_path: source_checked.source_path,
                source_follow: source_checked.source_follow,
                destination,
                destination_path,
            },
        })
    }

    pub(crate) fn check_rename_source_descriptor_policy(
        &self,
        admitted: AdmittedFilesystemRename,
        source: Descriptor,
        source_path: String,
    ) -> AgentFilesystemOperationResult<SourceCheckedFilesystemRename> {
        self.ensure_descriptor_writable(&source)?;
        Ok(SourceCheckedFilesystemRename {
            effect: admitted.effect,
            source,
            source_path,
        })
    }

    pub(crate) fn check_rename_destination_descriptor_policy(
        &self,
        source_checked: SourceCheckedFilesystemRename,
        destination: Descriptor,
        destination_path: String,
    ) -> AgentFilesystemOperationResult<DestinationCheckedFilesystemRename> {
        self.ensure_descriptor_writable(&destination)?;
        Ok(DestinationCheckedFilesystemRename {
            source_checked,
            destination,
            destination_path,
        })
    }

    pub(crate) fn check_rename_path_policy(
        &self,
        destination_checked: DestinationCheckedFilesystemRename,
    ) -> AgentFilesystemOperationResult<PathsCheckedFilesystemRename> {
        self.ensure_descriptor_path_writable(
            &destination_checked.source_checked.source,
            &destination_checked.source_checked.source_path,
            false,
            true,
        )?;
        self.ensure_descriptor_path_writable(
            &destination_checked.destination,
            &destination_checked.destination_path,
            false,
            true,
        )?;
        Ok(PathsCheckedFilesystemRename {
            destination_checked,
        })
    }

    pub(crate) fn prepare_rename(
        &self,
        checked: PathsCheckedFilesystemRename,
        source: Dir,
        destination: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        let DestinationCheckedFilesystemRename {
            source_checked,
            destination: checked_destination,
            destination_path,
        } = checked.destination_checked;
        self.ensure_dir_matches_descriptor(&source_checked.source, &source)?;
        self.ensure_dir_matches_descriptor(&checked_destination, &destination)?;
        validate_two_directory_mutation(&source, &destination)
            .map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemNamespaceMutation {
            effect: source_checked.effect,
            mutation: NamespaceMutation::Rename {
                source,
                source_path: source_checked.source_path,
                destination,
                destination_path,
            },
        })
    }

    pub(crate) fn check_remove_directory_policy(
        &self,
        admitted: AdmittedFilesystemRemoveDirectory,
        descriptor: Descriptor,
        path: String,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemRemoveDirectory> {
        self.ensure_descriptor_path_writable(&descriptor, &path, false, true)?;
        Ok(PolicyCheckedFilesystemRemoveDirectory {
            effect: admitted.effect,
            descriptor,
            path,
        })
    }

    pub(crate) fn prepare_remove_directory(
        &self,
        checked: PolicyCheckedFilesystemRemoveDirectory,
        directory: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        self.ensure_dir_matches_descriptor(&checked.descriptor, &directory)?;
        validate_directory_mutation(&directory).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemNamespaceMutation {
            effect: checked.effect,
            mutation: NamespaceMutation::RemoveDirectory {
                directory,
                path: checked.path,
            },
        })
    }

    pub(crate) fn check_symlink_policy(
        &self,
        admitted: AdmittedFilesystemSymlink,
        descriptor: Descriptor,
        target: String,
        path: String,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemSymlink> {
        self.ensure_descriptor_writable(&descriptor)?;
        self.ensure_descriptor_path_writable(&descriptor, &path, false, false)?;
        Ok(PolicyCheckedFilesystemSymlink {
            effect: admitted.effect,
            descriptor,
            target,
            path,
        })
    }

    pub(crate) fn prepare_symlink(
        &self,
        checked: PolicyCheckedFilesystemSymlink,
        directory: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        self.ensure_dir_matches_descriptor(&checked.descriptor, &directory)?;
        validate_directory_mutation(&directory).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemNamespaceMutation {
            effect: checked.effect,
            mutation: NamespaceMutation::Symlink {
                directory,
                target: checked.target,
                path: checked.path,
            },
        })
    }

    pub(crate) fn check_unlink_file_policy(
        &self,
        admitted: AdmittedFilesystemUnlinkFile,
        descriptor: Descriptor,
        path: String,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemUnlinkFile> {
        self.ensure_descriptor_writable(&descriptor)?;
        self.ensure_descriptor_path_writable(&descriptor, &path, false, false)?;
        Ok(PolicyCheckedFilesystemUnlinkFile {
            effect: admitted.effect,
            descriptor,
            path,
        })
    }

    pub(crate) fn prepare_unlink_file(
        &self,
        checked: PolicyCheckedFilesystemUnlinkFile,
        directory: Dir,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemNamespaceMutation> {
        self.ensure_dir_matches_descriptor(&checked.descriptor, &directory)?;
        validate_directory_mutation(&directory).map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemNamespaceMutation {
            effect: checked.effect,
            mutation: NamespaceMutation::UnlinkFile {
                directory,
                path: checked.path,
            },
        })
    }

    pub(crate) async fn run_namespace_mutation(
        &self,
        prepared: PreparedFilesystemNamespaceMutation,
    ) -> AgentFilesystemOperationResult {
        self.run_namespace(prepared.effect, prepared.mutation).await
    }

    pub(crate) fn check_writable_open_policy(
        &self,
        descriptor: &Descriptor,
        path: &str,
        follow: bool,
    ) -> AgentFilesystemOperationResult {
        self.ensure_descriptor_path_writable(descriptor, path, follow, false)
    }

    pub(crate) fn check_mutating_open_policy(
        &self,
        admitted: AdmittedFilesystemMutatingOpen,
        descriptor: Descriptor,
        path: String,
        follow: bool,
    ) -> AgentFilesystemOperationResult<PolicyCheckedFilesystemMutatingOpen> {
        self.ensure_descriptor_path_writable(&descriptor, &path, follow, false)?;
        Ok(PolicyCheckedFilesystemMutatingOpen {
            effect: admitted.effect,
            descriptor: Some(descriptor),
            path,
            follow,
        })
    }

    pub(crate) fn bind_nonwritable_mutating_open(
        &self,
        admitted: AdmittedFilesystemMutatingOpen,
        path: String,
        follow: bool,
    ) -> PolicyCheckedFilesystemMutatingOpen {
        PolicyCheckedFilesystemMutatingOpen {
            effect: admitted.effect,
            descriptor: None,
            path,
            follow,
        }
    }

    pub(crate) fn prepare_mutating_open(
        &self,
        checked: PolicyCheckedFilesystemMutatingOpen,
        directory: Dir,
        options: NativeOpenOptions,
        unsupported_sync_flags: bool,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemMutatingOpen> {
        self.ensure_matching_follow(checked.follow, options.follow)?;
        match &checked.descriptor {
            Some(descriptor) => self.ensure_dir_matches_descriptor(descriptor, &directory)?,
            None if options.truncate || options.write => {
                return Err(Self::invalid_prepared_input());
            }
            None => {}
        }
        validate_open(&directory, options, unsupported_sync_flags)
            .map_err(AgentFilesystemMutationError::Guest)?;
        Ok(PreparedFilesystemMutatingOpen {
            effect: checked.effect,
            directory,
            path: checked.path,
            options,
            unsupported_sync_flags,
        })
    }

    pub(crate) async fn open_mutating(
        &self,
        prepared: PreparedFilesystemMutatingOpen,
    ) -> AgentFilesystemOperationResult<NativeOpenResult> {
        let PreparedFilesystemMutatingOpen {
            effect,
            directory,
            path,
            options,
            unsupported_sync_flags: _,
        } = prepared;
        let before = self
            .initial_probe(
                FilesystemPressureOperation::Metadata,
                path_state_with_follow(&directory, &path, options.follow).await,
            )
            .await?;
        let requested_type = if options.directory {
            PathObjectType::Directory
        } else {
            PathObjectType::RegularFile
        };
        let operation = if options.create {
            FilesystemPressureOperation::Create
        } else {
            FilesystemPressureOperation::Resize
        };
        let started = Instant::now();
        let mut failures = 0;
        loop {
            let attempt_directory = directory.clone();
            let attempt_path = path.clone();
            let attempt_effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(attempt_effect, move || {
                super::open(&attempt_directory, &attempt_path, options)
            })
            .await
            {
                Ok(result) => return Ok(result),
                Err(error) => {
                    failures += 1;
                    let postcondition = open_postcondition(
                        before,
                        path_state_with_follow(&directory, &path, options.follow).await,
                        requested_type,
                        options.truncate,
                        options.exclusive,
                    );
                    match self
                        .resolve_non_prefix_failure(
                            operation,
                            error,
                            postcondition,
                            failures,
                            started,
                        )
                        .await
                    {
                        NonPrefixResolution::Retry => {}
                        NonPrefixResolution::Error(error) => return Err(error),
                        NonPrefixResolution::Success => {
                            let safe = NativeOpenOptions {
                                create: false,
                                truncate: false,
                                exclusive: false,
                                ..options
                            };
                            return self
                                .safe_reopen(
                                    Arc::new(NativeFilesystemSafeReopen {
                                        directory: directory.clone(),
                                        path: path.clone(),
                                        options: safe,
                                    }),
                                    effect,
                                    operation,
                                    failures,
                                    started,
                                )
                                .await;
                        }
                    }
                }
            }
        }
    }

    async fn safe_reopen(
        &self,
        native: Arc<dyn FilesystemSafeReopen>,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
        operation: FilesystemPressureOperation,
        mut failures: usize,
        started: Instant,
    ) -> AgentFilesystemOperationResult<NativeOpenResult> {
        loop {
            match native.reopen(Arc::clone(&effect)).await {
                Ok(result) => return Ok(result),
                Err(error) => {
                    failures += 1;
                    match self
                        .resolve_non_prefix_failure(
                            operation,
                            error,
                            MutationPostcondition::NoEffect,
                            failures,
                            started,
                        )
                        .await
                    {
                        NonPrefixResolution::Retry => {}
                        NonPrefixResolution::Error(error) => return Err(error),
                        NonPrefixResolution::Success => {
                            unreachable!("failed nonmutating reopen cannot satisfy the open")
                        }
                    }
                }
            }
        }
    }

    async fn run_namespace(
        &self,
        effect: Arc<AgentFilesystemEffectLease>,
        mutation: NamespaceMutation,
    ) -> AgentFilesystemOperationResult {
        let operation = mutation.operation();
        let before = mutation.before(self).await?;
        let started = Instant::now();
        let mut failures = 0;
        loop {
            match mutation.execute(Arc::clone(&effect)).await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    failures += 1;
                    let postcondition = mutation.postcondition(&before).await;
                    match self
                        .resolve_non_prefix_failure(
                            operation,
                            error,
                            postcondition,
                            failures,
                            started,
                        )
                        .await
                    {
                        NonPrefixResolution::Retry => {}
                        NonPrefixResolution::Success => return Ok(()),
                        NonPrefixResolution::Error(error) => return Err(error),
                    }
                }
            }
        }
    }

    async fn begin_effect(
        &self,
    ) -> AgentFilesystemOperationResult<Arc<AgentFilesystemEffectLease>> {
        self.runtime
            .begin_effect()
            .await
            .map(Arc::new)
            .map_err(|_| AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            })
    }

    async fn begin_update_effect(
        &self,
    ) -> AgentFilesystemOperationResult<Arc<AgentFilesystemUpdateEffectLease>> {
        self.runtime
            .begin_update_effect()
            .await
            .map(Arc::new)
            .map_err(|_| AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            })
    }

    async fn begin_namespace_effect(
        &self,
    ) -> AgentFilesystemOperationResult<Arc<AgentFilesystemEffectLease>> {
        self.runtime
            .begin_path_effect()
            .await
            .map(Arc::new)
            .map_err(|_| AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            })
    }

    fn ensure_descriptor_path_writable(
        &self,
        descriptor: &Descriptor,
        path: &str,
        follow: bool,
        include_descendants: bool,
    ) -> AgentFilesystemOperationResult {
        let base = match descriptor {
            Descriptor::File(file) => &file.path,
            Descriptor::Dir(directory) => &directory.path,
        };
        self.ensure_writable(&base.join(path), follow, include_descendants)
    }

    fn ensure_descriptor_writable(
        &self,
        descriptor: &Descriptor,
    ) -> AgentFilesystemOperationResult {
        match descriptor {
            Descriptor::File(file) => self.ensure_writable(&file.path, true, false),
            Descriptor::Dir(_) => Ok(()),
        }
    }

    fn ensure_file_matches_descriptor(
        &self,
        descriptor: &Descriptor,
        file: &File,
    ) -> AgentFilesystemOperationResult {
        match descriptor {
            Descriptor::File(checked)
                if checked.path == file.path && Arc::ptr_eq(&checked.file, &file.file) =>
            {
                Ok(())
            }
            Descriptor::File(_) | Descriptor::Dir(_) => Err(Self::invalid_prepared_input()),
        }
    }

    fn ensure_dir_matches_descriptor(
        &self,
        descriptor: &Descriptor,
        directory: &Dir,
    ) -> AgentFilesystemOperationResult {
        match descriptor {
            Descriptor::Dir(checked)
                if checked.path == directory.path && Arc::ptr_eq(&checked.dir, &directory.dir) =>
            {
                Ok(())
            }
            Descriptor::File(_) | Descriptor::Dir(_) => Err(Self::invalid_prepared_input()),
        }
    }

    fn ensure_descriptor_matches(
        &self,
        checked: &Descriptor,
        descriptor: &Descriptor,
    ) -> AgentFilesystemOperationResult {
        match (checked, descriptor) {
            (Descriptor::File(checked), Descriptor::File(descriptor))
                if checked.path == descriptor.path
                    && Arc::ptr_eq(&checked.file, &descriptor.file) =>
            {
                Ok(())
            }
            (Descriptor::Dir(checked), Descriptor::Dir(descriptor))
                if checked.path == descriptor.path
                    && Arc::ptr_eq(&checked.dir, &descriptor.dir) =>
            {
                Ok(())
            }
            _ => Err(Self::invalid_prepared_input()),
        }
    }

    fn ensure_matching_follow(
        &self,
        checked: bool,
        supplied: bool,
    ) -> AgentFilesystemOperationResult {
        if checked == supplied {
            Ok(())
        } else {
            Err(Self::invalid_prepared_input())
        }
    }

    fn invalid_prepared_input() -> AgentFilesystemMutationError {
        AgentFilesystemMutationError::RuntimeInvalidated {
            error: None,
            completed: None,
        }
    }

    fn ensure_writable(
        &self,
        path: &Path,
        follow: bool,
        include_descendants: bool,
    ) -> AgentFilesystemOperationResult {
        let read_only = if include_descendants {
            self.runtime.contains_read_only_path(path, follow)
        } else {
            self.runtime.is_read_only_path(path, follow)
        };
        if read_only {
            Err(AgentFilesystemMutationError::Guest(
                NativeMutationGuestError::NotPermitted,
            ))
        } else {
            Ok(())
        }
    }

    async fn initial_probe<T>(
        &self,
        operation: FilesystemPressureOperation,
        result: std::io::Result<T>,
    ) -> AgentFilesystemOperationResult<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(self
                .classify_probe_failure(operation, error)
                .await
                .unwrap_err()),
        }
    }

    async fn resolve_non_prefix_failure(
        &self,
        operation: FilesystemPressureOperation,
        error: std::io::Error,
        postcondition: MutationPostcondition,
        failures: usize,
        started: Instant,
    ) -> NonPrefixResolution {
        let native_error = NativeFilesystemError::capture(&error);
        let effect = match postcondition {
            MutationPostcondition::Satisfied => MutationEffect::DesiredPostconditionSatisfied,
            MutationPostcondition::NoEffect => MutationEffect::ProvenNoEffect,
            MutationPostcondition::Unknown => MutationEffect::Unknown,
        };
        let decision = self
            .runtime
            .classify_io_failure(operation, error, effect)
            .await;
        let failures = if postcondition == MutationPostcondition::NoEffect {
            failures
        } else {
            FILESYSTEM_MUTATION_MAX_ATTEMPTS
        };
        match resolve_decision(
            &self.runtime,
            decision,
            native_error,
            0,
            failures,
            started,
            false,
            operation,
        )
        .await
        {
            DecisionResolution::Retry => NonPrefixResolution::Retry,
            DecisionResolution::Complete(Ok(_)) => NonPrefixResolution::Success,
            DecisionResolution::Complete(Err(error)) => NonPrefixResolution::Error(error),
        }
    }

    async fn resize_with_native(
        &self,
        native: Arc<dyn FilesystemResize>,
        size: u64,
    ) -> AgentFilesystemMutationResult {
        let effect = self.begin_update_effect().await?;
        let prepared = self
            .prepare_resize_with_native(native, effect, size)
            .await?;
        self.resize_with_prepared_native(prepared).await
    }

    async fn prepare_resize_with_native(
        &self,
        native: Arc<dyn FilesystemResize>,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
        size: u64,
    ) -> AgentFilesystemOperationResult<PreparedFilesystemResize> {
        let before = match native.state().await {
            Ok(before) => before,
            Err(error) => {
                return self
                    .classify_probe_failure(FilesystemPressureOperation::Resize, error)
                    .await
                    .map(|_| unreachable!("probe failure cannot produce resize progress"));
            }
        };
        Ok(PreparedFilesystemResize {
            native,
            effect,
            before,
            size,
        })
    }

    async fn resize_with_prepared_native(
        &self,
        prepared: PreparedFilesystemResize,
    ) -> AgentFilesystemMutationResult {
        let PreparedFilesystemResize {
            native,
            effect,
            before,
            size,
        } = prepared;
        let started = Instant::now();
        let mut failures = 0;

        loop {
            let result = native.resize(size, Arc::clone(&effect)).await;
            let Err(error) = result else {
                return Ok(0);
            };
            failures += 1;
            let native_error = NativeFilesystemError::capture(&error);
            let postcondition = resize_postcondition(before, native.state().await, size);
            let mutation_effect = match postcondition {
                MutationPostcondition::Satisfied => MutationEffect::DesiredPostconditionSatisfied,
                MutationPostcondition::NoEffect => MutationEffect::ProvenNoEffect,
                MutationPostcondition::Unknown => MutationEffect::Unknown,
            };
            let decision = self
                .runtime
                .classify_io_failure(FilesystemPressureOperation::Resize, error, mutation_effect)
                .await;
            match resolve_decision(
                &self.runtime,
                decision,
                native_error,
                0,
                failures,
                started,
                false,
                FilesystemPressureOperation::Resize,
            )
            .await
            {
                DecisionResolution::Retry => {}
                DecisionResolution::Complete(result) => return result,
            }
        }
    }

    async fn classify_probe_failure(
        &self,
        operation: FilesystemPressureOperation,
        error: std::io::Error,
    ) -> AgentFilesystemMutationResult {
        let native_error = NativeFilesystemError::capture(&error);
        match self
            .runtime
            .classify_io_failure(operation, error, MutationEffect::ProvenNoEffect)
            .await
        {
            MutationDecision::Quota => Err(AgentFilesystemMutationError::QuotaExhausted {
                error: native_error,
                completed: 0,
            }),
            MutationDecision::InsufficientSpace | MutationDecision::PhysicalPressure => {
                Err(AgentFilesystemMutationError::InsufficientSpace {
                    error: native_error,
                    completed: 0,
                })
            }
            MutationDecision::Invalidate => Err(AgentFilesystemMutationError::RuntimeInvalidated {
                error: Some(native_error),
                completed: Some(0),
            }),
            MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
                Err(AgentFilesystemMutationError::Native {
                    error: native_error,
                    completed: 0,
                })
            }
            MutationDecision::Success => unreachable!("failed probe cannot satisfy a mutation"),
        }
    }

    #[cfg(test)]
    fn writer_with_native(
        &self,
        native: Arc<dyn FilesystemWriter>,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        AgentFilesystemWriter::new(self.clone(), native, mode, WriteCompletion::Fill)
    }

    #[cfg(test)]
    async fn resize_with_scripted_native(
        &self,
        native: Arc<dyn FilesystemResize>,
        size: u64,
    ) -> AgentFilesystemMutationResult {
        self.resize_with_native(native, size).await
    }
}

enum NamespaceMutation {
    CreateDirectory {
        directory: Dir,
        path: String,
    },
    HardLink {
        source: Dir,
        source_path: String,
        source_follow: bool,
        destination: Dir,
        destination_path: String,
    },
    Rename {
        source: Dir,
        source_path: String,
        destination: Dir,
        destination_path: String,
    },
    RemoveDirectory {
        directory: Dir,
        path: String,
    },
    Symlink {
        directory: Dir,
        target: String,
        path: String,
    },
    UnlinkFile {
        directory: Dir,
        path: String,
    },
}

enum NamespaceBefore {
    SinglePathState(Option<PathState>),
    SourceAndDestinationStates(Option<PathState>, Option<PathState>),
    SymlinkState(SymlinkState),
}

impl NamespaceMutation {
    fn operation(&self) -> FilesystemPressureOperation {
        match self {
            Self::CreateDirectory { .. } | Self::Symlink { .. } => {
                FilesystemPressureOperation::Create
            }
            _ => FilesystemPressureOperation::Metadata,
        }
    }

    async fn before(
        &self,
        mutations: &AgentFilesystemMutations,
    ) -> AgentFilesystemOperationResult<NamespaceBefore> {
        match self {
            Self::CreateDirectory { directory, path }
            | Self::RemoveDirectory { directory, path }
            | Self::UnlinkFile { directory, path } => mutations
                .initial_probe(
                    FilesystemPressureOperation::Metadata,
                    path_state(directory, path).await,
                )
                .await
                .map(NamespaceBefore::SinglePathState),
            Self::HardLink {
                source,
                source_path,
                source_follow: _,
                destination,
                destination_path,
            }
            | Self::Rename {
                source,
                source_path,
                destination,
                destination_path,
            } => {
                let source = mutations
                    .initial_probe(
                        FilesystemPressureOperation::Metadata,
                        path_state(source, source_path).await,
                    )
                    .await?;
                let destination = mutations
                    .initial_probe(
                        FilesystemPressureOperation::Metadata,
                        path_state(destination, destination_path).await,
                    )
                    .await?;
                Ok(NamespaceBefore::SourceAndDestinationStates(
                    source,
                    destination,
                ))
            }
            Self::Symlink {
                directory, path, ..
            } => mutations
                .initial_probe(
                    FilesystemPressureOperation::Metadata,
                    symlink_state(directory, path).await,
                )
                .await
                .map(NamespaceBefore::SymlinkState),
        }
    }

    async fn execute(&self, effect: Arc<AgentFilesystemEffectLease>) -> std::io::Result<()> {
        match self {
            Self::CreateDirectory { directory, path } => {
                let directory = directory.clone();
                let path = path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::create_directory(&directory, &path)
                })
                .await
            }
            Self::HardLink {
                source,
                source_path,
                source_follow: _,
                destination,
                destination_path,
            } => {
                let source = source.clone();
                let source_path = source_path.clone();
                let destination = destination.clone();
                let destination_path = destination_path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::hard_link(&source, &source_path, &destination, &destination_path)
                })
                .await
            }
            Self::Rename {
                source,
                source_path,
                destination,
                destination_path,
            } => {
                let source = source.clone();
                let source_path = source_path.clone();
                let destination = destination.clone();
                let destination_path = destination_path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::rename(&source, &source_path, &destination, &destination_path)
                })
                .await
            }
            Self::RemoveDirectory { directory, path } => {
                let directory = directory.clone();
                let path = path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::remove_directory(&directory, &path)
                })
                .await
            }
            Self::Symlink {
                directory,
                target,
                path,
            } => {
                let directory = directory.clone();
                let target = target.clone();
                let path = path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::symlink(&directory, &target, &path)
                })
                .await
            }
            Self::UnlinkFile { directory, path } => {
                let directory = directory.clone();
                let path = path.clone();
                run_blocking_filesystem_mutation(effect, move || {
                    super::unlink_file(&directory, &path)
                })
                .await
            }
        }
    }

    async fn postcondition(&self, before: &NamespaceBefore) -> MutationPostcondition {
        match (self, before) {
            (
                Self::CreateDirectory { directory, path },
                NamespaceBefore::SinglePathState(before),
            ) => create_directory_postcondition(*before, path_state(directory, path).await),
            (
                Self::RemoveDirectory { directory, path } | Self::UnlinkFile { directory, path },
                NamespaceBefore::SinglePathState(before),
            ) => remove_postcondition(*before, path_state(directory, path).await),
            (
                Self::HardLink {
                    source,
                    source_path,
                    source_follow: _,
                    destination,
                    destination_path,
                },
                NamespaceBefore::SourceAndDestinationStates(source_before, destination_before),
            ) => link_postcondition(
                *source_before,
                *destination_before,
                path_state(source, source_path).await,
                path_state(destination, destination_path).await,
            ),
            (
                Self::Rename {
                    source,
                    source_path,
                    destination,
                    destination_path,
                },
                NamespaceBefore::SourceAndDestinationStates(source_before, destination_before),
            ) => rename_postcondition(
                *source_before,
                *destination_before,
                path_state(source, source_path).await,
                path_state(destination, destination_path).await,
            ),
            (
                Self::Symlink {
                    directory,
                    target,
                    path,
                },
                NamespaceBefore::SymlinkState(before),
            ) => symlink_postcondition(before, symlink_state(directory, path).await, target),
            _ => unreachable!("namespace mutation precondition type mismatch"),
        }
    }
}

pub(crate) struct AgentFilesystemWriter {
    mutations: AgentFilesystemMutations,
    native: Arc<dyn FilesystemWriter>,
    state: Arc<tokio::sync::Mutex<WriterState>>,
    sequence: Arc<WriterSequence>,
    completion: WriteCompletion,
}

#[derive(Clone, Copy)]
enum WriteCompletion {
    Fill,
    FirstSuccess,
}

struct WriterState {
    mode: AgentFilesystemWriteMode,
}

struct WriterSequence {
    state: std::sync::Mutex<WriterSequenceState>,
    advanced: tokio::sync::Notify,
}

struct WriterSequenceState {
    next_admission: u64,
    next_execution: u64,
    skipped: std::collections::BTreeSet<u64>,
}

impl AgentFilesystemWriter {
    fn new(
        mutations: AgentFilesystemMutations,
        native: Arc<dyn FilesystemWriter>,
        mode: AgentFilesystemWriteMode,
        completion: WriteCompletion,
    ) -> Self {
        Self {
            mutations,
            native,
            state: Arc::new(tokio::sync::Mutex::new(WriterState { mode })),
            sequence: Arc::new(WriterSequence {
                state: std::sync::Mutex::new(WriterSequenceState {
                    next_admission: 0,
                    next_execution: 0,
                    skipped: std::collections::BTreeSet::new(),
                }),
                advanced: tokio::sync::Notify::new(),
            }),
            completion,
        }
    }

    pub(crate) fn admit(
        &self,
        contents: Bytes,
    ) -> Result<AdmittedFilesystemWrite, AgentFilesystemMutationError> {
        let admission = self.mutations.runtime.admit_effect().map_err(|_| {
            AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            }
        })?;
        Ok(AdmittedFilesystemWrite {
            runtime: self.mutations.runtime.clone(),
            native: Arc::clone(&self.native),
            state: Arc::clone(&self.state),
            ticket: WriterTicket::reserve(&self.sequence),
            contents,
            admission,
            completion: self.completion,
        })
    }
}

pub(crate) struct AdmittedFilesystemWrite {
    runtime: AgentFilesystemRuntime,
    native: Arc<dyn FilesystemWriter>,
    state: Arc<tokio::sync::Mutex<WriterState>>,
    ticket: WriterTicket,
    contents: Bytes,
    admission: AgentFilesystemEffectAdmission,
    completion: WriteCompletion,
}

impl AdmittedFilesystemWrite {
    pub(crate) fn execute(
        self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AgentFilesystemWriteCompletion {
        let runtime = self.runtime.clone();
        AgentFilesystemWriteCompletion {
            runtime: runtime.clone(),
            task: tokio::spawn(async move {
                match tokio::spawn(self.run(cancellation)).await {
                    Ok(result) => result,
                    Err(_) => {
                        runtime.invalidate_runtime().await;
                        Err(AgentFilesystemMutationError::RuntimeInvalidated {
                            error: None,
                            completed: None,
                        })
                    }
                }
            }),
        }
    }

    async fn run(
        mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AgentFilesystemMutationResult {
        if !self.ticket.wait_for_turn(&cancellation).await {
            return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
        }
        let mut state = tokio::select! {
            state = self.state.lock() => state,
            _ = cancellation.cancelled() => {
                return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
            }
        };
        let initial_mode = state.mode;
        let effect = tokio::select! {
            effect = async {
                match initial_mode {
                    AgentFilesystemWriteMode::Position(_) => self.admission.begin().await,
                    AgentFilesystemWriteMode::Append => self.admission.begin_append().await,
                }
            } => effect.map_err(|_| AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: None,
            })?,
            _ = cancellation.cancelled() => {
                return Err(AgentFilesystemMutationError::Cancelled { completed: 0 });
            }
        };
        let effect = Arc::new(effect);
        let started = Instant::now();
        let mut completed = 0usize;
        let mut failures = 0;

        while completed < self.contents.len() {
            if cancellation.is_cancelled() {
                let completed = completed_u64(completed);
                advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
                return Err(AgentFilesystemMutationError::Cancelled { completed });
            }
            let mode = match attempt_mode(initial_mode, completed) {
                Ok(mode) => mode,
                Err(error) => {
                    self.runtime.invalidate_runtime().await;
                    return Err(error);
                }
            };
            let attempt = self
                .native
                .write(mode, self.contents.slice(completed..), Arc::clone(&effect))
                .await;
            let remaining = self.contents.len() - completed;
            if attempt.written > remaining {
                self.runtime.invalidate_runtime().await;
                return Err(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: Some(completed_u64(completed)),
                });
            }
            completed += attempt.written;

            let (error, mutation_effect) = match attempt.result {
                Ok(()) if matches!(self.completion, WriteCompletion::FirstSuccess) => {
                    let completed = completed_u64(completed);
                    advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
                    return Ok(completed);
                }
                Ok(()) if attempt.written != 0 => {
                    if cancellation.is_cancelled() {
                        let completed = completed_u64(completed);
                        advance_position_or_invalidate(&self.runtime, &mut state, completed)
                            .await?;
                        return Err(AgentFilesystemMutationError::Cancelled { completed });
                    }
                    continue;
                }
                Ok(()) => (
                    std::io::Error::from(std::io::ErrorKind::WriteZero),
                    proven_write_progress_effect(completed),
                ),
                Err(error) => {
                    let effect = attempt
                        .failure_effect
                        .unwrap_or_else(|| native_write_failure_effect(&error, completed));
                    (error, effect)
                }
            };
            failures += 1;
            let native_error = NativeFilesystemError::capture(&error);
            let decision = self
                .runtime
                .classify_io_failure(FilesystemPressureOperation::Write, error, mutation_effect)
                .await;
            let completed_u64 = completed_u64(completed);
            match resolve_decision(
                &self.runtime,
                decision,
                native_error,
                completed_u64,
                failures,
                started,
                cancellation.is_cancelled(),
                FilesystemPressureOperation::Write,
            )
            .await
            {
                DecisionResolution::Retry => {}
                DecisionResolution::Complete(result) => {
                    advance_position_or_invalidate(&self.runtime, &mut state, completed_u64)
                        .await?;
                    return result;
                }
            }
        }

        let completed = completed_u64(completed);
        advance_position_or_invalidate(&self.runtime, &mut state, completed).await?;
        Ok(completed)
    }
}

pub(crate) struct AgentFilesystemWriteCompletion {
    runtime: AgentFilesystemRuntime,
    task: tokio::task::JoinHandle<AgentFilesystemMutationResult>,
}

impl Future for AgentFilesystemWriteCompletion {
    type Output = AgentFilesystemMutationResult;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.task).poll(context).map(|result| {
            result.unwrap_or_else(|_| {
                self.runtime.seal();
                Err(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: None,
                })
            })
        })
    }
}

struct WriterTicket {
    sequence: Arc<WriterSequence>,
    number: u64,
    entered: bool,
}

impl WriterTicket {
    fn reserve(sequence: &Arc<WriterSequence>) -> Self {
        let number = {
            let mut state = sequence
                .state
                .lock()
                .expect("filesystem writer sequence lock poisoned");
            let number = state.next_admission;
            state.next_admission = state
                .next_admission
                .checked_add(1)
                .expect("filesystem writer admission sequence overflowed");
            number
        };
        Self {
            sequence: Arc::clone(sequence),
            number,
            entered: false,
        }
    }

    async fn wait_for_turn(&mut self, cancellation: &tokio_util::sync::CancellationToken) -> bool {
        loop {
            let mut advanced = Box::pin(self.sequence.advanced.notified());
            advanced.as_mut().enable();
            if self
                .sequence
                .state
                .lock()
                .expect("filesystem writer sequence lock poisoned")
                .next_execution
                == self.number
            {
                self.entered = true;
                return true;
            }
            tokio::select! {
                _ = advanced => {}
                _ = cancellation.cancelled() => return false,
            }
        }
    }
}

impl Drop for WriterTicket {
    fn drop(&mut self) {
        let mut state = self
            .sequence
            .state
            .lock()
            .expect("filesystem writer sequence lock poisoned");
        if self.entered {
            debug_assert_eq!(state.next_execution, self.number);
            state.next_execution += 1;
        } else {
            state.skipped.insert(self.number);
        }
        while state.next_execution < state.next_admission {
            let next = state.next_execution;
            if !state.skipped.remove(&next) {
                break;
            }
            state.next_execution += 1;
        }
        drop(state);
        self.sequence.advanced.notify_waiters();
    }
}

struct FilesystemWriteAttempt {
    written: usize,
    result: std::io::Result<()>,
    failure_effect: Option<MutationEffect>,
}

#[async_trait]
trait FilesystemWriter: Send + Sync {
    async fn write(
        &self,
        mode: AgentFilesystemWriteMode,
        contents: Bytes,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt;
}

#[async_trait]
trait FilesystemResize: Send + Sync {
    async fn state(&self) -> std::io::Result<PathState>;

    async fn resize(
        &self,
        size: u64,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<()>;
}

#[async_trait]
trait FilesystemSafeReopen: Send + Sync {
    async fn reopen(
        &self,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<NativeOpenResult>;
}

struct NativeFilesystemResize {
    file: File,
}

#[async_trait]
impl FilesystemResize for NativeFilesystemResize {
    async fn state(&self) -> std::io::Result<PathState> {
        descriptor_state(&Descriptor::File(self.file.clone())).await
    }

    async fn resize(
        &self,
        size: u64,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<()> {
        let file = self.file.clone();
        run_blocking_filesystem_mutation(effect, move || resize_file(&file, size)).await
    }
}

struct NativeFilesystemSafeReopen {
    directory: Dir,
    path: String,
    options: NativeOpenOptions,
}

#[async_trait]
impl FilesystemSafeReopen for NativeFilesystemSafeReopen {
    async fn reopen(
        &self,
        effect: Arc<AgentFilesystemUpdateEffectLease>,
    ) -> std::io::Result<NativeOpenResult> {
        let directory = self.directory.clone();
        let path = self.path.clone();
        let options = self.options;
        run_blocking_filesystem_mutation(effect, move || super::open(&directory, &path, options))
            .await
    }
}

struct NativeFilesystemWriter {
    file: File,
}

#[async_trait]
impl FilesystemWriter for NativeFilesystemWriter {
    async fn write(
        &self,
        mode: AgentFilesystemWriteMode,
        contents: Bytes,
        effect: Arc<AgentFilesystemEffectLease>,
    ) -> FilesystemWriteAttempt {
        let file = Arc::clone(&self.file.file);
        spawn_blocking(move || {
            let _effect = effect;
            let result = match mode {
                AgentFilesystemWriteMode::Position(position) => file.write_at(&contents, position),
                AgentFilesystemWriteMode::Append => {
                    let mut file = file.as_ref();
                    file.seek(SeekFrom::End(0))
                        .and_then(|_| file.write(&contents))
                }
            };
            match result {
                Ok(written) => FilesystemWriteAttempt {
                    written,
                    result: Ok(()),
                    failure_effect: None,
                },
                Err(error) => FilesystemWriteAttempt {
                    written: 0,
                    result: Err(error),
                    failure_effect: None,
                },
            }
        })
        .await
    }
}

enum DecisionResolution {
    Retry,
    Complete(AgentFilesystemMutationResult),
}

enum NonPrefixResolution {
    Retry,
    Success,
    Error(AgentFilesystemMutationError),
}

async fn resolve_decision(
    runtime: &AgentFilesystemRuntime,
    decision: MutationDecision,
    error: NativeFilesystemError,
    completed: u64,
    failures: usize,
    started: Instant,
    cancelled: bool,
    operation: FilesystemPressureOperation,
) -> DecisionResolution {
    let within_retry_bound = failures < FILESYSTEM_MUTATION_MAX_ATTEMPTS
        && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT;
    match decision {
        MutationDecision::BoundedRetry if cancelled => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Cancelled { completed }))
        }
        MutationDecision::BoundedRetry if within_retry_bound => DecisionResolution::Retry,
        MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Native {
                error,
                completed,
            }))
        }
        MutationDecision::Quota => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::QuotaExhausted {
                error,
                completed,
            }))
        }
        MutationDecision::InsufficientSpace => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::InsufficientSpace {
                error,
                completed,
            }))
        }
        MutationDecision::PhysicalPressure if cancelled => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::Cancelled { completed }))
        }
        MutationDecision::PhysicalPressure
            if within_retry_bound
                && runtime
                    .recover_physical_pressure(
                        operation,
                        started + FILESYSTEM_MUTATION_RETRY_TIMEOUT,
                    )
                    .await
                && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
        {
            DecisionResolution::Retry
        }
        MutationDecision::PhysicalPressure => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::InsufficientSpace {
                error,
                completed,
            }))
        }
        MutationDecision::Success => DecisionResolution::Complete(Ok(completed)),
        MutationDecision::Invalidate => {
            DecisionResolution::Complete(Err(AgentFilesystemMutationError::RuntimeInvalidated {
                error: Some(error),
                completed: Some(completed),
            }))
        }
    }
}

fn attempt_mode(
    mode: AgentFilesystemWriteMode,
    completed: usize,
) -> Result<AgentFilesystemWriteMode, AgentFilesystemMutationError> {
    match mode {
        AgentFilesystemWriteMode::Position(position) => {
            let completed = completed_u64(completed);
            position
                .checked_add(completed)
                .map(AgentFilesystemWriteMode::Position)
                .ok_or(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: None,
                    completed: Some(completed),
                })
        }
        AgentFilesystemWriteMode::Append => Ok(AgentFilesystemWriteMode::Append),
    }
}

fn completed_u64(completed: usize) -> u64 {
    u64::try_from(completed).expect("usize must fit in u64 on supported targets")
}

fn advance_position(
    state: &mut WriterState,
    completed: u64,
) -> Result<(), AgentFilesystemMutationError> {
    if let AgentFilesystemWriteMode::Position(position) = &mut state.mode {
        *position = position.checked_add(completed).ok_or(
            AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: Some(completed),
            },
        )?;
    }
    Ok(())
}

async fn advance_position_or_invalidate(
    runtime: &AgentFilesystemRuntime,
    state: &mut WriterState,
    completed: u64,
) -> Result<(), AgentFilesystemMutationError> {
    match advance_position(state, completed) {
        Ok(()) => Ok(()),
        Err(error) => {
            runtime.invalidate_runtime().await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::agent::AgentFileContentHash;
    use golem_common::model::component::{AgentFilePath, AgentFilePermissions};
    use golem_common::model::diff::Hash;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_r::test;

    struct ScriptedFilesystemWriter {
        attempts: std::sync::Mutex<VecDeque<FilesystemWriteAttempt>>,
        calls: std::sync::Mutex<Vec<(AgentFilesystemWriteMode, Vec<u8>)>>,
        started: Option<Arc<tokio::sync::Notify>>,
        release: Option<Arc<tokio::sync::Semaphore>>,
    }

    struct ScriptedFilesystemResize {
        states: std::sync::Mutex<VecDeque<Result<PathState, i32>>>,
        attempts: std::sync::Mutex<VecDeque<Option<i32>>>,
    }

    struct ScriptedFilesystemSafeReopen {
        attempts: std::sync::Mutex<VecDeque<i32>>,
    }

    #[async_trait]
    impl FilesystemResize for ScriptedFilesystemResize {
        async fn state(&self) -> std::io::Result<PathState> {
            self.states
                .lock()
                .unwrap()
                .pop_front()
                .unwrap()
                .map_err(std::io::Error::from_raw_os_error)
        }

        async fn resize(
            &self,
            _size: u64,
            _effect: Arc<AgentFilesystemUpdateEffectLease>,
        ) -> std::io::Result<()> {
            match self.attempts.lock().unwrap().pop_front().unwrap() {
                Some(errno) => Err(std::io::Error::from_raw_os_error(errno)),
                None => Ok(()),
            }
        }
    }

    #[async_trait]
    impl FilesystemSafeReopen for ScriptedFilesystemSafeReopen {
        async fn reopen(
            &self,
            _effect: Arc<AgentFilesystemUpdateEffectLease>,
        ) -> std::io::Result<NativeOpenResult> {
            Err(std::io::Error::from_raw_os_error(
                self.attempts.lock().unwrap().pop_front().unwrap(),
            ))
        }
    }

    struct PanickingFilesystemWriter;

    #[async_trait]
    impl FilesystemWriter for PanickingFilesystemWriter {
        async fn write(
            &self,
            _mode: AgentFilesystemWriteMode,
            _contents: Bytes,
            _effect: Arc<AgentFilesystemEffectLease>,
        ) -> FilesystemWriteAttempt {
            panic!("scripted native write panic")
        }
    }

    impl ScriptedFilesystemWriter {
        fn new(attempts: impl IntoIterator<Item = FilesystemWriteAttempt>) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                calls: std::sync::Mutex::new(Vec::new()),
                started: None,
                release: None,
            }
        }

        fn blocked(
            attempts: impl IntoIterator<Item = FilesystemWriteAttempt>,
            started: Arc<tokio::sync::Notify>,
            release: Arc<tokio::sync::Semaphore>,
        ) -> Self {
            Self {
                attempts: std::sync::Mutex::new(attempts.into_iter().collect()),
                calls: std::sync::Mutex::new(Vec::new()),
                started: Some(started),
                release: Some(release),
            }
        }

        fn calls(&self) -> Vec<(AgentFilesystemWriteMode, Vec<u8>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl FilesystemWriter for ScriptedFilesystemWriter {
        async fn write(
            &self,
            mode: AgentFilesystemWriteMode,
            contents: Bytes,
            _effect: Arc<AgentFilesystemEffectLease>,
        ) -> FilesystemWriteAttempt {
            let index = {
                let mut calls = self.calls.lock().unwrap();
                let index = calls.len();
                calls.push((mode, contents.to_vec()));
                index
            };
            if index == 0 {
                if let Some(started) = &self.started {
                    started.notify_one();
                }
                if let Some(release) = &self.release {
                    release.acquire().await.unwrap().forget();
                }
            }
            self.attempts.lock().unwrap().pop_front().unwrap()
        }
    }

    fn success(written: usize) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Ok(()),
            failure_effect: None,
        }
    }

    fn failure(written: usize, errno: i32) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Err(std::io::Error::from_raw_os_error(errno)),
            failure_effect: None,
        }
    }

    fn failure_with_effect(
        written: usize,
        errno: i32,
        effect: MutationEffect,
    ) -> FilesystemWriteAttempt {
        FilesystemWriteAttempt {
            written,
            result: Err(std::io::Error::from_raw_os_error(errno)),
            failure_effect: Some(effect),
        }
    }

    fn writer(
        runtime: &AgentFilesystemRuntime,
        native: Arc<ScriptedFilesystemWriter>,
        mode: AgentFilesystemWriteMode,
    ) -> AgentFilesystemWriter {
        runtime.mutations().writer_with_native(native, mode)
    }

    fn path_state(size: u64) -> PathState {
        PathState {
            identity: None,
            type_: PathObjectType::RegularFile,
            size,
        }
    }

    fn writable_directory(path: &std::path::Path) -> Dir {
        Dir::new(
            cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority()).unwrap(),
            DirPerms::all(),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            path.to_path_buf(),
        )
    }

    fn writable_file(path: &std::path::Path) -> File {
        File::new(
            cap_std::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(path)
                    .unwrap(),
            ),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            path.to_path_buf(),
        )
    }

    fn mark_read_only(runtime: &AgentFilesystemRuntime, path: &std::path::Path) {
        runtime.runtime_state.initial_files.write().unwrap().insert(
            path.to_path_buf(),
            InitialAgentFile {
                content_hash: AgentFileContentHash(Hash::empty()),
                path: AgentFilePath::from_abs_str("/read-only").unwrap(),
                permissions: AgentFilePermissions::ReadOnly,
                size: 0,
            },
        );
    }

    #[test]
    async fn descriptor_policy_allows_directories_and_rejects_read_only_files() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let file_path = root.path().join("read-only-file");
        std::fs::write(&file_path, b"contents").unwrap();
        mark_read_only(&runtime, root.path());
        mark_read_only(&runtime, &file_path);
        let mutations = runtime.mutations();

        let directory = Descriptor::Dir(writable_directory(root.path()));
        let admitted = mutations.admit_descriptor_times().await.unwrap();
        let checked = mutations
            .check_descriptor_times_policy(admitted, directory)
            .unwrap();
        drop(checked);

        let file = Descriptor::File(writable_file(&file_path));
        let admitted = mutations.admit_descriptor_times().await.unwrap();
        assert!(matches!(
            mutations.check_descriptor_times_policy(admitted, file),
            Err(AgentFilesystemMutationError::Guest(
                NativeMutationGuestError::NotPermitted
            ))
        ));
    }

    #[test]
    async fn path_policy_rejects_read_only_target_beneath_directory_descriptor() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let target = root.path().join("target");
        std::fs::write(&target, b"contents").unwrap();
        mark_read_only(&runtime, &target);
        let descriptor = Descriptor::Dir(writable_directory(root.path()));
        let mutations = runtime.mutations();
        let admitted = mutations.admit_path_times().await.unwrap();

        assert!(matches!(
            mutations.check_path_times_policy(admitted, descriptor, "target".to_string(), true,),
            Err(AgentFilesystemMutationError::Guest(
                NativeMutationGuestError::NotPermitted
            ))
        ));
    }

    #[test]
    async fn resize_preparation_rejects_substituted_file() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let checked_path = root.path().join("checked");
        let substituted_path = root.path().join("substituted");
        std::fs::write(&checked_path, b"checked").unwrap();
        std::fs::write(&substituted_path, b"substituted").unwrap();
        let checked_file = writable_file(&checked_path);
        let mutations = runtime.mutations();
        let admitted = mutations.admit_resize().await.unwrap();
        let checked = mutations
            .check_resize_policy(admitted, Descriptor::File(checked_file), 2)
            .unwrap();

        assert!(matches!(
            mutations
                .prepare_resize(checked, writable_file(&substituted_path))
                .await,
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn writable_nonmutating_open_checks_read_only_alias_without_admission() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let target = root.path().join("target");
        std::fs::write(&target, b"read only").unwrap();
        std::os::unix::fs::symlink(&target, root.path().join("alias")).unwrap();
        mark_read_only(&runtime, &target);
        let descriptor = Descriptor::Dir(writable_directory(root.path()));
        let mutations = runtime.mutations();

        assert!(!runtime.has_active_effects());
        assert_eq!(
            mutations.check_writable_open_policy(&descriptor, "alias", true),
            Err(AgentFilesystemMutationError::Guest(
                NativeMutationGuestError::NotPermitted,
            ))
        );
        assert_eq!(
            mutations.check_writable_open_policy(&descriptor, "alias", false),
            Ok(())
        );
        assert!(!runtime.has_active_effects());
    }

    #[test]
    async fn successful_write_reports_completed_prefix() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(7),
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn positioned_write_preserves_successful_short_write() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(2)]));

        let result = runtime
            .mutations()
            .positioned_write_with_native(
                Arc::clone(&native) as Arc<dyn FilesystemWriter>,
                7,
                Bytes::from_static(b"hello"),
            )
            .unwrap()
            .await;

        assert_eq!(result, Ok(2));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn positioned_write_preserves_successful_zero_write() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(0)]));

        let result = runtime
            .mutations()
            .positioned_write_with_native(
                Arc::clone(&native) as Arc<dyn FilesystemWriter>,
                7,
                Bytes::from_static(b"hello"),
            )
            .unwrap()
            .await;

        assert_eq!(result, Ok(0));
        assert_eq!(
            native.calls(),
            [(AgentFilesystemWriteMode::Position(7), b"hello".to_vec())]
        );
    }

    #[test]
    async fn semantic_resize_executes_behind_mutation_seam() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("resized");
        std::fs::write(&path, b"hello").unwrap();
        let file = File::new(
            cap_std::fs::File::from_std(
                std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .unwrap(),
            ),
            FilePerms::all(),
            OpenMode::READ | OpenMode::WRITE,
            false,
            path.clone(),
        );

        let mutations = runtime.mutations();
        let admitted = mutations.admit_resize().await.unwrap();
        let checked = mutations
            .check_resize_policy(admitted, Descriptor::File(file.clone()), 2)
            .unwrap();
        let prepared = mutations.prepare_resize(checked, file).await.unwrap();
        let result = mutations.resize(prepared).await;

        assert_eq!(result, Ok(0));
        assert_eq!(std::fs::read(path).unwrap(), b"he");
    }

    #[test]
    async fn semantic_namespace_mutations_execute_behind_mutation_seam() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let directory = writable_directory(root.path());
        let descriptor = Descriptor::Dir(directory.clone());
        let mutations = runtime.mutations();

        let checked = mutations
            .check_create_directory_policy(
                mutations.admit_create_directory().await.unwrap(),
                descriptor.clone(),
                "created".to_string(),
            )
            .unwrap();
        let prepared = mutations
            .prepare_create_directory(checked, directory.clone())
            .unwrap();
        mutations.run_namespace_mutation(prepared).await.unwrap();
        let checked = mutations
            .check_symlink_policy(
                mutations.admit_symlink().await.unwrap(),
                descriptor.clone(),
                "created".to_string(),
                "alias".to_string(),
            )
            .unwrap();
        let prepared = mutations
            .prepare_symlink(checked, directory.clone())
            .unwrap();
        mutations.run_namespace_mutation(prepared).await.unwrap();
        let checked = mutations
            .check_unlink_file_policy(
                mutations.admit_unlink_file().await.unwrap(),
                descriptor.clone(),
                "alias".to_string(),
            )
            .unwrap();
        let prepared = mutations
            .prepare_unlink_file(checked, directory.clone())
            .unwrap();
        mutations.run_namespace_mutation(prepared).await.unwrap();
        let source_checked = mutations
            .check_rename_source_descriptor_policy(
                mutations.admit_rename().await.unwrap(),
                descriptor.clone(),
                "created".to_string(),
            )
            .unwrap();
        let destination_checked = mutations
            .check_rename_destination_descriptor_policy(
                source_checked,
                descriptor.clone(),
                "renamed".to_string(),
            )
            .unwrap();
        let checked = mutations
            .check_rename_path_policy(destination_checked)
            .unwrap();
        let prepared = mutations
            .prepare_rename(checked, directory.clone(), directory.clone())
            .unwrap();
        mutations.run_namespace_mutation(prepared).await.unwrap();
        let checked = mutations
            .check_remove_directory_policy(
                mutations.admit_remove_directory().await.unwrap(),
                descriptor,
                "renamed".to_string(),
            )
            .unwrap();
        let prepared = mutations
            .prepare_remove_directory(checked, directory)
            .unwrap();
        mutations.run_namespace_mutation(prepared).await.unwrap();

        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }

    #[test]
    async fn semantic_mutating_open_returns_descriptor() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let root = tempfile::TempDir::new().unwrap();
        let directory = writable_directory(root.path());
        let options = NativeOpenOptions {
            create: true,
            directory: false,
            exclusive: false,
            truncate: true,
            follow: true,
            read: true,
            write: true,
        };

        let mutations = runtime.mutations();
        let descriptor = Descriptor::Dir(directory.clone());
        let checked = mutations
            .check_mutating_open_policy(
                mutations.admit_mutating_open().await.unwrap(),
                descriptor,
                "opened".to_string(),
                options.follow,
            )
            .unwrap();
        let prepared = mutations
            .prepare_mutating_open(checked, directory, options, false)
            .unwrap();
        let result = mutations.open_mutating(prepared).await.unwrap();

        assert!(matches!(
            result,
            NativeOpenResult::Descriptor(Descriptor::File(_))
        ));
        assert!(root.path().join("opened").is_file());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn terminal_safe_reopen_failure_invalidates_runtime() {
        for errno in [libc::EIO, libc::ESTALE, libc::ENODEV] {
            let runtime = AgentFilesystemRuntime::new_for_test();
            let mutations = runtime.mutations();
            let effect = Arc::new(runtime.begin_update_effect().await.unwrap());
            let native = Arc::new(ScriptedFilesystemSafeReopen {
                attempts: std::sync::Mutex::new([errno].into_iter().collect()),
            });

            let result = mutations
                .safe_reopen(
                    native,
                    effect,
                    FilesystemPressureOperation::Create,
                    0,
                    Instant::now(),
                )
                .await;

            assert!(matches!(
                result,
                Err(AgentFilesystemMutationError::RuntimeInvalidated {
                    error: Some(_),
                    completed: Some(0),
                })
            ));
            assert!(runtime.begin_effect().await.is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn ordinary_safe_reopen_failure_preserves_raw_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mutations = runtime.mutations();
        let effect = Arc::new(runtime.begin_update_effect().await.unwrap());
        let native = Arc::new(ScriptedFilesystemSafeReopen {
            attempts: std::sync::Mutex::new([libc::ENOENT].into_iter().collect()),
        });

        let result = mutations
            .safe_reopen(
                native,
                effect,
                FilesystemPressureOperation::Create,
                0,
                Instant::now(),
            )
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error == Some(libc::ENOENT)
        ));
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[test]
    async fn admitted_write_registers_effect_synchronously() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let admitted = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap();

        assert!(runtime.has_active_effects());
        drop(admitted);
        assert!(!runtime.has_active_effects());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn satisfied_direct_postcondition_turns_native_failure_into_success() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new(
                [Ok(path_state(5)), Ok(path_state(2))].into_iter().collect(),
            ),
            attempts: std::sync::Mutex::new([Some(libc::EBUSY)].into_iter().collect()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert_eq!(result, Ok(0));
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn proven_no_effect_direct_failure_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        runtime.set_retry_callback(Some(Arc::new(|| Box::pin(async { false }))));
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new(
                [Ok(path_state(5)), Ok(path_state(5))].into_iter().collect(),
            ),
            attempts: std::sync::Mutex::new([Some(libc::EBUSY)].into_iter().collect()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn terminal_initial_probe_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemResize {
            states: std::sync::Mutex::new([Err(libc::EIO)].into_iter().collect()),
            attempts: std::sync::Mutex::new(VecDeque::new()),
        });

        let result = runtime
            .mutations()
            .resize_with_scripted_native(native, 2)
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn proven_no_effect_failure_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        runtime.set_retry_callback(Some(Arc::new(|| Box::pin(async { false }))));
        let native = Arc::new(ScriptedFilesystemWriter::new([failure_with_effect(
            0,
            libc::EBUSY,
            MutationEffect::ProvenNoEffect,
        )]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn partial_prefix_is_settled_and_only_suffix_is_retried() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(2, libc::EBUSY),
            success(3),
        ]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(11),
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(
            native.calls(),
            [
                (AgentFilesystemWriteMode::Position(11), b"hello".to_vec()),
                (AgentFilesystemWriteMode::Position(13), b"llo".to_vec()),
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn quota_exhaustion_is_terminal_and_preserves_errno() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
            Some(AgentFilesystemUsage {
                allocated_bytes: 50,
                filesystem_objects: 1,
            }),
            Some(ResolvedAgentFilesystemLimits {
                allocated_bytes: 50,
                filesystem_objects: 10,
                filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
            }),
            capacity,
        );
        let native = Arc::new(ScriptedFilesystemWriter::new([failure(0, libc::ENOSPC)]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::QuotaExhausted { error, completed: 0 })
                if error.raw_os_error() == Some(libc::ENOSPC)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn physical_pressure_recovers_before_retrying() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let recoveries = Arc::new(AtomicUsize::new(0));
        runtime.set_pressure_recovery_callback(Some(Arc::new({
            let recoveries = Arc::clone(&recoveries);
            move |_, _| {
                let recoveries = Arc::clone(&recoveries);
                Box::pin(async move {
                    recoveries.fetch_add(1, Ordering::AcqRel);
                    true
                })
            }
        })));
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(0, libc::ENOSPC),
            success(5),
        ]));

        let result = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"hello"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new())
        .await;

        assert_eq!(result, Ok(5));
        assert_eq!(recoveries.load(Ordering::Acquire), 1);
        assert_eq!(native.calls().len(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn physical_pressure_without_recovery_is_insufficient_space() {
        let capacity = FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 0,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        };
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, capacity);
        let native = Arc::new(ScriptedFilesystemWriter::new([failure(0, libc::ENOSPC)]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::InsufficientSpace { error, completed: 0 })
                if error.raw_os_error() == Some(libc::ENOSPC)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn retry_exhaustion_preserves_native_errno() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([
            failure(0, libc::EBUSY),
            failure(0, libc::EBUSY),
        ]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::Native { error, completed: 0 })
                if error.raw_os_error() == Some(libc::EBUSY)
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn unknown_effect_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([failure_with_effect(
            0,
            libc::EINTR,
            MutationEffect::Unknown,
        )]));

        let result = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new())
            .await;

        assert!(matches!(
            result,
            Err(AgentFilesystemMutationError::RuntimeInvalidated {
                completed: Some(0),
                ..
            })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn cancellation_during_native_completion_retains_prefix() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(2)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let cancellation = tokio_util::sync::CancellationToken::new();
        let completion = writer(&runtime, native, AgentFilesystemWriteMode::Position(0))
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(cancellation.clone());
        started.notified().await;

        cancellation.cancel();
        assert!(runtime.has_active_effects());
        release.add_permits(1);

        assert_eq!(
            completion.await,
            Err(AgentFilesystemMutationError::Cancelled { completed: 2 })
        );
    }

    #[test]
    async fn dropped_awaiter_keeps_native_completion_owned() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&started),
            Arc::clone(&release),
        ));
        let completion = writer(&runtime, native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        started.notified().await;
        drop(completion);

        assert!(runtime.has_active_effects());
        release.add_permits(1);
        tokio::time::timeout(std::time::Duration::from_secs(1), runtime.drain())
            .await
            .unwrap();
    }

    #[test]
    async fn native_task_failure_seals_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let completion = runtime
            .mutations()
            .writer_with_native(
                Arc::new(PanickingFilesystemWriter),
                AgentFilesystemWriteMode::Position(0),
            )
            .admit(Bytes::from_static(b"hello"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());

        assert!(matches!(
            completion.await,
            Err(AgentFilesystemMutationError::RuntimeInvalidated { .. })
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn idle_writer_holds_no_effect_admission() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([]));
        let _writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));

        assert!(!runtime.has_active_effects());
    }

    #[test]
    async fn cancellation_while_append_admission_waits_never_calls_native() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let first_native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let second_native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let first = writer(&runtime, first_native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let second = writer(
            &runtime,
            Arc::clone(&second_native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"other"))
        .unwrap()
        .execute(cancellation.clone());

        cancellation.cancel();
        assert_eq!(
            second.await,
            Err(AgentFilesystemMutationError::Cancelled { completed: 0 })
        );
        assert!(second_native.calls().is_empty());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
    }

    #[test]
    async fn append_coordination_is_shared_across_prepared_writers() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let first_native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let second_native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let first = writer(&runtime, first_native, AgentFilesystemWriteMode::Append)
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let second = writer(
            &runtime,
            Arc::clone(&second_native),
            AgentFilesystemWriteMode::Append,
        )
        .admit(Bytes::from_static(b"other"))
        .unwrap()
        .execute(tokio_util::sync::CancellationToken::new());
        tokio::task::yield_now().await;

        assert!(second_native.calls().is_empty());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        assert_eq!(second.await, Ok(5));
    }

    #[test]
    async fn admitted_chunks_execute_in_admission_order() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5), success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let writer = writer(
            &runtime,
            Arc::clone(&native),
            AgentFilesystemWriteMode::Position(10),
        );
        let first = writer.admit(Bytes::from_static(b"first")).unwrap();
        let second = writer.admit(Bytes::from_static(b"other")).unwrap();
        let second = second.execute(tokio_util::sync::CancellationToken::new());
        let first = first.execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;

        assert_eq!(native.calls().len(), 1);
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        assert_eq!(second.await, Ok(5));
        assert_eq!(
            native.calls(),
            [
                (AgentFilesystemWriteMode::Position(10), b"first".to_vec()),
                (AgentFilesystemWriteMode::Position(15), b"other".to_vec()),
            ]
        );
    }

    #[test]
    async fn dropped_admitted_chunk_does_not_block_later_chunks() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let native = Arc::new(ScriptedFilesystemWriter::new([success(5)]));
        let writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));
        let skipped = writer.admit(Bytes::from_static(b"skip!")).unwrap();
        let next = writer.admit(Bytes::from_static(b"hello")).unwrap();

        drop(skipped);

        assert_eq!(
            next.execute(tokio_util::sync::CancellationToken::new())
                .await,
            Ok(5)
        );
    }

    #[test]
    async fn cancellation_while_writer_sequence_waits_releases_admission() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let first_started = Arc::new(tokio::sync::Notify::new());
        let first_release = Arc::new(tokio::sync::Semaphore::new(0));
        let native = Arc::new(ScriptedFilesystemWriter::blocked(
            [success(5)],
            Arc::clone(&first_started),
            Arc::clone(&first_release),
        ));
        let writer = writer(&runtime, native, AgentFilesystemWriteMode::Position(0));
        let first = writer
            .admit(Bytes::from_static(b"first"))
            .unwrap()
            .execute(tokio_util::sync::CancellationToken::new());
        first_started.notified().await;
        let cancellation = tokio_util::sync::CancellationToken::new();
        let second = writer
            .admit(Bytes::from_static(b"other"))
            .unwrap()
            .execute(cancellation.clone());

        cancellation.cancel();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), second)
                .await
                .unwrap(),
            Err(AgentFilesystemMutationError::Cancelled { completed: 0 })
        );
        assert!(runtime.has_active_effects());
        first_release.add_permits(1);
        assert_eq!(first.await, Ok(5));
        runtime.drain().await;
    }
}
