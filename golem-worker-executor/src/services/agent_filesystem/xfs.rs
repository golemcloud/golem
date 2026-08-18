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

use super::{
    AgentFilesystemUsage, FilesystemCapacity, FilesystemStorageError,
    ResolvedAgentFilesystemLimits, create_materialization_parent, quota::capacity_from_values,
    set_initial_file_permissions,
};
use golem_common::model::RetryConfig;
use rustix::fs::{
    FlockOperation, Mode, OFlags, StatVfsMountFlags, flock, fstatfs, fstatvfs, ioctl_ficlone,
    mkdirat, openat,
};
use rustix::ioctl::{Getter, Setter, ioctl};
use std::collections::HashMap;
use std::fs::File;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
const XFS_BASIC_BLOCK_BYTES: u64 = 512;
const XQM_PRJQUOTA: u32 = 2;
const Q_XGETQUOTA: u32 = (b'X' as u32) << 8 | 3;
const Q_XSETQLIM: u32 = (b'X' as u32) << 8 | 4;
const Q_XGETQSTATV: u32 = (b'X' as u32) << 8 | 8;
const FS_DQUOT_VERSION: i8 = 1;
const FS_QSTATV_VERSION1: i8 = 1;
const FS_PROJ_QUOTA: i8 = 1 << 1;
const FS_QUOTA_PDQ_ACCT: u16 = 1 << 4;
const FS_QUOTA_PDQ_ENFD: u16 = 1 << 5;
const FS_DQ_ISOFT: u16 = 1 << 0;
const FS_DQ_IHARD: u16 = 1 << 1;
const FS_DQ_BSOFT: u16 = 1 << 2;
const FS_DQ_BHARD: u16 = 1 << 3;
const FS_DQ_RTBSOFT: u16 = 1 << 4;
const FS_DQ_RTBHARD: u16 = 1 << 5;
const PROJECT_LIMIT_FIELDS: u16 =
    FS_DQ_ISOFT | FS_DQ_IHARD | FS_DQ_BSOFT | FS_DQ_BHARD | FS_DQ_RTBSOFT | FS_DQ_RTBHARD;
const PROJECT_DATA_LIMIT_FIELDS: u16 = FS_DQ_ISOFT | FS_DQ_IHARD | FS_DQ_BSOFT | FS_DQ_BHARD;

