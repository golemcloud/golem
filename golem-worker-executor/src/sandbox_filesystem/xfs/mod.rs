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
use crate::metrics::agent_filesystem::{self, ProjectCleanup as ProjectCleanupMetric};
use golem_xfs_quotad::{
    FS_DQUOT_VERSION, FS_PROJ_QUOTA, FS_QSTATV_VERSION1, FsDiskQuota, FsQuotaStatV,
    PROJECT_DATA_LIMIT_FIELDS, PROJECT_LIMIT_FIELDS, XfsQuotadClient,
};
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, StatVfsMountFlags, StatxAttributes, StatxFlags, flock,
    fstatfs, fstatvfs, ioctl_ficlone, mkdirat, openat, statx,
};
use rustix::ioctl::{Getter, Setter, ioctl};
use std::collections::HashMap;
use std::fs::File;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
// The mount table of the process: one line per mount, in the format of `proc_pid_mountinfo(5)`.
const MOUNT_TABLE: &str = "/proc/self/mountinfo";
// `XFS_FSOP_GEOM_FLAGS_DIRV2CI` of `xfs_fs.h`: names are compared without ASCII case.
const XFS_FSOP_GEOM_FLAGS_DIRV2CI: u32 = 1 << 12;
// `XFS_IOC_FSGEOMETRY_V1` of `xfs_fs.h`: `_IOR('X', 100, struct xfs_fsop_geom_v1)`.
const XFS_IOC_FSGEOMETRY_V1: rustix::ioctl::Opcode =
    rustix::ioctl::opcode::read::<XfsGeometryV1>(b'X', 100);
const XFS_BASIC_BLOCK_BYTES: u64 = 512;
const FS_QUOTA_PDQ_ACCT: u16 = 1 << 4;
const FS_QUOTA_PDQ_ENFD: u16 = 1 << 5;

#[derive(Clone, Copy)]
pub(super) struct ValidatedXfsNameMode {
    identity: FilesystemIdentity,
}

impl ValidatedXfsNameMode {
    pub(super) fn matches_device(self, device: u64) -> bool {
        self.identity.device == device
    }
}

#[cfg(test)]
pub(super) fn validated_xfs_name_mode_for_test(device: u64) -> ValidatedXfsNameMode {
    validated_xfs_name_mode(XFS_SUPER_MAGIC, FilesystemIdentity { device })
        .expect("XFS filesystem type must produce a managed name-mode proof")
}

fn validated_xfs_name_mode(
    filesystem_type: u64,
    identity: FilesystemIdentity,
) -> Option<ValidatedXfsNameMode> {
    (filesystem_type == XFS_SUPER_MAGIC).then_some(ValidatedXfsNameMode { identity })
}

#[derive(Default)]
struct ProjectAllocator {
    next: u32,
    active: HashMap<NonZeroU32, Box<Path>>,
}

/// `struct xfs_fsop_geom_v1` of `xfs_fs.h`, the output of `XFS_IOC_FSGEOMETRY_V1`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XfsGeometryV1 {
    blocksize: u32,
    rtextsize: u32,
    agblocks: u32,
    agcount: u32,
    logblocks: u32,
    sectsize: u32,
    inodesize: u32,
    imaxpct: u32,
    datablocks: u64,
    rtblocks: u64,
    rtextents: u64,
    logstart: u64,
    uuid: [u8; 16],
    sunit: u32,
    swidth: u32,
    version: i32,
    flags: u32,
    logsectsize: u32,
    rtsectsize: u32,
    dirblocksize: u32,
}

#[derive(Clone)]
pub(super) struct ManagedProvisioning {
    volume: FilesystemVolume,
    root: Arc<Path>,
    root_fd: Arc<File>,
    quotas: Arc<XfsQuotadClient>,
    allocator: Arc<Mutex<ProjectAllocator>>,
    filesystem_block_bytes: NonZeroU64,
    validated_name_mode: ValidatedXfsNameMode,
    cleanup_retry: RetryConfig,
}

/// The root directory of an XFS volume after the checks that every XFS storage mode needs.
///
/// The value holds the exclusive lock of the root for as long as its descriptor is open.
pub(super) struct XfsRoot {
    /// The root as the configuration gives it. Errors name it.
    root: Box<Path>,
    root_fd: Arc<File>,
    /// The root through the open descriptor, which keeps naming the same directory.
    stable_root: Box<Path>,
    identity: FilesystemIdentity,
    name_mode: ValidatedXfsNameMode,
    filesystem_block_bytes: NonZeroU64,
    /// The filesystem options of the mount, as XFS shows them in the mount table.
    mount_options: Box<str>,
}

