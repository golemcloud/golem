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

use super::{FilesystemIdentity, FilesystemVolume, space_from_values};
use crate::filesystem_error::FilesystemOperationError;

use crate::metrics::agent_filesystem;
use crate::path_removal;
use crate::storage_volume::{FilesystemAllocation, FilesystemLimits, FilesystemSpace};
use golem_common::model::RetryConfig;
use golem_xfs_quotad::{
    FS_DQUOT_VERSION, FS_PROJ_QUOTA, FS_QSTATV_VERSION1, FsDiskQuota, FsQuotaStatV,
    PROJECT_DATA_LIMIT_FIELDS, PROJECT_LIMIT_FIELDS, XfsQuotadClient,
};
use rustix::fs::{
    AtFlags, FlockOperation, StatVfsMountFlags, StatxAttributes, StatxFlags, flock, fstatfs,
    fstatvfs, ioctl_ficlone, statx,
};
use rustix::ioctl::{Getter, Setter, ioctl};
use std::collections::HashMap;
use std::fmt::Debug;
use std::fs::File;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::ControlFlow;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) const XFS_SUPER_MAGIC: u64 = 0x5846_5342;

// The mount table of the process: one line per mount, in the format of `proc_pid_mountinfo(5)`.
pub(crate) const MOUNT_TABLE: &str = "/proc/self/mountinfo";

// `XFS_FSOP_GEOM_FLAGS_DIRV2CI` of `xfs_fs.h`: names are compared without ASCII case.
pub(crate) const XFS_FSOP_GEOM_FLAGS_DIRV2CI: u32 = 1 << 12;

// `XFS_IOC_FSGEOMETRY_V1` of `xfs_fs.h`: `_IOR('X', 100, struct xfs_fsop_geom_v1)`.
pub(crate) const XFS_IOC_FSGEOMETRY_V1: rustix::ioctl::Opcode =
    rustix::ioctl::opcode::read::<XfsGeometryV1>(b'X', 100);

pub(crate) const XFS_BASIC_BLOCK_BYTES: u64 = 512;

pub(crate) const FS_QUOTA_PDQ_ACCT: u16 = 1 << 4;

pub(crate) const FS_QUOTA_PDQ_ENFD: u16 = 1 << 5;

#[derive(Clone, Copy)]
pub(crate) struct ValidatedXfsNameMode {
    pub(crate) identity: FilesystemIdentity,
}

impl ValidatedXfsNameMode {
    pub(crate) fn matches_device(self, device: u64) -> bool {
        self.identity.device == device
    }
}

#[cfg(test)]
pub(crate) fn validated_xfs_name_mode_for_test(device: u64) -> ValidatedXfsNameMode {
    validated_xfs_name_mode(XFS_SUPER_MAGIC, FilesystemIdentity { device })
        .expect("XFS filesystem type must produce a managed name-mode proof")
}

pub(crate) fn validated_xfs_name_mode(
    filesystem_type: u64,
    identity: FilesystemIdentity,
) -> Option<ValidatedXfsNameMode> {
    (filesystem_type == XFS_SUPER_MAGIC).then_some(ValidatedXfsNameMode { identity })
}

#[derive(Default)]
pub(crate) struct ProjectAllocator {
    pub(crate) next: u32,
    pub(crate) active: HashMap<NonZeroU32, Box<Path>>,
}

/// `struct xfs_fsop_geom_v1` of `xfs_fs.h`, the output of `XFS_IOC_FSGEOMETRY_V1`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct XfsGeometryV1 {
    pub(crate) blocksize: u32,
    pub(crate) rtextsize: u32,
    pub(crate) agblocks: u32,
    pub(crate) agcount: u32,
    pub(crate) logblocks: u32,
    pub(crate) sectsize: u32,
    pub(crate) inodesize: u32,
    pub(crate) imaxpct: u32,
    pub(crate) datablocks: u64,
    pub(crate) rtblocks: u64,
    pub(crate) rtextents: u64,
    pub(crate) logstart: u64,
    pub(crate) uuid: [u8; 16],
    pub(crate) sunit: u32,
    pub(crate) swidth: u32,
    pub(crate) version: i32,
    pub(crate) flags: u32,
    pub(crate) logsectsize: u32,
    pub(crate) rtsectsize: u32,
    pub(crate) dirblocksize: u32,
}

#[derive(Clone)]
pub(crate) struct ProjectQuotas {
    pub(super) root: Arc<Path>,
    pub(super) root_fd: Arc<File>,
    pub(super) quotas: Arc<XfsQuotadClient>,
    allocator: Arc<Mutex<ProjectAllocator>>,
    pub(super) filesystem_block_bytes: NonZeroU64,
    pub(super) validated_name_mode: ValidatedXfsNameMode,
}

/// The root directory of an XFS volume after the checks that every XFS storage mode needs.
///
/// The value holds the exclusive lock of the root for as long as its descriptor is open.
pub(crate) struct XfsRoot {
    /// The root as the configuration gives it. Errors name it.
    pub(crate) root: Box<Path>,
    pub(crate) root_fd: Arc<File>,
    /// The root through the open descriptor, which keeps naming the same directory.
    pub(crate) stable_root: Box<Path>,
    pub(crate) identity: FilesystemIdentity,
    pub(crate) name_mode: ValidatedXfsNameMode,
    pub(crate) filesystem_block_bytes: NonZeroU64,
    /// The filesystem options of the mount, as XFS shows them in the mount table.
    pub(crate) mount_options: Box<str>,
}