#[derive(Default)]
struct ProjectAllocator {
    next: u32,
    active: HashMap<NonZeroU32, PathBuf>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FsDiskQuota {
    d_version: i8,
    d_flags: i8,
    d_fieldmask: u16,
    d_id: u32,
    d_blk_hardlimit: u64,
    d_blk_softlimit: u64,
    d_ino_hardlimit: u64,
    d_ino_softlimit: u64,
    d_bcount: u64,
    d_icount: u64,
    d_itimer: i32,
    d_btimer: i32,
    d_iwarns: u16,
    d_bwarns: u16,
    d_itimer_hi: i8,
    d_btimer_hi: i8,
    d_rtbtimer_hi: i8,
    d_padding2: i8,
    d_rtb_hardlimit: u64,
    d_rtb_softlimit: u64,
    d_rtbcount: u64,
    d_rtbtimer: i32,
    d_rtbwarns: u16,
    d_padding3: i16,
    d_padding4: [i8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FsQuotaFileStatV {
    qfs_ino: u64,
    qfs_nblks: u64,
    qfs_nextents: u32,
    qfs_pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FsQuotaStatV {
    qs_version: i8,
    qs_pad1: u8,
    qs_flags: u16,
    qs_incoredqs: u32,
    qs_uquota: FsQuotaFileStatV,
    qs_gquota: FsQuotaFileStatV,
    qs_pquota: FsQuotaFileStatV,
    qs_btimelimit: i32,
    qs_itimelimit: i32,
    qs_rtbtimelimit: i32,
    qs_bwarnlimit: u16,
    qs_iwarnlimit: u16,
    qs_rtbwarnlimit: u16,
    qs_pad3: u16,
    qs_pad4: u32,
    qs_pad2: [u64; 7],
}

pub(super) struct XfsBackend {
    root: PathBuf,
    root_fd: File,
    allocator: Mutex<ProjectAllocator>,
    filesystem_block_bytes: u64,
}

impl XfsBackend {
    pub(super) fn new(
        root: &Path,
        cleanup_retry: &RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let root_fd = File::open(root)
            .map_err(|error| FilesystemStorageError::io("open managed XFS root", root, error))?;
        flock(&root_fd, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            FilesystemStorageError::io(
                "acquire exclusive ownership of managed XFS root",
                root,
                errno_to_io(error),
            )
        })?;
        let filesystem = fstatfs(&root_fd).map_err(|error| {
            FilesystemStorageError::io("inspect managed XFS root", root, errno_to_io(error))
        })?;
        if filesystem.f_type as u64 != XFS_SUPER_MAGIC {
            return Err(FilesystemStorageError::verification(
                "validate managed XFS root filesystem type",
                root,
            ));
        }
        let filesystem_block_bytes = u64::try_from(filesystem.f_bsize).map_err(|_| {
            FilesystemStorageError::verification("validate managed XFS filesystem block size", root)
        })?;
        if filesystem_block_bytes == 0
            || !filesystem_block_bytes.is_multiple_of(XFS_BASIC_BLOCK_BYTES)
        {
            return Err(FilesystemStorageError::verification(
                "validate managed XFS filesystem block size",
                root,
            ));
        }

        let stable_root = PathBuf::from(format!("/proc/self/fd/{}", root_fd.as_raw_fd()));
        std::fs::metadata(&stable_root).map_err(|error| {
            FilesystemStorageError::io(
                "open managed XFS root through its stable descriptor",
                root,
                error,
            )
        })?;

        let backend = Self {
            root: stable_root,
            root_fd,
            allocator: Mutex::new(ProjectAllocator {
                next: 1,
                active: HashMap::new(),
            }),
            filesystem_block_bytes,
        };
        backend.clear_root_project_assignment()?;
        backend.validate_project_quota_state()?;
        backend.validate_project_assignment(cleanup_retry)?;

        Ok(backend)
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    #[allow(
        dead_code,
        reason = "authoritative capacity observation is part of the backend interface"
    )]
    pub(super) fn capacity(&self) -> std::io::Result<FilesystemCapacity> {
        let capacity = fstatvfs(&self.root_fd).map_err(errno_to_io)?;
        if capacity.f_flag.contains(StatVfsMountFlags::RDONLY) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::ReadOnlyFilesystem,
                "managed XFS mount is read-only",
            ));
        }
        capacity_from_values(
            capacity.f_blocks,
            capacity.f_bavail,
            capacity.f_frsize,
            capacity.f_files,
            capacity.f_ffree,
        )
    }

    pub(super) fn project_id(&self, file: &File) -> std::io::Result<Option<NonZeroU32>> {
        let attributes = get_fsxattr(file)?;
        Ok(NonZeroU32::new(attributes.fsx_projid))
    }

    pub(super) fn open_agent_parent(
        &self,
        environment: &str,
        component: &str,
    ) -> std::io::Result<File> {
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
        self.allocator
            .lock()
            .expect("XFS project allocator lock poisoned")
            .active
            .iter()
            .find_map(|(project_id, active_owner)| (active_owner == owner).then_some(*project_id))
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
        match allocator.active.get(&project_id) {
            Some(active_owner) if active_owner == owner => Ok(()),
            Some(active_owner) => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "XFS project {project_id} is owned by {}",
                    active_owner.display()
                ),
            )),
            None => {
                allocator.active.insert(project_id, owner.to_path_buf());
                Ok(())
            }
        }
    }

    pub(super) fn reserve_project(&self, owner: &Path) -> std::io::Result<NonZeroU32> {
        let mut first_candidate = None;
        loop {
            let project_id = self.reserve_project_candidate(owner)?;
            if first_candidate == Some(project_id) {
                self.release_project(project_id);
                return Err(std::io::Error::other(
                    "no reusable XFS project IDs are available",
                ));
            }
            first_candidate.get_or_insert(project_id);

            let prepared = (|| {
                let usage = self.project_usage(project_id.get())?;
                if usage.allocated_bytes != 0 || usage.filesystem_objects != 0 {
                    return Ok(false);
                }
                self.clear_project_limits(project_id)?;
                Ok(true)
            })();

            match prepared {
                Ok(true) => return Ok(project_id),
                Ok(false) => self.release_project(project_id),
                Err(error) => {
                    self.release_project(project_id);
                    return Err(error);
                }
            }
        }
    }

    fn reserve_project_candidate(&self, owner: &Path) -> std::io::Result<NonZeroU32> {
        let mut allocator = self
            .allocator
            .lock()
            .expect("XFS project allocator lock poisoned");
        let first = allocator.next.max(1);
        let mut candidate = first;
        loop {
            let project_id = NonZeroU32::new(candidate).expect("candidate must be nonzero");
            if let std::collections::hash_map::Entry::Vacant(entry) =
                allocator.active.entry(project_id)
            {
                entry.insert(owner.to_path_buf());
                allocator.next = candidate.checked_add(1).unwrap_or(1);
                return Ok(project_id);
            }

            candidate = candidate.checked_add(1).unwrap_or(1);
            if candidate == first {
                return Err(std::io::Error::other(
                    "no reusable XFS project IDs are available",
                ));
            }
        }
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

    pub(super) fn materialize_initial_file(
        &self,
        root: &Path,
        project_id: NonZeroU32,
        source: &Path,
        target: &Path,
        read_only: bool,
    ) -> std::io::Result<()> {
        let parent = create_materialization_parent(root, target)?;
        {
            let temporary = tempfile::NamedTempFile::new_in(parent)?;
            let source = File::open(source)?;
            if self.project_id(temporary.as_file())? != Some(project_id) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "managed XFS initial-file destination did not inherit its project identity",
                ));
            }
            ioctl_ficlone(temporary.as_file(), &source).map_err(errno_to_io)?;
            temporary.as_file().sync_all()?;
            set_initial_file_permissions(temporary.as_file(), read_only)?;
            temporary
                .persist_noclobber(target)
                .map_err(|error| error.error)?;
        }
        rustix::fs::syncfs(&self.root_fd).map_err(errno_to_io)
    }

    #[allow(
        dead_code,
        reason = "authoritative usage observation is part of the backend interface"
    )]
    pub(super) fn usage(&self, project_id: NonZeroU32) -> std::io::Result<AgentFilesystemUsage> {
        self.project_usage(project_id.get())
    }

    pub(super) fn usage_and_limits(
        &self,
        runtime_root: &Path,
        project_id: NonZeroU32,
        policy_version: u32,
    ) -> std::io::Result<(AgentFilesystemUsage, Option<ResolvedAgentFilesystemLimits>)> {
        self.observe_project_quota_state()?;
        let runtime_root = File::open(runtime_root)?;
        validate_project_attributes(get_fsxattr(&runtime_root)?, project_id)?;
        let quota = self.project_quota(project_id.get())?;
        let usage = usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)?;
        let limits = match (quota.d_blk_hardlimit, quota.d_ino_hardlimit) {
            (0, 0) => None,
            (0, _) | (_, 0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "XFS project retained only one member of the quota limit pair",
                ));
            }
            (block_hard_limit, filesystem_objects) => Some(ResolvedAgentFilesystemLimits {
                allocated_bytes: block_hard_limit
                    .checked_mul(XFS_BASIC_BLOCK_BYTES)
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "XFS project byte limit exceeds u64",
                        )
                    })?,
                filesystem_objects,
                filesystem_object_limit_policy_version: policy_version,
            }),
        };
        Ok((usage, limits))
    }

    pub(super) fn finish_project_cleanup(&self, project_id: NonZeroU32) -> std::io::Result<()> {
        let mut usage = self.project_usage(project_id.get())?;
        if usage.allocated_bytes != 0 || usage.filesystem_objects != 0 {
            rustix::fs::syncfs(&self.root_fd).map_err(errno_to_io)?;
            usage = self.project_usage(project_id.get())?;
        }
        if usage.allocated_bytes != 0 || usage.filesystem_objects != 0 {
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

    pub(super) fn install_project_limits(
        &self,
        project_id: NonZeroU32,
        limits: ResolvedAgentFilesystemLimits,
    ) -> std::io::Result<ResolvedAgentFilesystemLimits> {
        if limits.allocated_bytes == 0
            || !limits
                .allocated_bytes
                .is_multiple_of(self.filesystem_block_bytes)
            || limits.filesystem_objects == 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "agent filesystem limits are not exactly representable by managed XFS",
            ));
        }
        let block_hard_limit = limits.allocated_bytes / XFS_BASIC_BLOCK_BYTES;
        let mut quota = FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: PROJECT_DATA_LIMIT_FIELDS,
            d_id: project_id.get(),
            d_blk_hardlimit: block_hard_limit,
            d_ino_hardlimit: limits.filesystem_objects,
            ..FsDiskQuota::default()
        };
        self.set_project_quota_record(project_id, &mut quota)?;

        let mut installed = FsDiskQuota::default();
        self.get_project_quota(project_id.get(), &mut installed)?;
        if installed.d_version != FS_DQUOT_VERSION
            || installed.d_flags != FS_PROJ_QUOTA
            || installed.d_id != project_id.get()
            || installed.d_blk_hardlimit != block_hard_limit
            || installed.d_blk_softlimit != 0
            || installed.d_ino_hardlimit != limits.filesystem_objects
            || installed.d_ino_softlimit != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("XFS project {project_id} did not retain the complete quota limit pair"),
            ));
        }
        Ok(limits)
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

    fn clear_root_project_assignment(&self) -> Result<(), FilesystemStorageError> {
        let mut attributes = get_fsxattr(&self.root_fd).map_err(|error| {
            FilesystemStorageError::io(
                "inspect managed XFS root project attributes",
                &self.root,
                error,
            )
        })?;
        attributes.fsx_projid = 0;
        attributes.fsx_xflags &= !linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
        attributes.fsx_pad = [0; 8];
        set_fsxattr(&self.root_fd, attributes).map_err(|error| {
            FilesystemStorageError::io(
                "clear managed XFS root project inheritance",
                &self.root,
                error,
            )
        })?;
        let assigned = get_fsxattr(&self.root_fd).map_err(|error| {
            FilesystemStorageError::io(
                "verify managed XFS root project attributes",
                &self.root,
                error,
            )
        })?;
        if assigned.fsx_projid != 0
            || assigned.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT != 0
        {
            return Err(FilesystemStorageError::verification(
                "verify managed XFS root has neutral project identity",
                &self.root,
            ));
        }
        Ok(())
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
        if usage.allocated_bytes != 0 || usage.filesystem_objects != 0 {
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

    fn project_usage(&self, project_id: u32) -> std::io::Result<AgentFilesystemUsage> {
        let quota = self.project_quota(project_id)?;
        usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)
    }

    fn project_quota(&self, project_id: u32) -> std::io::Result<FsDiskQuota> {
        let mut quota = FsDiskQuota::default();
        if let Err(error) = self.get_project_quota(project_id, &mut quota) {
            if matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH)) {
                quota.d_version = FS_DQUOT_VERSION;
                quota.d_flags = FS_PROJ_QUOTA;
                quota.d_id = project_id;
                return Ok(quota);
            }
            return Err(error);
        }
        if quota.d_version != FS_DQUOT_VERSION
            || quota.d_flags != FS_PROJ_QUOTA
            || quota.d_id != project_id
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "XFS returned an invalid project quota record",
            ));
        }
        Ok(quota)
    }

    fn clear_project_limits(&self, project_id: NonZeroU32) -> std::io::Result<()> {
        let mut quota = FsDiskQuota {
            d_version: FS_DQUOT_VERSION,
            d_flags: FS_PROJ_QUOTA,
            d_fieldmask: PROJECT_LIMIT_FIELDS,
            d_id: project_id.get(),
            ..FsDiskQuota::default()
        };
        self.set_project_quota_record(project_id, &mut quota)?;

        let mut cleared = FsDiskQuota::default();
        if let Err(error) = self.get_project_quota(project_id.get(), &mut cleared) {
            if matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH)) {
                return Ok(());
            }
            return Err(error);
        }
        if cleared.d_blk_hardlimit != 0
            || cleared.d_blk_softlimit != 0
            || cleared.d_ino_hardlimit != 0
            || cleared.d_ino_softlimit != 0
            || cleared.d_rtb_hardlimit != 0
            || cleared.d_rtb_softlimit != 0
            || cleared.d_itimer != 0
            || cleared.d_btimer != 0
            || cleared.d_rtbtimer != 0
            || cleared.d_iwarns != 0
            || cleared.d_bwarns != 0
            || cleared.d_rtbwarns != 0
            || cleared.d_itimer_hi != 0
            || cleared.d_btimer_hi != 0
            || cleared.d_rtbtimer_hi != 0
        {
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
        // SAFETY: Q_XGETQUOTA writes exactly one `fs_disk_quota` record.
        unsafe {
            self.quotactl_raw(
                Q_XGETQUOTA,
                project_id,
                std::ptr::from_mut(quota).cast::<libc::c_void>(),
            )
        }
    }

    fn set_project_quota_record(
        &self,
        project_id: NonZeroU32,
        quota: &mut FsDiskQuota,
    ) -> std::io::Result<()> {
        // SAFETY: Q_XSETQLIM reads exactly one `fs_disk_quota` record.
        unsafe {
            self.quotactl_raw(
                Q_XSETQLIM,
                project_id.get(),
                std::ptr::from_mut(quota).cast::<libc::c_void>(),
            )
        }
    }

    fn get_quota_state(&self, state: &mut FsQuotaStatV) -> std::io::Result<()> {
        // SAFETY: Q_XGETQSTATV writes exactly one `fs_quota_statv` record.
        unsafe {
            self.quotactl_raw(
                Q_XGETQSTATV,
                0,
                std::ptr::from_mut(state).cast::<libc::c_void>(),
            )
        }
    }

    unsafe fn quotactl_raw(
        &self,
        command: u32,
        id: u32,
        data: *mut libc::c_void,
    ) -> std::io::Result<()> {
        let operation = (command << 8) | (XQM_PRJQUOTA & 0xff);
        // SAFETY: The caller guarantees that `data` points to the UAPI
        // structure selected by `command` for the duration of the syscall.
        let result = unsafe {
            libc::syscall(
                libc::SYS_quotactl_fd,
                self.root_fd.as_raw_fd(),
                operation,
                id,
                data,
            )
        };
        if result == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
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
) -> std::io::Result<AgentFilesystemUsage> {
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
    Ok(AgentFilesystemUsage {
        allocated_bytes,
        filesystem_objects,
    })
}

