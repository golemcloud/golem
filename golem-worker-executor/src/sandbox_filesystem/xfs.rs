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
use rustix::fs::{
    AtFlags, FlockOperation, Mode, OFlags, StatVfsMountFlags, StatxAttributes, StatxFlags, flock,
    fstatfs, fstatvfs, ioctl_ficlone, mkdirat, openat, statx,
};
use rustix::ioctl::{Getter, Setter, ioctl};
use std::collections::HashMap;
use std::fs::File;
use std::num::NonZeroU32;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const XFS_SUPER_MAGIC: u64 = 0x5846_5342;
// XFS reserves inode 128 for the filesystem root.
const XFS_ROOT_INODE: u64 = 128;
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

#[derive(Clone, Copy)]
pub(super) struct ValidatedManagedXfsNameMode {
    identity: FilesystemIdentity,
}

impl ValidatedManagedXfsNameMode {
    pub(super) fn matches_device(self, device: u64) -> bool {
        self.identity.device == device
    }
}

#[cfg(test)]
pub(super) fn validated_managed_xfs_name_mode_for_test(device: u64) -> ValidatedManagedXfsNameMode {
    validated_managed_xfs_name_mode(XFS_SUPER_MAGIC, FilesystemIdentity { device })
        .expect("XFS filesystem type must produce a managed name-mode proof")
}

fn validated_managed_xfs_name_mode(
    filesystem_type: u64,
    identity: FilesystemIdentity,
) -> Option<ValidatedManagedXfsNameMode> {
    (filesystem_type == XFS_SUPER_MAGIC).then_some(ValidatedManagedXfsNameMode { identity })
}

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

#[derive(Clone)]
pub(super) struct ManagedProvisioning {
    volume: FilesystemVolume,
    root: PathBuf,
    root_fd: Arc<File>,
    allocator: Arc<Mutex<ProjectAllocator>>,
    filesystem_block_bytes: NonZeroU64,
    validated_name_mode: ValidatedManagedXfsNameMode,
    cleanup_retry: RetryConfig,
}

impl ManagedProvisioning {
    pub(super) fn new(
        root: &Path,
        cleanup_retry: &RetryConfig,
    ) -> Result<Self, FilesystemStorageError> {
        let root_fd = File::open(root)
            .map_err(|error| FilesystemStorageError::io("open managed XFS root", root, error))?;
        let filesystem = fstatfs(&root_fd).map_err(|error| {
            FilesystemStorageError::io("inspect managed XFS root", root, errno_to_io(error))
        })?;
        if filesystem.f_type as u64 != XFS_SUPER_MAGIC {
            return Err(FilesystemStorageError::verification(
                "validate managed XFS root filesystem type",
                root,
            ));
        }
        validate_managed_xfs_root_location(&root_fd, root)?;
        flock(&root_fd, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            FilesystemStorageError::io(
                "acquire exclusive ownership of managed XFS root",
                root,
                errno_to_io(error),
            )
        })?;
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

        let identity = filesystem_identity(&root_fd).map_err(|error| {
            FilesystemStorageError::io("identify managed XFS root", root, error)
        })?;
        let validated_name_mode =
            validated_managed_xfs_name_mode(filesystem.f_type as u64, identity)
                .expect("validated XFS filesystem type must produce a name-mode proof");
        let root_fd = Arc::new(root_fd);
        let backend = Self {
            volume: FilesystemVolume::managed(Arc::clone(&root_fd), identity),
            root: stable_root,
            root_fd,
            allocator: Arc::new(Mutex::new(ProjectAllocator {
                next: 1,
                active: HashMap::new(),
            })),
            filesystem_block_bytes: NonZeroU64::new(filesystem_block_bytes)
                .expect("validated XFS filesystem block size must be nonzero"),
            validated_name_mode,
            cleanup_retry: cleanup_retry.clone(),
        };
        backend.clear_root_project_assignment()?;
        backend.validate_project_quota_state()?;
        backend.validate_project_assignment(cleanup_retry)?;