impl XfsRoot {
    /// Opens the XFS volume at `root` for one executor.
    ///
    /// The path must be the root directory of an XFS filesystem and the root of its mount. The
    /// filesystem must compare names with their case and have a valid block size. No other
    /// process must hold the lock of the root. The function takes the exclusive lock of the root.
    /// It changes nothing on the volume, and none of its calls needs a privilege.
    pub(crate) fn open(root: &Path) -> Result<Self, FilesystemOperationError> {
        let root_fd = File::open(root)
            .map_err(|error| FilesystemOperationError::io("open XFS root", Some(root), error))?;
        let filesystem = fstatfs(&root_fd).map_err(|error| {
            FilesystemOperationError::io("inspect XFS root", Some(root), errno_to_io(error))
        })?;
        let identity = filesystem_identity(&root_fd).map_err(|error| {
            FilesystemOperationError::io("identify XFS root", Some(root), error)
        })?;
        let name_mode =
            validated_xfs_name_mode(filesystem.f_type as u64, identity).ok_or_else(|| {
                FilesystemOperationError::verification(
                    "validate XFS root filesystem type",
                    Some(root),
                )
            })?;
        let flags = geometry_flags(&root_fd).map_err(|error| {
            FilesystemOperationError::io("inspect XFS root geometry", Some(root), error)
        })?;
        if !names_are_case_sensitive(flags) {
            return Err(FilesystemOperationError::verification(
                "validate XFS names are case-sensitive",
                Some(root),
            ));
        }
        let mount_options = validate_xfs_root_location(&root_fd, root)?;
        flock(&root_fd, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            FilesystemOperationError::io(
                "acquire exclusive ownership of XFS root",
                Some(root),
                errno_to_io(error),
            )
        })?;
        let filesystem_block_bytes = u64::try_from(filesystem.f_bsize)
            .ok()
            .filter(|bytes| *bytes != 0 && bytes.is_multiple_of(XFS_BASIC_BLOCK_BYTES))
            .and_then(NonZeroU64::new)
            .ok_or_else(|| {
                FilesystemOperationError::verification(
                    "validate XFS filesystem block size",
                    Some(root),
                )
            })?;
        let stable_root: Box<Path> =
            PathBuf::from(format!("/proc/self/fd/{}", root_fd.as_raw_fd())).into_boxed_path();
        std::fs::metadata(&stable_root).map_err(|error| {
            FilesystemOperationError::io(
                "open XFS root through its stable descriptor",
                Some(root),
                error,
            )
        })?;
        Ok(Self {
            root: Box::from(root),
            root_fd: Arc::new(root_fd),
            stable_root,
            identity,
            name_mode,
            filesystem_block_bytes,
            mount_options,
        })
    }
}

/// Binds XFS storage with reflink and without project quotas on the opened root `root`: plain
/// agent directories under the root, copied by reflink.
///
/// The root is refused when its mount accounts project quotas, when it has a project id or the
/// project-inherit flag, or when a reflink in it fails. The function never changes the project id
/// or the flags of the root, and none of its calls needs a privilege.
pub(crate) fn bind_reflink(
    root: XfsRoot,
) -> Result<(FilesystemVolume, Arc<Path>, ValidatedXfsNameMode), FilesystemOperationError> {
    verify_no_project_quotas(&root)?;
    probe_reflink(&root)?;
    let XfsRoot {
        root_fd,
        stable_root,
        identity,
        name_mode,
        ..
    } = root;
    agent_filesystem::record_agent_filesystem_volume_opened(false);
    Ok((
        FilesystemVolume::copy_on_write(root_fd, identity),
        Arc::from(stable_root),
        name_mode,
    ))
}

/// Checks that the mount of `root` accounts no project quotas, and that the root has no project
/// id and no project-inherit flag. Then no agent directory and no host directory gets a project.
pub(crate) fn verify_no_project_quotas(root: &XfsRoot) -> Result<(), FilesystemOperationError> {
    if project_quota_accounting(&root.mount_options) == ProjectQuotaAccounting::On {
        return Err(FilesystemOperationError::verification(
            "verify XFS volume has no project quota accounting",
            Some(&root.root),
        ));
    }
    let attributes = get_fsxattr(&root.root_fd).map_err(|error| {
        FilesystemOperationError::io(
            "inspect XFS root project attributes",
            Some(&root.root),
            error,
        )
    })?;
    if has_project_identity(&attributes) {
        return Err(FilesystemOperationError::verification(
            "verify XFS root has no project identity",
            Some(&root.root),
        ));
    }
    Ok(())
}

/// The directory under the root of an XFS volume in which [`probe_reflink`] tests a reflink.
pub(crate) const REFLINK_PROBE: &str = ".golem-xfs-reflink-probe";

/// Checks that a reflink works on the volume of `root`: makes a file under [`REFLINK_PROBE`],
/// reflinks it into a second file, and reads the second file back.
///
/// What an earlier process left under the probe name is removed first. The probe is removed after
/// the check, also when the check fails.
pub(crate) fn probe_reflink(root: &XfsRoot) -> Result<(), FilesystemOperationError> {
    let probe = root.stable_root.join(REFLINK_PROBE);
    let configured_probe = root.root.join(REFLINK_PROBE);
    remove_probe(&probe, &configured_probe, "remove stale XFS reflink probe")?;
    std::fs::create_dir(&probe).map_err(|error| {
        FilesystemOperationError::io("create XFS reflink probe", Some(&configured_probe), error)
    })?;
    let probed = reflink_probe_contents(&probe);
    let removed = remove_probe(&probe, &configured_probe, "remove XFS reflink probe");
    probed
        .map_err(|error| {
            FilesystemOperationError::io(
                "validate XFS reflink support",
                Some(&configured_probe),
                error,
            )
        })
        .and(removed)
}

fn remove_probe(
    path: &Path,
    configured: &Path,
    operation: &'static str,
) -> Result<(), FilesystemOperationError> {
    path_removal::remove_and_verify(path)
        .map_err(|failure| FilesystemOperationError::removal(operation, configured, failure))
}