fn errno_to_io(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn capacity_uses_fragment_size_and_executor_available_counts() {
        let capacity = capacity_from_values(10, 4, 4096, 20, 7).unwrap();

        assert_eq!(capacity.total_bytes, 40_960);
        assert_eq!(capacity.available_bytes, 16_384);
        assert_eq!(capacity.total_filesystem_objects, 20);
        assert_eq!(capacity.available_filesystem_objects, 7);
    }

    #[test]
    fn capacity_rejects_byte_overflow() {
        assert!(capacity_from_values(u64::MAX, 1, 2, 0, 0).is_err());
    }

    #[test]
    fn project_usage_uses_xfs_basic_block_units() {
        let usage = usage_from_quota_counts(3, 2, 2).unwrap();

        assert_eq!(usage.allocated_bytes, 2560);
        assert_eq!(usage.filesystem_objects, 2);
    }

    #[test]
    fn project_usage_rejects_combined_block_overflow() {
        assert!(usage_from_quota_counts(u64::MAX, 1, 0).is_err());
    }

    #[test]
    fn quota_abi_layout_matches_linux_uapi() {
        assert_eq!(std::mem::size_of::<FsDiskQuota>(), 112);
        assert_eq!(std::mem::align_of::<FsDiskQuota>(), 8);
        assert_eq!(std::mem::size_of::<FsQuotaStatV>(), 160);
        assert_eq!(std::mem::align_of::<FsQuotaStatV>(), 8);
    }

    #[test]
    fn project_quota_state_requires_accounting_and_enforcement() {
        let healthy = FsQuotaStatV {
            qs_version: FS_QSTATV_VERSION1,
            qs_flags: FS_QUOTA_PDQ_ACCT | FS_QUOTA_PDQ_ENFD,
            ..FsQuotaStatV::default()
        };
        assert!(validate_project_quota_state_record(&healthy).is_ok());

        for flags in [0, FS_QUOTA_PDQ_ACCT, FS_QUOTA_PDQ_ENFD] {
            let unhealthy = FsQuotaStatV {
                qs_version: FS_QSTATV_VERSION1,
                qs_flags: flags,
                ..FsQuotaStatV::default()
            };
            assert_eq!(
                validate_project_quota_state_record(&unhealthy)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
        }
    }

    #[test]
    fn project_attributes_require_expected_identity_and_inheritance() {
        let project_id = NonZeroU32::new(17).unwrap();
        let healthy = linux_raw_sys::general::fsxattr {
            fsx_xflags: linux_raw_sys::general::FS_XFLAG_PROJINHERIT,
            fsx_extsize: 0,
            fsx_nextents: 0,
            fsx_projid: project_id.get(),
            fsx_cowextsize: 0,
            fsx_pad: [0; 8],
        };
        assert!(validate_project_attributes(healthy, project_id).is_ok());

        let wrong_project = linux_raw_sys::general::fsxattr {
            fsx_projid: project_id.get() + 1,
            ..healthy
        };
        assert_eq!(
            validate_project_attributes(wrong_project, project_id)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );

        let no_inheritance = linux_raw_sys::general::fsxattr {
            fsx_xflags: 0,
            ..healthy
        };
        assert_eq!(
            validate_project_attributes(no_inheritance, project_id)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