        Ok(backend)
    }

    pub(super) fn volume(&self) -> &FilesystemVolume {
        &self.volume
    }

    pub(super) fn root(&self) -> &Path {
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

    fn project_usage(&self, project_id: u32) -> std::io::Result<FilesystemAllocation> {
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

fn validate_managed_xfs_root_location(
    root_fd: &File,
    root: &Path,
) -> Result<(), FilesystemStorageError> {
    let location = statx(
        root_fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::empty(),
    )
    .map_err(|error| {
        FilesystemStorageError::io(
            "inspect managed XFS root mount location",
            root,
            errno_to_io(error),
        )
    })?;
    let inode = root_fd.metadata().map_err(|error| {
        FilesystemStorageError::io("inspect managed XFS root inode", root, error)
    })?;
    if !managed_xfs_root_location_is_valid(
        inode.ino(),
        location.stx_attributes,
        location.stx_attributes_mask,
    ) {
        return Err(FilesystemStorageError::verification(
            "validate managed XFS root is the filesystem mount root",
            root,
        ));
    }
    Ok(())
}

fn managed_xfs_root_location_is_valid(
    inode: u64,
    attributes: StatxAttributes,
    attributes_mask: StatxAttributes,
) -> bool {
    inode == XFS_ROOT_INODE
        && attributes_mask.contains(StatxAttributes::MOUNT_ROOT)
        && attributes.contains(StatxAttributes::MOUNT_ROOT)
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
            "managed XFS mount is read-only",
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
            "managed filesystem capacity authority identity changed",
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

pub(super) fn reflink_file(
    root: &Path,
    project_id: NonZeroU32,
    source: &Path,
    target: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    let parent = create_copy_parent(root, target)?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let source = File::open(source)?;
    if NonZeroU32::new(get_fsxattr(temporary.as_file())?.fsx_projid) != Some(project_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed XFS file-copy destination did not inherit its project identity",
        ));
    }
    ioctl_ficlone(temporary.as_file(), &source).map_err(errno_to_io)?;
    temporary.as_file().sync_all()?;
    set_file_permissions(temporary.as_file(), read_only)?;
    temporary
        .persist_noclobber(target)
        .map_err(|error| error.error)?;
    rustix::fs::syncfs(&File::open(root)?).map_err(errno_to_io)
}

pub(super) fn reflink_file_at(
    root: &Path,
    project_id: NonZeroU32,
    destination_directory: &cap_std::fs::Dir,
    source: &Path,
    destination: &Path,
    read_only: bool,
) -> std::io::Result<()> {
    let (parent, destination) = create_capability_copy_parent(destination_directory, destination)?;
    let temporary = CapabilityTempFile::new(parent)?;
    let temporary_file = temporary.as_file().try_clone()?.into_std();
    let source = File::open(source)?;
    if NonZeroU32::new(get_fsxattr(&temporary_file)?.fsx_projid) != Some(project_id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "managed XFS file-copy destination did not inherit its project identity",
        ));
    }
    ioctl_ficlone(&temporary_file, &source).map_err(errno_to_io)?;
    temporary_file.sync_all()?;
    set_file_permissions(&temporary_file, read_only)?;
    temporary.persist_noclobber(&destination)?;
    rustix::fs::syncfs(&File::open(root)?).map_err(errno_to_io)
}

pub(super) fn project_allocation(
    volume: &FilesystemVolume,
    project_id: NonZeroU32,
) -> std::io::Result<FilesystemAllocation> {
    let quota = project_quota(volume_root(volume), project_id.get())?;
    usage_from_quota_counts(quota.d_bcount, quota.d_rtbcount, quota.d_icount)
}

pub(super) fn install_project_limits(
    volume: &FilesystemVolume,
    project_id: NonZeroU32,
    filesystem_block_bytes: NonZeroU64,
    limits: FilesystemLimits,
) -> std::io::Result<()> {
    if limits.allocated_bytes == 0
        || !limits
            .allocated_bytes
            .is_multiple_of(filesystem_block_bytes.get())
        || limits.filesystem_objects == 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "filesystem limits are not exactly representable by managed XFS",
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
    set_project_quota_record(volume_root(volume), project_id, &mut quota)?;

    let installed = project_quota(volume_root(volume), project_id.get())?;
    if installed.d_blk_hardlimit != block_hard_limit
        || installed.d_blk_softlimit != 0
        || installed.d_ino_hardlimit != limits.filesystem_objects
        || installed.d_ino_softlimit != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("XFS project {project_id} did not retain the complete quota limit pair"),
        ));
    }
    Ok(())
}