/// Writes a file in the empty directory `probe`, reflinks it into a new file, and checks that the
/// new file holds the same bytes.
pub(crate) fn reflink_probe_contents(probe: &Path) -> std::io::Result<()> {
    const CONTENTS: &[u8] = b"golem-xfs-reflink-probe";
    let mut create = std::fs::OpenOptions::new();
    create.read(true).write(true).create_new(true);
    let mut source = create.open(probe.join("source"))?;
    std::io::Write::write_all(&mut source, CONTENTS)?;
    let target = create.open(probe.join("target"))?;
    ioctl_ficlone(&target, &source).map_err(errno_to_io)?;
    if std::fs::read(probe.join("target"))? != CONTENTS {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "XFS reflink did not preserve probe contents",
        ));
    }
    Ok(())
}

impl ProjectQuotas {
    /// Binds managed XFS storage with project quotas on the opened root `root`. The quota calls go
    /// to `xfs-quotad` on `xfs_quotad_socket`.
    ///
    /// The function removes any project from the root, checks that project quota accounting and
    /// enforcement are on and that the helper may query and change project quotas, and probes
    /// project inheritance and reflink.
    pub(crate) fn new(
        root: XfsRoot,
        xfs_quotad_socket: &Path,
        cleanup_retry: &RetryConfig,
    ) -> Result<Self, FilesystemOperationError> {
        let XfsRoot {
            root: configured_root,
            root_fd,
            stable_root,
            name_mode,
            filesystem_block_bytes,
            ..
        } = root;
        let quotas = XfsQuotadClient::connect(xfs_quotad_socket, name_mode.identity.device)
            .map(Arc::new)
            .map_err(|error| {
                FilesystemOperationError::io("connect to xfs-quotad", Some(&configured_root), error)
            })?;
        clear_root_project_assignment(&root_fd, &configured_root)?;
        agent_filesystem::record_agent_filesystem_volume_opened(true);
        let backend = Self {
            root: Arc::from(stable_root),
            root_fd,
            quotas,
            allocator: Arc::new(Mutex::new(ProjectAllocator {
                next: 1,
                active: HashMap::new(),
            })),
            filesystem_block_bytes,
            validated_name_mode: name_mode,
        };
        backend.validate_project_quota_state()?;
        backend.validate_project_assignment(cleanup_retry)?;

        Ok(backend)
    }

    pub(crate) fn root(&self) -> &Arc<Path> {
        &self.root
    }

    pub(crate) fn filesystem_block_bytes(&self) -> NonZeroU64 {
        self.filesystem_block_bytes
    }

    pub(crate) fn validated_name_mode(&self) -> ValidatedXfsNameMode {
        self.validated_name_mode
    }

    pub(crate) fn project_id(&self, file: &File) -> std::io::Result<Option<NonZeroU32>> {
        let attributes = get_fsxattr(file)?;
        Ok(NonZeroU32::new(attributes.fsx_projid))
    }

    pub(crate) fn root_directory(&self) -> &File {
        &self.root_fd
    }

    pub(crate) fn reserved_project(&self, owner: &Path) -> Option<NonZeroU32> {
        let allocator = self
            .allocator
            .lock()
            .expect("XFS project allocator lock poisoned");
        project_of_owner(&allocator.active, owner)
    }