impl XfsRoot {
    /// Opens the XFS volume at `root` for one executor.
    ///
    /// The path must be the root directory of an XFS filesystem and the root of its mount. The
    /// filesystem must compare names with their case and have a valid block size. No other
    /// process must hold the lock of the root. The function takes the exclusive lock of the root.
    /// It changes nothing on the volume, and none of its calls needs a privilege.
    pub(super) fn open(root: &Path) -> Result<Self, FilesystemStorageError> {
        let root_fd = File::open(root)
            .map_err(|error| FilesystemStorageError::io("open XFS root", root, error))?;
        let filesystem = fstatfs(&root_fd).map_err(|error| {
            FilesystemStorageError::io("inspect XFS root", root, errno_to_io(error))
        })?;
        let identity = filesystem_identity(&root_fd)
            .map_err(|error| FilesystemStorageError::io("identify XFS root", root, error))?;
        let name_mode =
            validated_xfs_name_mode(filesystem.f_type as u64, identity).ok_or_else(|| {
                FilesystemStorageError::verification("validate XFS root filesystem type", root)
            })?;
        let flags = geometry_flags(&root_fd).map_err(|error| {
            FilesystemStorageError::io("inspect XFS root geometry", root, error)
        })?;
        if !names_are_case_sensitive(flags) {
            return Err(FilesystemStorageError::verification(
                "validate XFS names are case-sensitive",
                root,
            ));
        }
        let mount_options = validate_xfs_root_location(&root_fd, root)?;
        flock(&root_fd, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            FilesystemStorageError::io(
                "acquire exclusive ownership of XFS root",
                root,
                errno_to_io(error),
            )
        })?;
        let filesystem_block_bytes = u64::try_from(filesystem.f_bsize)
            .ok()
            .filter(|bytes| *bytes != 0 && bytes.is_multiple_of(XFS_BASIC_BLOCK_BYTES))
            .and_then(NonZeroU64::new)
            .ok_or_else(|| {
                FilesystemStorageError::verification("validate XFS filesystem block size", root)
            })?;
        let stable_root: Box<Path> =
            PathBuf::from(format!("/proc/self/fd/{}", root_fd.as_raw_fd())).into_boxed_path();
        std::fs::metadata(&stable_root).map_err(|error| {
            FilesystemStorageError::io("open XFS root through its stable descriptor", root, error)
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
pub(super) fn bind_reflink(
    root: XfsRoot,
    cleanup_retry: RetryConfig,
) -> Result<(FilesystemVolume, directories::DirectoryProvisioning), FilesystemStorageError> {
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
        directories::DirectoryProvisioning::new(
            Some(Arc::from(stable_root)),
            cleanup_retry,
            NativeNameModeSource::ValidatedXfs(name_mode),
        ),
    ))
}

/// Checks that the mount of `root` accounts no project quotas, and that the root has no project
/// id and no project-inherit flag. Then no agent directory and no host directory gets a project.
fn verify_no_project_quotas(root: &XfsRoot) -> Result<(), FilesystemStorageError> {
    if project_quota_accounting(&root.mount_options) == ProjectQuotaAccounting::On {
        return Err(FilesystemStorageError::verification(
            "verify XFS volume has no project quota accounting",
            &root.root,
        ));
    }
    let attributes = get_fsxattr(&root.root_fd).map_err(|error| {
        FilesystemStorageError::io("inspect XFS root project attributes", &root.root, error)
    })?;
    if has_project_identity(&attributes) {
        return Err(FilesystemStorageError::verification(
            "verify XFS root has no project identity",
            &root.root,
        ));
    }
    Ok(())
}

/// The directory under the root of an XFS volume in which [`probe_reflink`] tests a reflink.
const REFLINK_PROBE: &str = ".golem-xfs-reflink-probe";

/// Checks that a reflink works on the volume of `root`: makes a file under [`REFLINK_PROBE`],
/// reflinks it into a second file, and reads the second file back.
///
/// What an earlier process left under the probe name is removed first. The probe is removed after
/// the check, also when the check fails.
fn probe_reflink(root: &XfsRoot) -> Result<(), FilesystemStorageError> {
    let probe = root.stable_root.join(REFLINK_PROBE);
    let configured_probe = root.root.join(REFLINK_PROBE);
    remove_and_verify_blocking(&probe, "remove stale XFS reflink probe")
        .map_err(|error| error.about(&configured_probe))?;
    std::fs::create_dir(&probe).map_err(|error| {
        FilesystemStorageError::io("create XFS reflink probe", &configured_probe, error)
    })?;
    let probed = reflink_probe_contents(&probe);
    let removed = remove_and_verify_blocking(&probe, "remove XFS reflink probe")
        .map_err(|error| error.about(&configured_probe));
    probed
        .map_err(|error| {
            FilesystemStorageError::io("validate XFS reflink support", &configured_probe, error)
        })
        .and(removed)
}

/// Writes a file in the empty directory `probe`, reflinks it into a new file, and checks that the
/// new file holds the same bytes.
fn reflink_probe_contents(probe: &Path) -> std::io::Result<()> {
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

impl ManagedProvisioning {
    /// Binds managed XFS storage with project quotas on the opened root `root`. The quota calls go
    /// to `xfs-quotad` on `xfs_quotad_socket`.
    ///
    /// The function removes any project from the root, checks that project quota accounting and
    /// enforcement are on and that the helper may query and change project quotas, and probes
    /// project inheritance and reflink.
    pub(super) fn new(
        root: XfsRoot,
        xfs_quotad_socket: &Path,
        cleanup_retry: &RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let XfsRoot {
            root: configured_root,
            root_fd,
            stable_root,
            identity,
            name_mode,
            filesystem_block_bytes,
            ..
        } = root;
        let quotas = XfsQuotadClient::connect(xfs_quotad_socket, identity.device)
            .map(Arc::new)
            .map_err(|error| {
                FilesystemStorageError::io("connect to xfs-quotad", &configured_root, error)
            })?;
        clear_root_project_assignment(&root_fd, &configured_root)?;
        agent_filesystem::record_agent_filesystem_volume_opened(true);
        let backend = Self {
            volume: FilesystemVolume::copy_on_write(Arc::clone(&root_fd), identity)
                .with_project_quotas(Arc::clone(&quotas)),
            root: Arc::from(stable_root),
            root_fd,
            quotas,
            allocator: Arc::new(Mutex::new(ProjectAllocator {
                next: 1,
                active: HashMap::new(),
            })),
            filesystem_block_bytes,
            validated_name_mode: name_mode,
            cleanup_retry: cleanup_retry.clone(),
        };
        backend.validate_project_quota_state()?;
        backend.validate_project_assignment(cleanup_retry)?;

        Ok(backend)
    }

    pub(super) fn volume(&self) -> &FilesystemVolume {
        &self.volume
    }

    pub(super) fn root(&self) -> &Arc<Path> {
        &self.root
    }

    pub(super) fn project_id(&self, file: &File) -> std::io::Result<Option<NonZeroU32>> {
        let attributes = get_fsxattr(file)?;
        Ok(NonZeroU32::new(attributes.fsx_projid))
    }

    fn open_filesystem_parent(&self, environment: &str, component: &str) -> std::io::Result<File> {
        let environment = open_or_create_directory(&self.root_fd, environment)?;
        open_or_create_directory(&environment, component)
    }

    pub(super) fn open_entry(&self, parent: &File, name: &str) -> std::io::Result<File> {
        let entry = openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno_to_io)?;
        Ok(File::from(entry))
    }

    pub(super) fn open_directory(&self, parent: &File, name: &str) -> std::io::Result<File> {
        let directory = openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno_to_io)?;
        Ok(File::from(directory))
    }