fn volume_root(volume: &FilesystemVolume) -> &File {
    volume
        .managed_root()
        .expect("managed XFS operation requires a managed volume")
}

fn project_quota(root: &File, project_id: u32) -> std::io::Result<FsDiskQuota> {
    let mut quota = FsDiskQuota::default();
    if let Err(error) = get_project_quota(root, project_id, &mut quota) {
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

fn get_project_quota(root: &File, project_id: u32, quota: &mut FsDiskQuota) -> std::io::Result<()> {
    unsafe {
        quotactl_raw(
            root,
            Q_XGETQUOTA,
            project_id,
            std::ptr::from_mut(quota).cast::<libc::c_void>(),
        )
    }
}

fn set_project_quota_record(
    root: &File,
    project_id: NonZeroU32,
    quota: &mut FsDiskQuota,
) -> std::io::Result<()> {
    unsafe {
        quotactl_raw(
            root,
            Q_XSETQLIM,
            project_id.get(),
            std::ptr::from_mut(quota).cast::<libc::c_void>(),
        )
    }
}

unsafe fn quotactl_raw(
    root: &File,
    command: u32,
    id: u32,
    data: *mut libc::c_void,
) -> std::io::Result<()> {
    let operation = (command << 8) | (XQM_PRJQUOTA & 0xff);
    let result =
        unsafe { libc::syscall(libc::SYS_quotactl_fd, root.as_raw_fd(), operation, id, data) };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn space_from_values(
    blocks: u64,
    available_blocks: u64,
    fragment_size: u64,
    filesystem_objects: u64,
    available_filesystem_objects: u64,
) -> std::io::Result<FilesystemSpace> {
    let total_bytes = blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem total capacity exceeds u64",
        )
    })?;
    let available_bytes = available_blocks.checked_mul(fragment_size).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "filesystem available capacity exceeds u64",
        )
    })?;
    Ok(FilesystemSpace::Observed {
        total_bytes,
        available_bytes,
        total_filesystem_objects: filesystem_objects,
        available_filesystem_objects,
    })
}