    pub(crate) fn reserve_existing_project(
        &self,
        project_id: NonZeroU32,
        owner: &Path,
    ) -> std::io::Result<()> {
        let mut allocator = self
            .allocator
            .lock()
            .expect("XFS project allocator lock poisoned");
        match existing_reservation(&allocator.active, project_id, owner) {
            ExistingReservation::Held => Ok(()),
            ExistingReservation::HeldByOther(active_owner) => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "XFS project {project_id} is owned by {}",
                    active_owner.display()
                ),
            )),
            ExistingReservation::Free => {
                allocator.active.insert(project_id, owner.into());
                Ok(())
            }
        }
    }

    pub(crate) fn reserve_project(&self, owner: &Path) -> std::io::Result<NonZeroU32> {
        // Each attempt reserves a new candidate, so the scan stops when it reaches its first
        // candidate again. When another caller holds that candidate at that moment, the bound of
        // 2^32 attempts stops it with the same error.
        let outcome = (0..=u32::MAX).try_fold(None, |first, _| {
            let project_id = match self.reserve_project_candidate(owner) {
                Ok(project_id) => project_id,
                Err(error) => return ControlFlow::Break(Err(error)),
            };
            if candidate_repeats(first, project_id) {
                self.release_project(project_id);
                return ControlFlow::Break(Err(no_reusable_projects()));
            }
            match self.prepare_project(project_id) {
                Ok(true) => ControlFlow::Break(Ok(project_id)),
                Ok(false) => {
                    self.release_project(project_id);
                    ControlFlow::Continue(Some(first.unwrap_or(project_id)))
                }
                Err(error) => {
                    self.release_project(project_id);
                    ControlFlow::Break(Err(error))
                }
            }
        });
        match outcome {
            ControlFlow::Break(result) => result,
            ControlFlow::Continue(_) => Err(no_reusable_projects()),
        }
    }

    /// Makes the reserved project `project_id` ready for a new owner when it holds nothing:
    /// clears its limits, and gives true. Gives false when it still holds something. This is the
    /// sequence of native calls; the decision is [`project_is_empty`].
    fn prepare_project(&self, project_id: NonZeroU32) -> std::io::Result<bool> {
        if !project_is_empty(self.project_usage(project_id.get())?) {
            return Ok(false);
        }
        self.clear_project_limits(project_id)?;
        Ok(true)
    }

    /// Reserves the next project that no owner holds for `owner`. The decision, the reservation and
    /// the next scan start are made under one lock, so a concurrent caller sees the reservation.
    fn reserve_project_candidate(&self, owner: &Path) -> std::io::Result<NonZeroU32> {
        let mut allocator = self
            .allocator
            .lock()
            .expect("XFS project allocator lock poisoned");
        let project_id = next_free_project(allocator.next, &allocator.active)
            .ok_or_else(no_reusable_projects)?;
        allocator.active.insert(project_id, owner.into());
        allocator.next = project_after(project_id);
        Ok(project_id)
    }

    pub(crate) fn release_project(&self, project_id: NonZeroU32) {
        self.allocator
            .lock()
            .expect("XFS project allocator lock poisoned")
            .active
            .remove(&project_id);
    }

    pub(crate) fn assign_project(
        &self,
        file: &File,
        project_id: NonZeroU32,
    ) -> std::io::Result<()> {
        set_project(file, project_id)?;
        validate_project_attributes(get_fsxattr(file)?, project_id)
    }

    pub(crate) fn finish_project_cleanup(&self, project_id: NonZeroU32) -> std::io::Result<()> {
        let mut usage = self.project_usage(project_id.get())?;
        if !project_is_empty(usage) {
            rustix::fs::syncfs(&self.root_fd).map_err(errno_to_io)?;
            usage = self.project_usage(project_id.get())?;
        }
        if !project_is_empty(usage) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                format!(
                    "XFS project {project_id} still owns {} bytes and {} filesystem objects",
                    usage.allocated_bytes, usage.filesystem_objects
                ),
            ));
        }
        self.clear_project_limits(project_id)
    }

    fn validate_project_quota_state(&self) -> Result<(), FilesystemOperationError> {
        self.observe_project_quota_state().map_err(|error| {
            FilesystemOperationError::io(
                "validate managed XFS project quota accounting and enforcement",
                Some(&self.root),
                error,
            )
        })?;

        // Querying project zero proves that the executor has quota-query
        // privileges without assigning a reusable project identity.
        self.project_usage(0).map_err(|error| {
            FilesystemOperationError::io(
                "validate managed XFS project quota query permissions",
                Some(&self.root),
                error,
            )
        })?;
        Ok(())
    }

    fn observe_project_quota_state(&self) -> std::io::Result<()> {
        let mut state = FsQuotaStatV {
            qs_version: FS_QSTATV_VERSION1,
            ..FsQuotaStatV::default()
        };
        self.get_quota_state(&mut state)?;
        validate_project_quota_state_record(&state)
    }

    fn validate_project_assignment(
        &self,
        cleanup_retry: &RetryConfig,
    ) -> Result<(), FilesystemOperationError> {
        const PROBE_PROJECT_ID: u32 = 0x8000_0000;
        let project_id = NonZeroU32::new(PROBE_PROJECT_ID).unwrap();
        let probe = self.root.join(".golem-xfs-project-probe");
        if probe.exists() {
            std::fs::remove_dir_all(&probe).map_err(|error| {
                FilesystemOperationError::cleanup_io(
                    "remove stale managed XFS startup probe",
                    Some(&probe),
                    error,
                )
            })?;
        }

        let usage = self.project_usage(project_id.get()).map_err(|error| {
            FilesystemOperationError::io(
                "validate 32-bit XFS project quota query",
                Some(&self.root),
                error,
            )
        })?;
        if !project_is_empty(usage) {
            return Err(FilesystemOperationError::verification(
                "reserve unused 32-bit XFS startup probe project",
                Some(&self.root),
            ));
        }
        self.clear_project_limits(project_id).map_err(|error| {
            FilesystemOperationError::io(
                "validate XFS project quota update permissions",
                Some(&self.root),
                error,
            )
        })?;
        std::fs::create_dir(&probe).map_err(|error| {
            FilesystemOperationError::io("create managed XFS startup probe", Some(&probe), error)
        })?;
        if let Err(error) = self.probe_project_inheritance(&probe, project_id) {
            let _ = std::fs::remove_dir_all(&probe);
            return Err(FilesystemOperationError::io(
                "validate managed XFS project assignment and inheritance",
                Some(&probe),
                error,
            ));
        }
        std::fs::remove_dir_all(&probe).map_err(|error| {
            FilesystemOperationError::cleanup_io(
                "remove managed XFS startup probe",
                Some(&probe),
                error,
            )
        })?;
        self.finish_project_cleanup_with_retry(project_id, cleanup_retry)
            .map_err(|error| {
                FilesystemOperationError::cleanup_io(
                    "clear managed XFS startup probe project",
                    Some(&probe),
                    error,
                )
            })
    }

    fn probe_project_inheritance(
        &self,
        probe: &Path,
        project_id: NonZeroU32,
    ) -> std::io::Result<()> {
        let probe_directory = File::open(probe)?;
        self.assign_project(&probe_directory, project_id)?;
        let child = probe.join("child");
        std::fs::create_dir(&child)?;
        let child = File::open(&child)?;
        let child_id = self.project_id(&child)?;
        if child_id != Some(project_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS child did not inherit its project identity",
            ));
        }
        let source_path = probe.join("reflink-source");
        let destination_path = probe.join("reflink-destination");
        std::fs::write(&source_path, b"golem-xfs-reflink-probe")?;
        let source = File::open(&source_path)?;
        let destination = File::create(&destination_path)?;
        ioctl_ficlone(&destination, &source).map_err(errno_to_io)?;
        if std::fs::read(&destination_path)? != b"golem-xfs-reflink-probe" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS reflink did not preserve probe contents",
            ));
        }
        Ok(())
    }

    fn project_usage(&self, project_id: u32) -> std::io::Result<FilesystemAllocation> {
        let quota = self.project_quota(project_id)?;
        usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)
    }

    fn project_quota(&self, project_id: u32) -> std::io::Result<FsDiskQuota> {
        let mut quota = FsDiskQuota::default();
        if let Err(error) = self.get_project_quota(project_id, &mut quota) {
            if quota_record_is_missing(&error) {
                return Ok(empty_project_quota(project_id));
            }
            return Err(error);
        }
        if !project_quota_record_is_valid(&quota, project_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS returned an invalid project quota record",
            ));
        }
        Ok(quota)
    }

    fn clear_project_limits(&self, project_id: NonZeroU32) -> std::io::Result<()> {
        let quota = FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: PROJECT_LIMIT_FIELDS,
            d_id: project_id.get(),
            ..FsDiskQuota::default()
        };
        self.quotas
            .set_project_quota_record(project_id.get(), &quota)?;

        let mut cleared = FsDiskQuota::default();
        if let Err(error) = self.get_project_quota(project_id.get(), &mut cleared) {
            if quota_record_is_missing(&error) {
                return Ok(());
            }
            return Err(error);
        }
        if project_quota_retains_state(&cleared) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("XFS project {project_id} retained quota limits or state"),
            ));
        }
        Ok(())
    }

    fn finish_project_cleanup_with_retry(
        &self,
        project_id: NonZeroU32,
        cleanup_retry: &RetryConfig,
    ) -> std::io::Result<()> {
        let attempts = cleanup_retry.max_attempts.max(1);
        let mut delay = cleanup_retry.min_delay;
        for attempt in 1..=attempts {
            match self.finish_project_cleanup(project_id) {
                Ok(()) => return Ok(()),
                Err(error) if attempt == attempts => return Err(error),
                Err(_) => {
                    std::thread::sleep(delay);
                    delay = delay
                        .mul_f64(cleanup_retry.multiplier)
                        .min(cleanup_retry.max_delay);
                }
            }
        }
        unreachable!("managed XFS cleanup always performs at least one attempt")
    }

    fn get_project_quota(&self, project_id: u32, quota: &mut FsDiskQuota) -> std::io::Result<()> {
        self.quotas.get_project_quota(project_id, quota)
    }

    fn get_quota_state(&self, state: &mut FsQuotaStatV) -> std::io::Result<()> {
        self.quotas.get_quota_state(state)
    }
}