    pub(super) fn reserved_project(&self, owner: &Path) -> Option<NonZeroU32> {
        let allocator = self
            .allocator
            .lock()
            .expect("XFS project allocator lock poisoned");
        project_of_owner(&allocator.active, owner)
    }

    pub(super) fn reserve_existing_project(
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

    pub(super) fn reserve_project(&self, owner: &Path) -> std::io::Result<NonZeroU32> {
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

    pub(super) fn release_project(&self, project_id: NonZeroU32) {
        self.allocator
            .lock()
            .expect("XFS project allocator lock poisoned")
            .active
            .remove(&project_id);
    }

    pub(super) fn assign_project(
        &self,
        file: &File,
        project_id: NonZeroU32,
    ) -> std::io::Result<()> {
        set_project(file, project_id)?;
        validate_project_attributes(get_fsxattr(file)?, project_id)
    }

    pub(super) fn finish_project_cleanup(&self, project_id: NonZeroU32) -> std::io::Result<()> {
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

    fn validate_project_quota_state(&self) -> Result<(), FilesystemStorageError> {
        self.observe_project_quota_state().map_err(|error| {
            FilesystemStorageError::io(
                "validate managed XFS project quota accounting and enforcement",
                &self.root,
                error,
            )
        })?;

        // Querying project zero proves that the executor has quota-query
        // privileges without assigning a reusable project identity.
        self.project_usage(0).map_err(|error| {
            FilesystemStorageError::io(
                "validate managed XFS project quota query permissions",
                &self.root,
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
    ) -> Result<(), FilesystemStorageError> {
        const PROBE_PROJECT_ID: u32 = 0x8000_0000;
        let project_id = NonZeroU32::new(PROBE_PROJECT_ID).unwrap();
        let probe = self.root.join(".golem-xfs-project-probe");
        if probe.exists() {
            std::fs::remove_dir_all(&probe).map_err(|error| {
                FilesystemStorageError::cleanup_io(
                    "remove stale managed XFS startup probe",
                    &probe,
                    error,
                )
            })?;
        }

        let usage = self.project_usage(project_id.get()).map_err(|error| {
            FilesystemStorageError::io("validate 32-bit XFS project quota query", &self.root, error)
        })?;
        if !project_is_empty(usage) {
            return Err(FilesystemStorageError::verification(
                "reserve unused 32-bit XFS startup probe project",
                &self.root,
            ));
        }
        self.clear_project_limits(project_id).map_err(|error| {
            FilesystemStorageError::io(
                "validate XFS project quota update permissions",
                &self.root,
                error,
            )
        })?;
        std::fs::create_dir(&probe).map_err(|error| {
            FilesystemStorageError::io("create managed XFS startup probe", &probe, error)
        })?;
        if let Err(error) = self.probe_project_inheritance(&probe, project_id) {
            let _ = std::fs::remove_dir_all(&probe);
            return Err(FilesystemStorageError::io(
                "validate managed XFS project assignment and inheritance",
                &probe,
                error,
            ));
        }
        std::fs::remove_dir_all(&probe).map_err(|error| {
            FilesystemStorageError::cleanup_io("remove managed XFS startup probe", &probe, error)
        })?;
        self.finish_project_cleanup_with_retry(project_id, cleanup_retry)
            .map_err(|error| {
                FilesystemStorageError::cleanup_io(
                    "clear managed XFS startup probe project",
                    &probe,
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
fn project_of_owner(active: &HashMap<NonZeroU32, Box<Path>>, owner: &Path) -> Option<NonZeroU32> {
    active
        .iter()
        .find_map(|(project_id, active_owner)| (&**active_owner == owner).then_some(*project_id))
}

/// What a reservation of an existing project finds in the active reservations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExistingReservation<'a> {
    /// The owner already holds the project.
    Held,
    /// Another owner holds the project.
    HeldByOther(&'a Path),
    /// No owner holds the project.
    Free,
}

/// What a reservation of `project_id` for `owner` finds in `active`.
fn existing_reservation<'a>(
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
fn next_free_project(next: u32, active: &HashMap<NonZeroU32, Box<Path>>) -> Option<NonZeroU32> {
    let first = next.max(1);
    (first..=u32::MAX)
        .chain(1..first)
        .filter_map(NonZeroU32::new)
        .find(|project_id| !active.contains_key(project_id))
}

/// Where the next scan starts after `project_id` is reserved: the next project, and 1 after
/// `u32::MAX`.
fn project_after(project_id: NonZeroU32) -> u32 {
    project_id.get().checked_add(1).unwrap_or(1)
}

/// Whether the reservation loop got back to `first`, the first candidate it reserved, so it went
/// once around the project ids.
fn candidate_repeats(first: Option<NonZeroU32>, candidate: NonZeroU32) -> bool {
    first == Some(candidate)
}

/// Whether a project with `usage` holds nothing, so it can be given to a new owner.
fn project_is_empty(usage: FilesystemAllocation) -> bool {
    usage.allocated_bytes == 0 && usage.filesystem_objects == 0
}

/// Whether a read of a project quota record failed because XFS has no record for the project.
fn quota_record_is_missing(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
}

/// The quota record of a project for which XFS has no record: no limits, no usage.
fn empty_project_quota(project_id: u32) -> FsDiskQuota {
    FsDiskQuota {
        d_version: FS_DQUOT_VERSION,
        d_flags: FS_PROJ_QUOTA,
        d_id: project_id,
        ..FsDiskQuota::default()
    }
}

/// Whether `quota` is a project quota record of the expected version for `project_id`.
fn project_quota_record_is_valid(quota: &FsDiskQuota, project_id: u32) -> bool {
    quota.d_version == FS_DQUOT_VERSION
        && quota.d_flags == FS_PROJ_QUOTA
        && quota.d_id == project_id
}

/// Whether `quota` still holds a limit, a timer or a warning count after its limits were cleared.
fn project_quota_retains_state(quota: &FsDiskQuota) -> bool {
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
fn limits_are_representable(limits: FilesystemLimits, filesystem_block_bytes: NonZeroU64) -> bool {
    limits.allocated_bytes != 0
        && limits
            .allocated_bytes
            .is_multiple_of(filesystem_block_bytes.get())
        && limits.filesystem_objects != 0
}

/// Whether `installed` holds the block hard limit `block_hard_limit` and the inode hard limit
/// `inode_hard_limit`, and no soft limit.
fn project_quota_holds_limits(
    installed: &FsDiskQuota,
    block_hard_limit: u64,
    inode_hard_limit: u64,
) -> bool {
    installed.d_blk_hardlimit == block_hard_limit
        && installed.d_blk_softlimit == 0
        && installed.d_ino_hardlimit == inode_hard_limit
        && installed.d_ino_softlimit == 0
}

fn no_reusable_projects() -> std::io::Error {
    std::io::Error::other("no reusable XFS project IDs are available")
}

// The root must have no project id and no inheritance before anything is created under it,
// so a host directory made under it belongs to no project.
fn clear_root_project_assignment(
    root_fd: &File,
    root: &Path,
) -> Result<(), FilesystemStorageError> {
    let mut attributes = get_fsxattr(root_fd).map_err(|error| {
        FilesystemStorageError::io("inspect managed XFS root project attributes", root, error)
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
        FilesystemStorageError::io("clear managed XFS root project inheritance", root, error)
    })?;
    let assigned = get_fsxattr(root_fd).map_err(|error| {
        FilesystemStorageError::io("verify managed XFS root project attributes", root, error)
    })?;
    if has_project_identity(&assigned) {
        return Err(FilesystemStorageError::verification(
            "verify managed XFS root has neutral project identity",
            root,
        ));
    }
    Ok(())
}

/// Gives the project of a stale managed sandbox path from the project on disk and the project
/// that this process reserved for the path.
///
/// The stale project is the one that exists. When both exist and differ, the result is
/// [`ProjectMismatch`].
fn stale_project(
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
struct ProjectMismatch;

/// Whether a directory has a project id or gives one to what is made in it.
fn has_project_identity(attributes: &linux_raw_sys::general::fsxattr) -> bool {
    attributes.fsx_projid != 0
        || attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT != 0
}

/// Checks that a host directory has no project id and gives no project id to what is made in it.
pub(super) fn verify_host_directory_has_no_project(
    path: &Path,
) -> Result<(), FilesystemStorageError> {
    let directory = File::open(path)
        .map_err(|error| FilesystemStorageError::io("open XFS host directory", path, error))?;
    let attributes = get_fsxattr(&directory).map_err(|error| {
        FilesystemStorageError::io("inspect XFS host directory project attributes", path, error)
    })?;
    if has_project_identity(&attributes) {
        return Err(FilesystemStorageError::verification(
            "verify XFS host directory has no project identity",
            path,
        ));
    }
    Ok(())
}

/// Checks that `root` is the root directory of its filesystem and the root of its mount, and gives
/// the filesystem options of the mount.
fn validate_xfs_root_location(
    root_fd: &File,
    root: &Path,
) -> Result<Box<str>, FilesystemStorageError> {
    let location = statx(
        root_fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID,
    )
    .map_err(|error| {
        FilesystemStorageError::io("inspect XFS root mount location", root, errno_to_io(error))
    })?;
    let mount_table = std::fs::read_to_string(MOUNT_TABLE).map_err(|error| {
        FilesystemStorageError::io("read the mount table", Path::new(MOUNT_TABLE), error)
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
        _ => Err(FilesystemStorageError::verification(
            "validate XFS root is the filesystem mount root",
            root,
        )),
    }
}

/// Whether an XFS mount accounts project quotas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProjectQuotaAccounting {
    On,
    Off,
}

/// The fields of one line of `/proc/self/mountinfo` that the root checks read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MountLine<'a> {
    /// The directory of the filesystem that is the root of the mount.
    root: &'a str,
    /// The options of the filesystem, as the filesystem shows them.
    super_options: &'a str,
}

/// Whether a directory is the root directory of its filesystem and is mounted there.
///
/// The directory must be a mount root, and `line`, the line of its mount in the mount table, must
/// give `/` as the root of the mount within its filesystem. A bind mount of a subdirectory is a
/// mount root too, but its mount has another root. A directory without a known mount line is not
/// valid.
fn xfs_root_location_is_valid(
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
fn mount_line(mount_table: &str, mount_id: u64) -> Option<MountLine<'_>> {
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
fn project_quota_accounting(super_options: &str) -> ProjectQuotaAccounting {
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
fn names_are_case_sensitive(geometry_flags: u32) -> bool {
    geometry_flags & XFS_FSOP_GEOM_FLAGS_DIRV2CI == 0
}

/// Reads the superblock flags of the XFS volume that holds `file`. The call needs no privilege.
fn geometry_flags(file: &File) -> std::io::Result<u32> {
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

pub(super) fn observe_space(
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

fn filesystem_identity(root: &File) -> std::io::Result<FilesystemIdentity> {
    Ok(FilesystemIdentity {
        device: root.metadata()?.dev(),
    })
}

fn validate_filesystem_identity(root: &File, expected: FilesystemIdentity) -> std::io::Result<()> {
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

pub(super) fn assign_project(file: &File, project_id: NonZeroU32) -> std::io::Result<()> {
    set_project(file, project_id)?;
    validate_project_attributes(get_fsxattr(file)?, project_id)
}

#[cfg(test)]
pub(super) fn file_project_id(file: &File) -> std::io::Result<Option<NonZeroU32>> {
    Ok(NonZeroU32::new(get_fsxattr(file)?.fsx_projid))
}

// The kernel default copy-on-write extent size hint, in filesystem blocks, for a volume whose
// root inode carries no hint of its own.
#[cfg(test)]
const XFS_DEFAULT_COW_EXTENT_SIZE_BLOCKS: u64 = 32;

/// Reads the copy-on-write extent size hint that a write into a shared extent of this volume
/// allocates: the hint on the root inode when it has one, else the kernel default.
#[cfg(test)]
pub(super) fn cow_extent_size_hint(
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
pub(super) fn clone_file(target: &File, source: &File) -> std::io::Result<()> {
    ioctl_ficlone(target, source).map_err(errno_to_io)
}

/// Makes the new file `target` share the extents of `source`.
///
/// `target` must have `project_id`, which it gets from its directory, so that the extents are
/// charged to that project. A target with another project gives an `InvalidData` error.
pub(super) fn reflink_into_project(
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

/// Writes the pending changes of the volume that holds `root` to stable storage.
pub(super) fn sync_volume(root: &Path) -> std::io::Result<()> {
    rustix::fs::syncfs(&File::open(root)?).map_err(errno_to_io)
}

pub(super) fn project_allocation(
    volume: &FilesystemVolume,
    project_id: NonZeroU32,
) -> std::io::Result<FilesystemAllocation> {
    let quota = project_quota(volume_quotas(volume), project_id.get())?;
    usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)
}

pub(super) fn install_project_limits(
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

fn volume_quotas(volume: &FilesystemVolume) -> &XfsQuotadClient {
    volume
        .project_quotas()
        .expect("managed XFS operation requires a managed volume")
}

fn project_quota(quotas: &XfsQuotadClient, project_id: u32) -> std::io::Result<FsDiskQuota> {
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

impl ManagedProvisioning {
    pub(super) async fn create_fresh(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let owner = name.relative_path();
        let lifecycle = acquire_filesystem_lease(&self.root().join(&owner)).await;
        let provisioning = self.clone();
        let error_path = self.root().join(&owner);
        tokio::spawn(async move { provisioning.provision(volume, name, lifecycle).await })
            .await
            .map_err(|error| {
                FilesystemStorageError::io(
                    "provision managed XFS sandbox filesystem",
                    &error_path,
                    std::io::Error::other(error),
                )
            })?
    }

    async fn provision(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
        lifecycle: OwnedMutexGuard<()>,
    ) -> Result<SandboxFilesystem, FilesystemStorageError> {
        let [environment, component, filesystem_name] = name.components();
        let owner = name.relative_path();
        let mut lifecycle = Some(lifecycle);
        let parent = self
            .open_filesystem_parent(environment, component)
            .map_err(|error| {
                FilesystemStorageError::io(
                    "open managed runtime directory parent",
                    &self
                        .root()
                        .join(owner.parent().expect("owner must have a parent")),
                    error,
                )
            })?;
        let cleanup_path =
            PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(filesystem_name);

        let disk_project = match tokio::fs::symlink_metadata(&cleanup_path).await {
            Ok(metadata) if !metadata.file_type().is_symlink() => {
                let provisioning = self.clone();
                let existing_entry =
                    provisioning
                        .open_entry(&parent, filesystem_name)
                        .map_err(|error| {
                            FilesystemStorageError::cleanup_io(
                                "open stale managed XFS runtime path",
                                &cleanup_path,
                                error,
                            )
                        })?;
                execute_native(
                    NativeStorageProfile::KnownLocal,
                    NativeOperation::Quota,
                    move || provisioning.project_id(&existing_entry),
                )
                .await
                .map_err(|error| {
                    FilesystemStorageError::task_failure(
                        "inspect stale managed XFS project",
                        &cleanup_path,
                        error,
                    )
                })?
                .map_err(|error| {
                    FilesystemStorageError::io(
                        "inspect stale managed XFS project",
                        &cleanup_path,
                        error,
                    )
                })?
            }
            Ok(_) => None,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(FilesystemStorageError::cleanup_io(
                    "inspect stale managed XFS runtime path",
                    &cleanup_path,
                    error,
                ));
            }
        };
        let reserved_project = self.reserved_project(&owner);
        let Ok(stale_project) = stale_project(disk_project, reserved_project) else {
            return Err(FilesystemStorageError::cleanup_verification(
                "match stale managed XFS path and reserved project",
                &cleanup_path,
            ));
        };

        let mut stale_cleanup = if let Some(project_id) = stale_project {
            let cleanup_parent = parent.try_clone().map_err(|error| {
                FilesystemStorageError::cleanup_io(
                    "retain managed XFS runtime directory parent",
                    &cleanup_path,
                    error,
                )
            })?;
            self.reserve_existing_project(project_id, &owner)
                .map_err(|error| {
                    FilesystemStorageError::cleanup_io(
                        "reserve stale managed XFS project for cleanup",
                        &cleanup_path,
                        error,
                    )
                })?;
            Some(StaleProjectCleanup::new(
                ProjectCleanup::new(
                    self.clone(),
                    project_id,
                    cleanup_path.clone(),
                    self.cleanup_retry.clone(),
                ),
                cleanup_parent,
                lifecycle
                    .take()
                    .expect("managed XFS lifecycle owner must exist"),
            ))
        } else {
            None
        };
        remove_and_verify(
            &cleanup_path,
            "remove stale managed XFS runtime directory",
            &self.cleanup_retry,
        )
        .await?;
        if stale_project.is_some() {
            stale_cleanup
                .as_ref()
                .expect("stale project cleanup owner must exist")
                .project
                .finish()
                .await?;
            agent_filesystem::record_project_stale_reclaim();
            lifecycle = Some(
                stale_cleanup
                    .as_mut()
                    .expect("stale project cleanup owner must exist")
                    .disarm(),
            );
        }

        if let Err(error) = tokio::fs::create_dir(&cleanup_path).await {
            return Err(rollback_creation(
                &cleanup_path,
                FilesystemStorageError::io(
                    "create fresh managed runtime directory",
                    &cleanup_path,
                    error,
                ),
                &self.cleanup_retry,
            )
            .await);
        }

        let project_id = self.reserve_project(&owner).map_err(|error| {
            FilesystemStorageError::io("allocate managed XFS project", &cleanup_path, error)
        });
        let project_id = match project_id {
            Ok(project_id) => project_id,
            Err(error) => {
                return Err(rollback_creation(&cleanup_path, error, &self.cleanup_retry).await);
            }
        };
        let root_fd = match self.open_directory(&parent, filesystem_name) {
            Ok(root) => root,
            Err(error) => {
                self.release_project(project_id);
                return Err(rollback_creation(
                    &cleanup_path,
                    FilesystemStorageError::io(
                        "open fresh managed runtime directory",
                        &cleanup_path,
                        error,
                    ),
                    &self.cleanup_retry,
                )
                .await);
            }
        };
        let root = PathBuf::from(format!("/proc/self/fd/{}", root_fd.as_raw_fd()));
        let assignment_result = assign_project(&root_fd, project_id).map_err(|error| {
            FilesystemStorageError::io("assign managed XFS project", &root, error)
        });
        let native_root =
            NativeRoot::from_directory(root, Arc::new(cap_std::fs::Dir::from_std_file(root_fd)));
        let lease = LeaseState {
            lifecycle: lifecycle.expect("managed XFS lifecycle owner must exist"),
            cleanup: NativeCleanup::Managed(Box::new(ManagedProjectCleanup {
                project: ProjectCleanup::new(
                    self.clone(),
                    project_id,
                    cleanup_path,
                    self.cleanup_retry.clone(),
                ),
                _parent: parent,
            })),
        };
        let created = SandboxFilesystem::new(
            native_root,
            lease,
            volume,
            QuotaAuthority::Project {
                project_id,
                filesystem_block_bytes: self.filesystem_block_bytes,
            },
            NativeNameModeSource::ValidatedXfs(self.validated_name_mode),
        );
        if let Err(error) = assignment_result {
            return Err(rollback_created_filesystem(created, error).await);
        }
        if let Err(error) = verify_fresh_open_directory(created.root()).await {
            return Err(rollback_created_filesystem(created, error).await);
        }

        Ok(created)
    }
}

pub(super) struct ManagedProjectCleanup {
    project: ProjectCleanup,
    _parent: File,
}

impl ManagedProjectCleanup {
    pub(super) async fn delete(&mut self) -> Result<(), FilesystemStorageError> {
        let deleted = async {
            remove_and_verify(
                &self.project.path,
                "delete managed XFS runtime directory",
                &self.project.cleanup_retry,
            )
            .await?;
            self.project.finish().await
        }
        .await;
        if deleted.is_err() {
            agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Delete);
        }
        deleted
    }

    pub(super) fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        let deleted = self.project.remove_and_finish_blocking();
        if deleted.is_err() {
            agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Drop);
        }
        deleted
    }
}

struct StaleProjectCleanup {
    project: ProjectCleanup,
    parent: Option<File>,
    lifecycle: Option<OwnedMutexGuard<()>>,
    armed: bool,
}

impl StaleProjectCleanup {
    fn new(project: ProjectCleanup, parent: File, lifecycle: OwnedMutexGuard<()>) -> Self {
        Self {
            project,
            parent: Some(parent),
            lifecycle: Some(lifecycle),
            armed: true,
        }
    }

    fn disarm(&mut self) -> OwnedMutexGuard<()> {
        self.armed = false;
        self.parent.take();
        self.lifecycle
            .take()
            .expect("managed XFS stale cleanup lifecycle owner must exist")
    }
}

impl Drop for StaleProjectCleanup {
    fn drop(&mut self) {
        if self.armed {
            let project = self.project.clone();
            let parent = self.parent.take();
            let lifecycle = self.lifecycle.take();
            std::thread::spawn(move || {
                if let Err(error) = project.remove_and_finish_blocking() {
                    agent_filesystem::record_project_cleanup_failure(ProjectCleanupMetric::Stale);
                    tracing::error!(error = %error, "Failed to clean reserved managed XFS project");
                }
                drop((parent, lifecycle));
            });
        }
    }
}

#[derive(Clone)]
struct ProjectCleanup {
    provisioning: ManagedProvisioning,
    project_id: NonZeroU32,
    path: PathBuf,
    cleanup_retry: RetryConfig,
}

impl ProjectCleanup {
    fn new(
        provisioning: ManagedProvisioning,
        project_id: NonZeroU32,
        path: PathBuf,
        cleanup_retry: RetryConfig,
    ) -> Self {
        Self {
            provisioning,
            project_id,
            path,
            cleanup_retry,
        }
    }

    async fn finish(&self) -> Result<(), FilesystemStorageError> {
        let mut retry = RetryState::new(&self.cleanup_retry);
        loop {
            retry.start_attempt();
            let provisioning = self.provisioning.clone();
            let project_id = self.project_id;
            let attempt = execute_native(
                NativeStorageProfile::KnownLocal,
                NativeOperation::RecursiveCleanup,
                move || provisioning.finish_project_cleanup(project_id),
            )
            .await
            .map_err(|error| {
                let mut failure = FilesystemStorageError::task_failure(
                    "verify and clear managed XFS project",
                    &self.path,
                    error,
                );
                failure.inner.cleanup_failed = true;
                failure
            })?;
            match attempt {
                Ok(()) => {
                    self.provisioning.release_project(self.project_id);
                    return Ok(());
                }
                Err(error) => {
                    if !retry.failed_attempt().await {
                        return Err(FilesystemStorageError::cleanup_io(
                            "verify and clear managed XFS project",
                            &self.path,
                            error,
                        ));
                    }
                }
            }
        }
    }

    fn remove_and_finish_blocking(&self) -> Result<(), FilesystemStorageError> {
        remove_and_verify_blocking(&self.path, "delete managed XFS runtime directory")?;
        let attempts = self.cleanup_retry.max_attempts.max(1);
        let mut delay = self.cleanup_retry.min_delay;
        for attempt in 1..=attempts {
            match self.provisioning.finish_project_cleanup(self.project_id) {
                Ok(()) => {
                    self.provisioning.release_project(self.project_id);
                    return Ok(());
                }
                Err(error) if attempt == attempts => {
                    return Err(FilesystemStorageError::cleanup_io(
                        "verify and clear managed XFS project",
                        &self.path,
                        error,
                    ));
                }
                Err(_) => {
                    std::thread::sleep(delay);
                    delay = delay
                        .mul_f64(self.cleanup_retry.multiplier)
                        .min(self.cleanup_retry.max_delay);
                }
            }
        }
        unreachable!("managed XFS cleanup always performs at least one attempt")
    }
}

fn get_fsxattr(file: &File) -> std::io::Result<linux_raw_sys::general::fsxattr> {
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

fn validate_project_attributes(
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

fn validate_project_quota_state_record(state: &FsQuotaStatV) -> std::io::Result<()> {
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

fn set_project(file: &File, project_id: NonZeroU32) -> std::io::Result<()> {
    let mut attributes = get_fsxattr(file)?;
    attributes.fsx_projid = project_id.get();
    attributes.fsx_xflags |= linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
    attributes.fsx_pad = [0; 8];
    set_fsxattr(file, attributes)
}

fn set_fsxattr(file: &File, attributes: linux_raw_sys::general::fsxattr) -> std::io::Result<()> {
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

fn open_or_create_directory(parent: &File, name: &str) -> std::io::Result<File> {
    match mkdirat(parent, name, Mode::from_raw_mode(0o700)) {
        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
        Err(error) => return Err(errno_to_io(error)),
    }
    let directory = openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno_to_io)?;
    Ok(File::from(directory))
}

fn usage_from_quota_counts(
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

fn errno_to_io(error: rustix::io::Errno) -> std::io::Error {
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