impl ManagedProvisioning {
    pub(super) async fn create_fresh(
        &self,
        volume: FilesystemVolume,
        name: SandboxFilesystemName,
    ) -> Result<Arc<SandboxFilesystem>, FilesystemStorageError> {
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
    ) -> Result<Arc<SandboxFilesystem>, FilesystemStorageError> {
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
        let stale_project = match (disk_project, reserved_project) {
            (Some(disk_project), Some(reserved_project)) if disk_project != reserved_project => {
                return Err(FilesystemStorageError::cleanup_verification(
                    "match stale managed XFS path and reserved project",
                    &cleanup_path,
                ));
            }
            (disk_project, reserved_project) => disk_project.or(reserved_project),
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
        let created = Arc::new(SandboxFilesystem::new(
            NativeRoot::new(root, root_fd),
            LeaseState {
                lifecycle: lifecycle.expect("managed XFS lifecycle owner must exist"),
                cleanup: NativeCleanup::Managed(ManagedProjectCleanup {
                    project: ProjectCleanup::new(
                        self.clone(),
                        project_id,
                        cleanup_path,
                        self.cleanup_retry.clone(),
                    ),
                    _parent: parent,
                }),
            },
            volume,
            FileCopyMode::Reflink,
            QuotaAuthority::Project {
                project_id,
                filesystem_block_bytes: self.filesystem_block_bytes,
            },
            NativeNameModeSource::ValidatedManagedXfs(self.validated_name_mode),
        ));
        if let Err(error) = assignment_result {
            return Err(match SandboxFilesystem::delete_and_verify(&created).await {
                Ok(()) => error,
                Err(cleanup_error) => cleanup_error,
            });
        }
        if let Err(error) = verify_fresh_open_directory(created.root()).await {
            return Err(match SandboxFilesystem::delete_and_verify(&created).await {
                Ok(()) => error,
                Err(cleanup_error) => cleanup_error,
            });
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
        remove_and_verify(
            &self.project.path,
            "delete managed XFS runtime directory",
            &self.project.cleanup_retry,
        )
        .await?;
        self.project.finish().await
    }

    pub(super) fn delete_blocking(&mut self) -> Result<(), FilesystemStorageError> {
        self.project.remove_and_finish_blocking()
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
                failure.cleanup_failed = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::time::Duration;
    use test_r::{test, timeout};

    #[test]
    fn space_uses_fragment_size_and_executor_available_counts() {
        let space = space_from_values(10, 4, 4096, 20, 7).unwrap();

        assert_eq!(
            space,
            FilesystemSpace::Observed {
                total_bytes: 40_960,
                available_bytes: 16_384,
                total_filesystem_objects: 20,
                available_filesystem_objects: 7,
            }
        );
    }

    #[test]
    fn space_rejects_byte_overflow() {
        assert!(space_from_values(u64::MAX, 1, 2, 0, 0).is_err());
    }

    #[test]
    fn managed_name_mode_proof_requires_xfs_filesystem_type() {
        let identity = FilesystemIdentity { device: 17 };
        let proof = validated_managed_xfs_name_mode(XFS_SUPER_MAGIC, identity).unwrap();
        assert!(proof.matches_device(17));
        assert!(!proof.matches_device(18));
        assert!(validated_managed_xfs_name_mode(0, identity).is_none());
    }

    #[test]
    fn managed_root_must_be_the_xfs_filesystem_mount_root() {
        let mount_root = StatxAttributes::MOUNT_ROOT;

        assert!(managed_xfs_root_location_is_valid(
            XFS_ROOT_INODE,
            mount_root,
            mount_root,
        ));
        assert!(!managed_xfs_root_location_is_valid(
            XFS_ROOT_INODE + 1,
            mount_root,
            mount_root,
        ));
        assert!(!managed_xfs_root_location_is_valid(
            XFS_ROOT_INODE,
            StatxAttributes::empty(),
            mount_root,
        ));
        assert!(!managed_xfs_root_location_is_valid(
            XFS_ROOT_INODE,
            StatxAttributes::empty(),
            StatxAttributes::empty(),
        ));
    }

    #[test]
    fn capacity_authority_requires_the_expected_filesystem_identity() {
        let directory = tempfile::tempdir().unwrap();
        let root = File::open(directory.path()).unwrap();
        let identity = filesystem_identity(&root).unwrap();

        assert!(matches!(
            observe_space(&root, identity).unwrap(),
            FilesystemSpace::Observed { .. }
        ));

        let mismatched = FilesystemIdentity {
            device: identity.device.wrapping_add(1),
        };
        assert_eq!(
            observe_space(&root, mismatched).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
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

    #[test]
    #[ignore = "requires the privileged managed XFS test runner"]
    #[timeout("60s")]
    async fn managed_xfs_sandbox_filesystem_owns_allocation_limits_and_cleanup() {
        use std::collections::BTreeSet;

        let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
            .map(PathBuf::from)
            .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
        let provisioning =
            SandboxFilesystemProvisioning::new(None, Some(root.clone()), RetryConfig::default())
                .unwrap();
        assert!(
            SandboxFilesystemProvisioning::new(None, Some(root.clone()), RetryConfig::default())
                .is_err()
        );

        let source = root.join(format!(".sandbox-filesystem-source-{}", std::process::id()));
        let _ = std::fs::remove_file(&source);
        std::fs::write(&source, vec![0x5a; 8192]).unwrap();
        let source_descriptor = File::open(&source).unwrap();
        assert_eq!(file_project_id(&source_descriptor).unwrap(), None);
        drop(source_descriptor);

        let filesystem = provisioning
            .create_fresh(
                SandboxFilesystemName::new(
                    "native-test-environment".to_string(),
                    "native-test-component".to_string(),
                    format!("native-test-filesystem-{}", std::process::id()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let name_mode_parent = filesystem.root().join("name-mode-parent");
        std::fs::create_dir(&name_mode_parent).unwrap();
        filesystem
            .resolve_namespace_target(SandboxPath::at_root("name-mode-parent/child"))
            .await
            .unwrap();
        assert_eq!(filesystem.name_mode_probe_count(), 0);
        std::fs::remove_dir(name_mode_parent).unwrap();

        let unmanaged_parent = tempfile::tempdir().unwrap();
        let unmanaged = SandboxFilesystemProvisioning::new(
            Some(unmanaged_parent.path().to_path_buf()),
            None,
            RetryConfig::default(),
        )
        .unwrap()
        .create_fresh(
            SandboxFilesystemName::new(
                "native-test-unmanaged-environment".to_string(),
                "native-test-unmanaged-component".to_string(),
                format!("native-test-unmanaged-filesystem-{}", std::process::id()),
            )
            .unwrap(),
        )
        .await
        .unwrap();
        unmanaged
            .resolve_namespace_target(SandboxPath::at_root("name-mode-child"))
            .await
            .unwrap();
        assert_eq!(unmanaged.name_mode_probe_count(), 1);
        SandboxFilesystem::delete_and_verify(&unmanaged)
            .await
            .unwrap();

        let project_id = filesystem.project_id_for_test();
        let copied = filesystem.root().join("copied");
        <SandboxFilesystem as SandboxFilesystemAdapter>::seed_file(
            &filesystem,
            &source,
            SandboxPath::at_root("copied"),
            SandboxFilePermissions::ReadWrite,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&copied).unwrap(), vec![0x5a; 8192]);
        let copied_descriptor = File::open(&copied).unwrap();
        assert_eq!(
            file_project_id(&copied_descriptor).unwrap(),
            Some(project_id)
        );
        drop(copied_descriptor);

        let allocation = filesystem.observe_allocation().await.unwrap().unwrap();
        assert!(allocation.allocated_bytes >= 8192);
        assert!(allocation.filesystem_objects >= 2);
        let limits = FilesystemLimits {
            allocated_bytes: 128 * 1024 * 1024,
            filesystem_objects: 8192,
        };
        let installed = filesystem.install_limits(limits).await.unwrap();
        assert_eq!(installed.limits, limits);

        let sparse_path = filesystem.root().join("sparse");
        let sparse = File::create(&sparse_path).unwrap();
        sparse.set_len(16 * 1024 * 1024).unwrap();
        sparse.sync_all().unwrap();
        let sparse_allocation = filesystem.observe_allocation().await.unwrap().unwrap();
        assert_eq!(
            std::fs::metadata(&sparse_path).unwrap().len(),
            16 * 1024 * 1024
        );
        assert!(
            sparse_allocation.allocated_bytes < 16 * 1024 * 1024,
            "sparse logical length was treated as allocated bytes"
        );
        drop(sparse);

        let mut copied_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&copied)
            .unwrap();
        copied_file.seek(SeekFrom::Start(0)).unwrap();
        copied_file.write_all(b"COW!").unwrap();
        copied_file.sync_all().unwrap();
        drop(copied_file);
        assert_eq!(&std::fs::read(&source).unwrap()[..4], b"ZZZZ");
        assert_eq!(&std::fs::read(&copied).unwrap()[..4], b"COW!");

        let before_open_unlinked = filesystem.observe_allocation().await.unwrap().unwrap();
        let open_unlinked_path = filesystem.root().join("open-unlinked");
        let mut open_unlinked = File::create(&open_unlinked_path).unwrap();
        open_unlinked.write_all(&vec![0x3c; 1024 * 1024]).unwrap();
        open_unlinked.sync_all().unwrap();
        std::fs::remove_file(&open_unlinked_path).unwrap();
        let retained_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let while_open_unlinked = filesystem.observe_allocation().await.unwrap().unwrap();
            if while_open_unlinked.allocated_bytes > before_open_unlinked.allocated_bytes
                && while_open_unlinked.filesystem_objects > before_open_unlinked.filesystem_objects
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < retained_deadline,
                "open-unlinked allocation was not retained while its descriptor remained open: before={before_open_unlinked:?}, current={while_open_unlinked:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        drop(open_unlinked);
        let release_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let after_close = filesystem.observe_allocation().await.unwrap().unwrap();
            if after_close.allocated_bytes <= before_open_unlinked.allocated_bytes
                && after_close.filesystem_objects <= before_open_unlinked.filesystem_objects
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < release_deadline,
                "open-unlinked allocation was not released after closing its final descriptor: before={before_open_unlinked:?}, current={after_close:?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let second = provisioning
            .create_fresh(
                SandboxFilesystemName::new(
                    "native-test-environment".to_string(),
                    "native-test-component".to_string(),
                    format!("native-test-second-filesystem-{}", std::process::id()),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let second_project_id = second.project_id_for_test();
        let hard_link_error = std::fs::hard_link(&copied, second.root().join("cross-project-link"))
            .expect_err("XFS must reject hard links across project ownership domains");
        assert!(matches!(hard_link_error.raw_os_error(), Some(libc::EXDEV)));

        let current = filesystem.observe_allocation().await.unwrap().unwrap();
        let byte_limited = FilesystemLimits {
            allocated_bytes: current.allocated_bytes + 2 * 1024 * 1024,
            filesystem_objects: current.filesystem_objects + 32,
        };
        filesystem.install_limits(byte_limited).await.unwrap();
        let quota_path = filesystem.root().join("byte-quota");
        let mut quota_file = File::create(&quota_path).unwrap();
        let quota_error = quota_file
            .write_all(&vec![0x6b; 4 * 1024 * 1024])
            .expect_err("project byte quota did not stop allocation");
        assert!(matches!(
            quota_error.raw_os_error(),
            Some(libc::EDQUOT) | Some(libc::ENOSPC)
        ));

        let raised_byte_limit = FilesystemLimits {
            allocated_bytes: byte_limited.allocated_bytes + 4 * 1024 * 1024,
            filesystem_objects: byte_limited.filesystem_objects,
        };
        filesystem.install_limits(raised_byte_limit).await.unwrap();
        quota_file.write_all(&vec![0x7c; 512 * 1024]).unwrap();
        quota_file.sync_all().unwrap();
        drop(quota_file);

        let current = filesystem.observe_allocation().await.unwrap().unwrap();
        let object_limited = FilesystemLimits {
            allocated_bytes: raised_byte_limit.allocated_bytes + 4 * 1024 * 1024,
            filesystem_objects: current.filesystem_objects + 2,
        };
        filesystem.install_limits(object_limited).await.unwrap();
        let mut object_quota_error = None;
        let mut object_quota_files = Vec::new();
        for index in 0..8 {
            let name = format!("object-quota-{index}");
            match File::create(filesystem.root().join(&name)) {
                Ok(file) => {
                    drop(file);
                    object_quota_files.push(name);
                }
                Err(error) => {
                    object_quota_error = Some(error);
                    break;
                }
            }
        }
        let object_quota_error =
            object_quota_error.expect("project filesystem-object quota did not stop creation");
        assert!(matches!(
            object_quota_error.raw_os_error(),
            Some(libc::EDQUOT) | Some(libc::ENOSPC)
        ));
        filesystem
            .install_limits(FilesystemLimits {
                allocated_bytes: object_limited.allocated_bytes,
                filesystem_objects: object_limited.filesystem_objects + 4,
            })
            .await
            .unwrap();
        let object_after_limit_raise =
            File::create(filesystem.root().join("object-after-limit-raise")).unwrap();
        drop(object_after_limit_raise);

        let mut expected_linked_names = BTreeSet::from([
            "byte-quota".to_string(),
            "copied".to_string(),
            "object-after-limit-raise".to_string(),
            "sparse".to_string(),
        ]);
        expected_linked_names.extend(object_quota_files);
        let linked_names = std::fs::read_dir(filesystem.root())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(linked_names, expected_linked_names);
        let after_descriptor_drops = filesystem.observe_allocation().await.unwrap().unwrap();
        assert_eq!(
            after_descriptor_drops.filesystem_objects,
            u64::try_from(linked_names.len()).unwrap() + 1,
            "managed project usage retained an unlinked object after all test descriptors were dropped"
        );
        assert!(after_descriptor_drops.allocated_bytes >= 8192);

        let filesystem_root = filesystem.root().to_path_buf();
        SandboxFilesystem::delete_and_verify(&filesystem)
            .await
            .unwrap();
        assert!(!filesystem_root.exists());
        assert_eq!(
            provisioning
                .project_allocation_for_test(project_id)
                .unwrap(),
            FilesystemAllocation {
                allocated_bytes: 0,
                filesystem_objects: 0,
            }
        );
        let second_root = second.root().to_path_buf();
        SandboxFilesystem::delete_and_verify(&second).await.unwrap();
        assert!(!second_root.exists());
        assert_eq!(
            provisioning
                .project_allocation_for_test(second_project_id)
                .unwrap(),
            FilesystemAllocation {
                allocated_bytes: 0,
                filesystem_objects: 0,
            }
        );
        std::fs::remove_file(source).unwrap();
    }
}