/// The project that `owner` holds in `active`, if any.
pub(crate) fn project_of_owner(
    active: &HashMap<NonZeroU32, Box<Path>>,
    owner: &Path,
) -> Option<NonZeroU32> {
    active
        .iter()
        .find_map(|(project_id, active_owner)| (&**active_owner == owner).then_some(*project_id))
}

/// What a reservation of an existing project finds in the active reservations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExistingReservation<'a> {
    /// The owner already holds the project.
    Held,
    /// Another owner holds the project.
    HeldByOther(&'a Path),
    /// No owner holds the project.
    Free,
}

/// What a reservation of `project_id` for `owner` finds in `active`.
pub(crate) fn existing_reservation<'a>(
    active: &'a HashMap<NonZeroU32, Box<Path>>,
    project_id: NonZeroU32,
    owner: &Path,
) -> ExistingReservation<'a> {
    match active.get(&project_id) {
        Some(active_owner) if &**active_owner == owner => ExistingReservation::Held,
        Some(active_owner) => ExistingReservation::HeldByOther(active_owner),
        None => ExistingReservation::Free,
    }
}

/// The first project that `active` does not hold, scanning from `next` (0 counts as 1) up to
/// `u32::MAX`, and then from 1 up to the start. `None` when every project is held.
pub(crate) fn next_free_project(
    next: u32,
    active: &HashMap<NonZeroU32, Box<Path>>,
) -> Option<NonZeroU32> {
    let first = next.max(1);
    (first..=u32::MAX)
        .chain(1..first)
        .filter_map(NonZeroU32::new)
        .find(|project_id| !active.contains_key(project_id))
}

/// Where the next scan starts after `project_id` is reserved: the next project, and 1 after
/// `u32::MAX`.
pub(crate) fn project_after(project_id: NonZeroU32) -> u32 {
    project_id.get().checked_add(1).unwrap_or(1)
}

/// Whether the reservation loop got back to `first`, the first candidate it reserved, so it went
/// once around the project ids.
pub(crate) fn candidate_repeats(first: Option<NonZeroU32>, candidate: NonZeroU32) -> bool {
    first == Some(candidate)
}

/// Whether a project with `usage` holds nothing, so it can be given to a new owner.
pub(crate) fn project_is_empty(usage: FilesystemAllocation) -> bool {
    usage.allocated_bytes == 0 && usage.filesystem_objects == 0
}

/// Whether a read of a project quota record failed because XFS has no record for the project.
pub(crate) fn quota_record_is_missing(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
}

/// The quota record of a project for which XFS has no record: no limits, no usage.
pub(crate) fn empty_project_quota(project_id: u32) -> FsDiskQuota {
    FsDiskQuota {
        d_version: FS_DQUOT_VERSION,
        d_flags: FS_PROJ_QUOTA,
        d_id: project_id,
        ..FsDiskQuota::default()
    }
}

/// Whether `quota` is a project quota record of the expected version for `project_id`.
pub(crate) fn project_quota_record_is_valid(quota: &FsDiskQuota, project_id: u32) -> bool {
    quota.d_version == FS_DQUOT_VERSION
        && quota.d_flags == FS_PROJ_QUOTA
        && quota.d_id == project_id
}

/// Whether `quota` still holds a limit, a timer or a warning count after its limits were cleared.
pub(crate) fn project_quota_retains_state(quota: &FsDiskQuota) -> bool {
    quota.d_blk_hardlimit != 0
        || quota.d_blk_softlimit != 0
        || quota.d_ino_hardlimit != 0
        || quota.d_ino_softlimit != 0
        || quota.d_rtb_hardlimit != 0
        || quota.d_rtb_softlimit != 0
        || quota.d_itimer != 0
        || quota.d_btimer != 0
        || quota.d_rtbtimer != 0
        || quota.d_iwarns != 0
        || quota.d_bwarns != 0
        || quota.d_rtbwarns != 0
        || quota.d_itimer_hi != 0
        || quota.d_btimer_hi != 0
        || quota.d_rtbtimer_hi != 0
}

/// Whether XFS project quotas can hold `limits` exactly: both limits are positive and the byte
/// limit is a whole number of filesystem blocks.
pub(crate) fn limits_are_representable(
    limits: FilesystemLimits,
    filesystem_block_bytes: NonZeroU64,
) -> bool {
    limits.allocated_bytes != 0
        && limits
            .allocated_bytes
            .is_multiple_of(filesystem_block_bytes.get())
        && limits.filesystem_objects != 0
}

/// Whether `installed` holds the block hard limit `block_hard_limit` and the inode hard limit
/// `inode_hard_limit`, and no soft limit.
pub(crate) fn project_quota_holds_limits(
    installed: &FsDiskQuota,
    block_hard_limit: u64,
    inode_hard_limit: u64,
) -> bool {
    installed.d_blk_hardlimit == block_hard_limit
        && installed.d_blk_softlimit == 0
        && installed.d_ino_hardlimit == inode_hard_limit
        && installed.d_ino_softlimit == 0
}

pub(crate) fn no_reusable_projects() -> std::io::Error {
    std::io::Error::other("no reusable XFS project IDs are available")
}

// The root must have no project id and no inheritance before anything is created under it,
// so a host directory made under it belongs to no project.
pub(crate) fn clear_root_project_assignment(
    root_fd: &File,
    root: &Path,
) -> Result<(), FilesystemOperationError> {
    let mut attributes = get_fsxattr(root_fd).map_err(|error| {
        FilesystemOperationError::io(
            "inspect managed XFS root project attributes",
            Some(root),
            error,
        )
    })?;
    // Linux lets only the owner of the root change its attributes, and the root of a new volume
    // belongs to root. A root without a project needs no change, so the executor needs no
    // ownership of it.
    if !has_project_identity(&attributes) {
        return Ok(());
    }
    attributes.fsx_projid = 0;
    attributes.fsx_xflags &= !linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
    attributes.fsx_pad = [0; 8];
    set_fsxattr(root_fd, attributes).map_err(|error| {
        FilesystemOperationError::io(
            "clear managed XFS root project inheritance",
            Some(root),
            error,
        )
    })?;
    let assigned = get_fsxattr(root_fd).map_err(|error| {
        FilesystemOperationError::io(
            "verify managed XFS root project attributes",
            Some(root),
            error,
        )
    })?;
    if has_project_identity(&assigned) {
        return Err(FilesystemOperationError::verification(
            "verify managed XFS root has neutral project identity",
            Some(root),
        ));
    }
    Ok(())
}

/// Gives the project of a stale managed sandbox path from the project on disk and the project
/// that this process reserved for the path.
///
/// The stale project is the one that exists. When both exist and differ, the result is
/// [`ProjectMismatch`].
pub(crate) fn stale_project(
    disk: Option<NonZeroU32>,
    reserved: Option<NonZeroU32>,
) -> Result<Option<NonZeroU32>, ProjectMismatch> {
    match (disk, reserved) {
        (Some(disk), Some(reserved)) if disk != reserved => Err(ProjectMismatch),
        (disk, reserved) => Ok(disk.or(reserved)),
    }
}

/// The project on disk and the project that this process reserved for a path differ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProjectMismatch;

/// Whether a directory has a project id or gives one to what is made in it.
pub(crate) fn has_project_identity(attributes: &linux_raw_sys::general::fsxattr) -> bool {
    attributes.fsx_projid != 0
        || attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT != 0
}

/// Checks that a host directory has no project id and gives no project id to what is made in it.
pub(crate) fn verify_host_directory_has_no_project(
    path: &Path,
) -> Result<(), FilesystemOperationError> {
    let directory = File::open(path).map_err(|error| {
        FilesystemOperationError::io("open XFS host directory", Some(path), error)
    })?;
    let attributes = get_fsxattr(&directory).map_err(|error| {
        FilesystemOperationError::io(
            "inspect XFS host directory project attributes",
            Some(path),
            error,
        )
    })?;
    if has_project_identity(&attributes) {
        return Err(FilesystemOperationError::verification(
            "verify XFS host directory has no project identity",
            Some(path),
        ));
    }
    Ok(())
}

/// Checks that `root` is the root directory of its filesystem and the root of its mount, and gives
/// the filesystem options of the mount.
pub(crate) fn validate_xfs_root_location(
    root_fd: &File,
    root: &Path,
) -> Result<Box<str>, FilesystemOperationError> {
    let location = statx(
        root_fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID,
    )
    .map_err(|error| {
        FilesystemOperationError::io(
            "inspect XFS root mount location",
            Some(root),
            errno_to_io(error),
        )
    })?;
    let mount_table = std::fs::read_to_string(MOUNT_TABLE).map_err(|error| {
        FilesystemOperationError::io("read the mount table", Some(Path::new(MOUNT_TABLE)), error)
    })?;
    let mount_id =
        (location.stx_mask & StatxFlags::MNT_ID.bits() != 0).then_some(location.stx_mnt_id);
    match mount_id.and_then(|id| mount_line(&mount_table, id)) {
        Some(line)
            if xfs_root_location_is_valid(
                location.stx_attributes,
                location.stx_attributes_mask,
                Some(line),
            ) =>
        {
            Ok(Box::from(line.super_options))
        }
        _ => Err(FilesystemOperationError::verification(
            "validate XFS root is the filesystem mount root",
            Some(root),
        )),
    }
}

/// Whether an XFS mount accounts project quotas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectQuotaAccounting {
    On,
    Off,
}

/// The fields of one line of `/proc/self/mountinfo` that the root checks read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MountLine<'a> {
    /// The directory of the filesystem that is the root of the mount.
    pub(crate) root: &'a str,
    /// The options of the filesystem, as the filesystem shows them.
    pub(crate) super_options: &'a str,
}

/// Whether a directory is the root directory of its filesystem and is mounted there.
///
/// The directory must be a mount root, and `line`, the line of its mount in the mount table, must
/// give `/` as the root of the mount within its filesystem. A bind mount of a subdirectory is a
/// mount root too, but its mount has another root. A directory without a known mount line is not
/// valid.
pub(crate) fn xfs_root_location_is_valid(
    attributes: StatxAttributes,
    attributes_mask: StatxAttributes,
    line: Option<MountLine<'_>>,
) -> bool {
    attributes_mask.contains(StatxAttributes::MOUNT_ROOT)
        && attributes.contains(StatxAttributes::MOUNT_ROOT)
        && line.is_some_and(|line| line.root == "/")
}

/// Gives the line of the mount `mount_id` in `mount_table`, which has the format of
/// `/proc/self/mountinfo`: the mount id is the first field, the root of the mount the fourth, and
/// the filesystem options the third field after the separator `-`.
pub(crate) fn mount_line(mount_table: &str, mount_id: u64) -> Option<MountLine<'_>> {
    mount_table.lines().find_map(|line| {
        let (mount_fields, filesystem_fields) = line.split_once(" - ")?;
        let mut fields = mount_fields.split(' ');
        let id = fields.next()?.parse::<u64>().ok()?;
        let root = fields.nth(2)?;
        let super_options = filesystem_fields.split(' ').nth(2)?;
        (id == mount_id).then_some(MountLine {
            root,
            super_options,
        })
    })
}

/// Whether the XFS filesystem options `super_options` account project quotas. XFS shows
/// `prjquota` when it accounts and enforces them, and `pqnoenforce` when it only accounts them.
pub(crate) fn project_quota_accounting(super_options: &str) -> ProjectQuotaAccounting {
    if super_options
        .split(',')
        .any(|option| option == "prjquota" || option == "pqnoenforce")
    {
        ProjectQuotaAccounting::On
    } else {
        ProjectQuotaAccounting::Off
    }
}

/// Whether an XFS volume with the superblock flags `geometry_flags` compares names with their
/// case. A volume made with `mkfs.xfs -n version=ci` folds the case of ASCII names.
pub(crate) fn names_are_case_sensitive(geometry_flags: u32) -> bool {
    geometry_flags & XFS_FSOP_GEOM_FLAGS_DIRV2CI == 0
}

/// Reads the superblock flags of the XFS volume that holds `file`. The call needs no privilege.
pub(crate) fn geometry_flags(file: &File) -> std::io::Result<u32> {
    // SAFETY: The opcode encodes the size of `XfsGeometryV1`, which has the layout of
    // `struct xfs_fsop_geom_v1`, and the kernel writes the complete structure.
    unsafe {
        ioctl(
            file,
            Getter::<{ XFS_IOC_FSGEOMETRY_V1 }, XfsGeometryV1>::new(),
        )
    }
    .map(|geometry| geometry.flags)
    .map_err(errno_to_io)
}

pub(crate) fn observe_space(
    root: &File,
    identity: FilesystemIdentity,
) -> std::io::Result<FilesystemSpace> {
    validate_filesystem_identity(root, identity)?;
    let capacity = fstatvfs(root).map_err(errno_to_io)?;
    if capacity.f_flag.contains(StatVfsMountFlags::RDONLY) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ReadOnlyFilesystem,
            "XFS volume is mounted read-only",
        ));
    }
    space_from_values(
        capacity.f_blocks,
        capacity.f_bavail,
        capacity.f_frsize,
        capacity.f_files,
        capacity.f_ffree,
    )
}

pub(crate) fn filesystem_identity(root: &File) -> std::io::Result<FilesystemIdentity> {
    Ok(FilesystemIdentity {
        device: root.metadata()?.dev(),
    })
}

pub(crate) fn validate_filesystem_identity(
    root: &File,
    expected: FilesystemIdentity,
) -> std::io::Result<()> {
    let observed = filesystem_identity(root)?;
    if observed == expected {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "XFS volume identity changed",
        ))
    }
}

pub(crate) fn assign_project(file: &File, project_id: NonZeroU32) -> std::io::Result<()> {
    set_project(file, project_id)?;
    validate_project_attributes(get_fsxattr(file)?, project_id)
}

#[cfg(test)]
pub(crate) fn file_project_id(file: &File) -> std::io::Result<Option<NonZeroU32>> {
    Ok(NonZeroU32::new(get_fsxattr(file)?.fsx_projid))
}

// The kernel default copy-on-write extent size hint, in filesystem blocks, for a volume whose
// root inode carries no hint of its own.
#[cfg(test)]
pub(crate) const XFS_DEFAULT_COW_EXTENT_SIZE_BLOCKS: u64 = 32;

/// Reads the copy-on-write extent size hint that a write into a shared extent of this volume
/// allocates: the hint on the root inode when it has one, else the kernel default.
#[cfg(test)]
pub(crate) fn cow_extent_size_hint(
    root: &File,
    filesystem_block_bytes: u64,
) -> std::io::Result<u64> {
    let attributes = get_fsxattr(root)?;
    if attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_COWEXTSIZE != 0
        && attributes.fsx_cowextsize != 0
    {
        Ok(u64::from(attributes.fsx_cowextsize))
    } else {
        Ok(XFS_DEFAULT_COW_EXTENT_SIZE_BLOCKS * filesystem_block_bytes)
    }
}

/// Makes `target` share the extents of `source`. The kernel writes the page cache of `source`
/// back before it copies the extents, so an unsynced write is in the clone.
pub(crate) fn clone_file(target: &File, source: &File) -> std::io::Result<()> {
    ioctl_ficlone(target, source).map_err(errno_to_io)
}

/// Makes the new file `target` share the extents of `source`.
///
/// `target` must have `project_id`, which it gets from its directory, so that the extents are
/// charged to that project. A target with another project gives an `InvalidData` error.
pub(crate) fn reflink_into_project(
    project_id: NonZeroU32,
    target: &File,
    source: &File,
) -> std::io::Result<()> {
    if NonZeroU32::new(get_fsxattr(target)?.fsx_projid) != Some(project_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed XFS file-copy destination did not inherit its project identity",
        ));
    }
    ioctl_ficlone(target, source).map_err(errno_to_io)
}

pub(crate) fn project_allocation(
    volume: &FilesystemVolume,
    project_id: NonZeroU32,
) -> std::io::Result<FilesystemAllocation> {
    let quota = project_quota(volume_quotas(volume), project_id.get())?;
    usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)
}

pub(crate) fn install_project_limits(
    volume: &FilesystemVolume,
    project_id: NonZeroU32,
    filesystem_block_bytes: NonZeroU64,
    limits: FilesystemLimits,
) -> std::io::Result<()> {
    if !limits_are_representable(limits, filesystem_block_bytes) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "filesystem limits are not exactly representable by managed XFS",
        ));
    }
    let block_hard_limit = limits.allocated_bytes / XFS_BASIC_BLOCK_BYTES;
    let quota = FsDiskQuota {
        d_version: FS_DQUOT_VERSION,
        d_flags: FS_PROJ_QUOTA,
        d_fieldmask: PROJECT_DATA_LIMIT_FIELDS,
        d_id: project_id.get(),
        d_blk_hardlimit: block_hard_limit,
        d_ino_hardlimit: limits.filesystem_objects,
        ..FsDiskQuota::default()
    };
    let quotas = volume_quotas(volume);
    quotas.set_project_quota_record(project_id.get(), &quota)?;

    let installed = project_quota(quotas, project_id.get())?;
    if !project_quota_holds_limits(&installed, block_hard_limit, limits.filesystem_objects) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("XFS project {project_id} did not retain the complete quota limit pair"),
        ));
    }
    Ok(())
}

pub(crate) fn volume_quotas(volume: &FilesystemVolume) -> &XfsQuotadClient {
    volume
        .quota_client()
        .expect("managed XFS operation requires a managed volume")
}

pub(crate) fn project_quota(
    quotas: &XfsQuotadClient,
    project_id: u32,
) -> std::io::Result<FsDiskQuota> {
    let mut quota = FsDiskQuota::default();
    if let Err(error) = quotas.get_project_quota(project_id, &mut quota) {
        if quota_record_is_missing(&error) {
            return Ok(empty_project_quota(project_id));
        }
        return Err(error);
    }
    if !project_quota_record_is_valid(&quota, project_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "XFS returned an invalid project quota record",
        ));
    }
    Ok(quota)
}

pub(crate) fn get_fsxattr(file: &File) -> std::io::Result<linux_raw_sys::general::fsxattr> {
    // SAFETY: The generated opcode and `fsxattr` type come from the same Linux
    // UAPI version and the kernel initializes the complete output structure.
    unsafe {
        ioctl(
            file,
            Getter::<
                { linux_raw_sys::ioctl::FS_IOC_FSGETXATTR as rustix::ioctl::Opcode },
                linux_raw_sys::general::fsxattr,
            >::new(),
        )
    }
    .map_err(errno_to_io)
}

pub(crate) fn validate_project_attributes(
    attributes: linux_raw_sys::general::fsxattr,
    project_id: NonZeroU32,
) -> std::io::Result<()> {
    if attributes.fsx_projid != project_id.get()
        || attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT == 0
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "XFS project identity or inheritance did not persist",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_project_quota_state_record(state: &FsQuotaStatV) -> std::io::Result<()> {
    if state.qs_version != FS_QSTATV_VERSION1
        || state.qs_flags & (FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD)
            != FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed XFS project quota accounting or enforcement is disabled",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn set_project(file: &File, project_id: NonZeroU32) -> std::io::Result<()> {
    let mut attributes = get_fsxattr(file)?;
    attributes.fsx_projid = project_id.get();
    attributes.fsx_xflags |= linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
    attributes.fsx_pad = [0; 8];
    set_fsxattr(file, attributes)
}

pub(crate) fn set_fsxattr(
    file: &File,
    attributes: linux_raw_sys::general::fsxattr,
) -> std::io::Result<()> {
    // SAFETY: The generated opcode and `fsxattr` type come from the same Linux
    // UAPI version. The caller preserves existing settable attributes.
    unsafe {
        ioctl(
            file,
            Setter::<
                { linux_raw_sys::ioctl::FS_IOC_FSSETXATTR as rustix::ioctl::Opcode },
                linux_raw_sys::general::fsxattr,
            >::new(attributes),
        )
    }
    .map_err(errno_to_io)
}

pub(crate) fn usage_from_quota_counts(
    data_basic_blocks: u64,
    realtime_basic_blocks: u64,
    filesystem_objects: u64,
) -> std::io::Result<FilesystemAllocation> {
    let basic_blocks = data_basic_blocks
        .checked_add(realtime_basic_blocks)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS project data and realtime usage exceeds u64 blocks",
            )
        })?;
    let allocated_bytes = basic_blocks
        .checked_mul(XFS_BASIC_BLOCK_BYTES)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS project usage exceeds u64 bytes",
            )
        })?;
    Ok(FilesystemAllocation {
        allocated_bytes,
        filesystem_objects,
    })
}

pub(crate) fn errno_to_io(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

/// The socket of `xfs-quotad` on a thread of this test process, for the XFS root `root`. The test
/// process holds `CAP_SYS_ADMIN` itself.
#[cfg(test)]
pub(crate) fn in_process_xfs_quotad(root: impl AsRef<Path>) -> Box<Path> {
    golem_xfs_quotad::start_in_process(root.as_ref())
        .expect("failed to start xfs-quotad in the test process")
        .into_boxed_path()
}

#[cfg(test)]
mod tests;
