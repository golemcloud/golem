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
use crate::services::golem_config::FilesystemStorageMode;
use test_r::{test, timeout};

#[test]
fn stale_probe_removal_is_a_primary_failure_and_reports_the_configured_path() {
    let parent = tempfile::tempdir().unwrap();
    let file = parent.path().join("not-a-directory");
    std::fs::write(&file, b"sentinel").unwrap();
    let configured = Path::new("/configured/.golem-xfs-reflink-probe");
    let error = remove_probe(
        &file.join("probe"),
        configured,
        "remove stale XFS reflink probe",
    )
    .unwrap_err();
    assert!(!error.cleanup_failed());
    assert_eq!(error.context().path(), Some(configured));
    assert_eq!(
        error.context().io_kind(),
        Some(std::io::ErrorKind::NotADirectory)
    );
    assert_eq!(std::fs::read(file).unwrap(), b"sentinel");
}

const MOUNT_TABLE_FIXTURE: &str = "\
22 1 0:21 / / rw,relatime shared:1 - ext4 /dev/root rw
36 22 7:0 / /var/lib/golem/agents rw,relatime shared:2 - xfs /dev/loop0 rw,attr2,inode64,noquota
37 22 7:0 /agents /srv/bound rw,relatime shared:2 - xfs /dev/loop0 rw,attr2,inode64,noquota
38 22 7:1 / /var/lib/golem/managed rw,relatime - xfs /dev/loop1 rw,attr2,inode64,prjquota
39 22 7:2 / /var/lib/golem/accounted rw,relatime - xfs /dev/loop2 rw,usrquota,pqnoenforce
40 22 7:3 / /mnt/with\\040space rw - xfs /dev/loop3 rw,noquota
41 22 7:4 /dir\\040with\\040space /srv/bound\\040too rw - xfs /dev/loop4 rw,prjquota
";

fn project(id: u32) -> NonZeroU32 {
    NonZeroU32::new(id).unwrap()
}

fn reservations(entries: &[(u32, &str)]) -> HashMap<NonZeroU32, Box<Path>> {
    entries
        .iter()
        .map(|(id, owner)| (project(*id), Box::<Path>::from(Path::new(owner))))
        .collect()
}

fn managed_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

fn reflink_test_root() -> PathBuf {
    std::env::var_os("GOLEM_REFLINK_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_REFLINK_XFS_TEST_ROOT must name the mounted XFS test root without quotas")
}

fn without_reflink_test_root() -> PathBuf {
    std::env::var_os("GOLEM_XFS_WITHOUT_REFLINK_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_XFS_WITHOUT_REFLINK_TEST_ROOT must name an XFS test root without reflink")
}

fn reflink_storage(root: PathBuf) -> FilesystemStorageMode {
    FilesystemStorageMode::ReflinkXfs { root: root.into() }
}

fn bind_error(storage: &FilesystemStorageMode) -> String {
    match FilesystemVolume::open(storage, RetryConfig::default()) {
        Ok(_) => panic!("the storage mode must be refused at startup"),
        Err(error) => error.to_string(),
    }
}

/// Restores the project attributes of a directory when it is dropped, also after a failed
/// assertion.
struct RestoredProjectAttributes {
    directory: File,
    attributes: linux_raw_sys::general::fsxattr,
}

#[test]
fn managed_name_mode_proof_requires_xfs_filesystem_type() {
    let identity = FilesystemIdentity { device: 17 };
    let proof = validated_xfs_name_mode(XFS_SUPER_MAGIC, identity).unwrap();
    assert!(proof.matches_device(17));
    assert!(!proof.matches_device(18));
    assert!(validated_xfs_name_mode(0, identity).is_none());
}

#[test]
fn mount_line_gives_the_root_and_the_filesystem_options_of_the_mount() {
    assert_eq!(
        mount_line(MOUNT_TABLE_FIXTURE, 36),
        Some(MountLine {
            root: "/",
            super_options: "rw,attr2,inode64,noquota",
        })
    );
    assert_eq!(
        mount_line(MOUNT_TABLE_FIXTURE, 37).map(|line| line.root),
        Some("/agents")
    );
    assert_eq!(
        mount_line(MOUNT_TABLE_FIXTURE, 40),
        Some(MountLine {
            root: "/",
            super_options: "rw,noquota",
        })
    );
    assert_eq!(
        mount_line(MOUNT_TABLE_FIXTURE, 41),
        Some(MountLine {
            root: "/dir\\040with\\040space",
            super_options: "rw,prjquota",
        })
    );
    assert_eq!(mount_line(MOUNT_TABLE_FIXTURE, 99), None);
    assert_eq!(mount_line("36 22 7:0 / /a rw", 36), None);
    assert_eq!(mount_line("x 22 7:0 / /a rw - xfs /dev/loop0 rw", 36), None);
    assert_eq!(mount_line("36 22 7:0 / /a rw - xfs /dev/loop0", 36), None);
}

#[test]
fn an_xfs_root_is_a_mount_root_whose_mount_starts_at_the_filesystem_root() {
    let flag = StatxAttributes::MOUNT_ROOT;
    let valid = |attributes, mask, id| {
        xfs_root_location_is_valid(attributes, mask, mount_line(MOUNT_TABLE_FIXTURE, id))
    };

    assert!(valid(flag, flag, 36));
    assert!(!valid(flag, flag, 37));
    assert!(!valid(flag, flag, 99));
    assert!(!valid(StatxAttributes::empty(), flag, 36));
    assert!(!valid(flag, StatxAttributes::empty(), 36));
    assert!(!xfs_root_location_is_valid(flag, flag, None));
}

#[test]
fn project_quota_accounting_follows_the_xfs_quota_options() {
    let accounting = |id| {
        mount_line(MOUNT_TABLE_FIXTURE, id).map(|line| project_quota_accounting(line.super_options))
    };

    assert_eq!(accounting(36), Some(ProjectQuotaAccounting::Off));
    assert_eq!(accounting(38), Some(ProjectQuotaAccounting::On));
    assert_eq!(accounting(39), Some(ProjectQuotaAccounting::On));
    assert_eq!(
        project_quota_accounting("rw,usrquota,grpquota"),
        ProjectQuotaAccounting::Off
    );
    assert_eq!(
        project_quota_accounting("rw,prjquotas"),
        ProjectQuotaAccounting::Off
    );
}

#[test]
fn names_are_case_sensitive_unless_the_volume_folds_ascii_case() {
    assert!(names_are_case_sensitive(0));
    assert!(names_are_case_sensitive(!XFS_FSOP_GEOM_FLAGS_DIRV2CI));
    assert!(!names_are_case_sensitive(XFS_FSOP_GEOM_FLAGS_DIRV2CI));
    assert!(!names_are_case_sensitive(u32::MAX));
}

#[test]
fn project_of_owner_finds_the_project_of_that_owner_only() {
    let active = reservations(&[(3, "/a"), (7, "/b")]);

    assert_eq!(project_of_owner(&reservations(&[]), Path::new("/a")), None);
    assert_eq!(project_of_owner(&active, Path::new("/a")), Some(project(3)));
    assert_eq!(project_of_owner(&active, Path::new("/b")), Some(project(7)));
    assert_eq!(project_of_owner(&active, Path::new("/c")), None);
}

#[test]
fn an_existing_reservation_is_held_held_by_its_owner_or_free() {
    let active = reservations(&[(3, "/a")]);

    assert_eq!(
        existing_reservation(&active, project(3), Path::new("/a")),
        ExistingReservation::Held
    );
    assert_eq!(
        existing_reservation(&active, project(3), Path::new("/b")),
        ExistingReservation::HeldByOther(Path::new("/a"))
    );
    assert_eq!(
        existing_reservation(&active, project(4), Path::new("/b")),
        ExistingReservation::Free
    );
}

#[test]
fn the_next_free_project_scans_from_the_start_and_wraps_after_the_last_id() {
    let none = reservations(&[]);

    assert_eq!(next_free_project(0, &none), Some(project(1)));
    assert_eq!(next_free_project(1, &none), Some(project(1)));
    assert_eq!(next_free_project(5, &none), Some(project(5)));
    assert_eq!(
        next_free_project(5, &reservations(&[(5, "/a"), (6, "/b")])),
        Some(project(7))
    );
    assert_eq!(
        next_free_project(u32::MAX, &reservations(&[(u32::MAX, "/a")])),
        Some(project(1))
    );
    assert_eq!(
        next_free_project(
            u32::MAX - 1,
            &reservations(&[(u32::MAX - 1, "/a"), (u32::MAX, "/b"), (1, "/c")])
        ),
        Some(project(2))
    );
    assert_eq!(next_free_project(u32::MAX, &none), Some(project(u32::MAX)));
}

#[test]
fn the_scan_after_a_project_starts_at_the_next_id_and_wraps_to_one() {
    assert_eq!(project_after(project(1)), 2);
    assert_eq!(project_after(project(41)), 42);
    assert_eq!(project_after(project(u32::MAX)), 1);
}

#[test]
fn a_candidate_repeats_only_when_it_is_the_first_candidate() {
    assert!(!candidate_repeats(None, project(3)));
    assert!(candidate_repeats(Some(project(3)), project(3)));
    assert!(!candidate_repeats(Some(project(3)), project(4)));
}

#[test]
fn a_project_is_empty_only_without_bytes_and_objects() {
    let usage = |allocated_bytes, filesystem_objects| FilesystemAllocation {
        allocated_bytes,
        filesystem_objects,
    };

    assert!(project_is_empty(usage(0, 0)));
    assert!(!project_is_empty(usage(1, 0)));
    assert!(!project_is_empty(usage(0, 1)));
    assert!(!project_is_empty(usage(4096, 2)));
}

#[test]
fn a_quota_record_is_missing_only_for_enoent_and_esrch() {
    assert_eq!(
        [
            libc::ENOENT,
            libc::ESRCH,
            libc::EPERM,
            libc::EINVAL,
            libc::ENOSYS,
        ]
        .map(|errno| quota_record_is_missing(&std::io::Error::from_raw_os_error(errno))),
        [true, true, false, false, false]
    );
    assert!(!quota_record_is_missing(&std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "not an OS error"
    )));
}

#[test]
fn the_quota_record_of_a_missing_project_has_its_id_and_nothing_else() {
    let quota = empty_project_quota(7);

    assert_eq!(quota.d_version, FS_DQUOT_VERSION);
    assert_eq!(quota.d_flags, FS_PROJ_QUOTA);
    assert_eq!(quota.d_id, 7);
    assert_eq!(quota.d_fieldmask, 0);
    assert!(!project_quota_retains_state(&quota));
    assert_eq!(
        (quota.d_bcount, quota.d_icount, quota.d_rtbcount),
        (0, 0, 0)
    );
    assert!(project_quota_record_is_valid(&quota, 7));
}

#[test]
fn a_project_quota_record_is_valid_only_with_its_version_flags_and_id() {
    let valid = empty_project_quota(7);
    let changed: [fn(&mut FsDiskQuota); 3] = [
        |quota| quota.d_version = FS_DQUOT_VERSION + 1,
        |quota| quota.d_flags = FS_PROJ_QUOTA << 1,
        |quota| quota.d_id = 8,
    ];

    assert!(project_quota_record_is_valid(&valid, 7));
    assert!(!project_quota_record_is_valid(&valid, 8));
    changed.iter().for_each(|change| {
        let mut quota = valid;
        change(&mut quota);
        assert!(!project_quota_record_is_valid(&quota, 7));
    });
}

#[test]
fn a_project_quota_retains_state_when_any_limit_timer_or_warning_is_set() {
    let set: [fn(&mut FsDiskQuota); 15] = [
        |quota| quota.d_blk_hardlimit = 1,
        |quota| quota.d_blk_softlimit = 1,
        |quota| quota.d_ino_hardlimit = 1,
        |quota| quota.d_ino_softlimit = 1,
        |quota| quota.d_rtb_hardlimit = 1,
        |quota| quota.d_rtb_softlimit = 1,
        |quota| quota.d_itimer = 1,
        |quota| quota.d_btimer = 1,
        |quota| quota.d_rtbtimer = 1,
        |quota| quota.d_iwarns = 1,
        |quota| quota.d_bwarns = 1,
        |quota| quota.d_rtbwarns = 1,
        |quota| quota.d_itimer_hi = 1,
        |quota| quota.d_btimer_hi = 1,
        |quota| quota.d_rtbtimer_hi = 1,
    ];
    let usage_only = FsDiskQuota {
        d_bcount: 8,
        d_icount: 1,
        d_rtbcount: 8,
        ..FsDiskQuota::default()
    };

    assert!(!project_quota_retains_state(&FsDiskQuota::default()));
    assert!(!project_quota_retains_state(&usage_only));
    set.iter().enumerate().for_each(|(field, change)| {
        let mut quota = FsDiskQuota::default();
        change(&mut quota);
        assert!(
            project_quota_retains_state(&quota),
            "field {field} of the table is not seen"
        );
    });
}

#[test]
fn limits_are_representable_only_when_positive_and_whole_blocks() {
    let block = NonZeroU64::new(4096).unwrap();
    let limits = |allocated_bytes, filesystem_objects| FilesystemLimits {
        allocated_bytes,
        filesystem_objects,
    };

    assert!(limits_are_representable(limits(8192, 1), block));
    assert!(limits_are_representable(limits(4096, 100), block));
    assert!(!limits_are_representable(limits(0, 1), block));
    assert!(!limits_are_representable(limits(4097, 1), block));
    assert!(!limits_are_representable(limits(2048, 1), block));
    assert!(!limits_are_representable(limits(8192, 0), block));
}

#[test]
fn a_project_quota_holds_limits_only_with_both_hard_limits_and_no_soft_limit() {
    let holding = FsDiskQuota {
        d_blk_hardlimit: 16,
        d_ino_hardlimit: 100,
        d_bcount: 8,
        ..FsDiskQuota::default()
    };
    let change: [fn(&mut FsDiskQuota); 4] = [
        |quota| quota.d_blk_hardlimit = 15,
        |quota| quota.d_blk_softlimit = 1,
        |quota| quota.d_ino_hardlimit = 99,
        |quota| quota.d_ino_softlimit = 1,
    ];

    assert!(project_quota_holds_limits(&holding, 16, 100));
    assert!(!project_quota_holds_limits(&holding, 17, 100));
    assert!(!project_quota_holds_limits(&holding, 16, 101));
    change.iter().enumerate().for_each(|(field, change)| {
        let mut quota = holding;
        change(&mut quota);
        assert!(
            !project_quota_holds_limits(&quota, 16, 100),
            "field {field} of the table is not seen"
        );
    });
}

#[test]
fn geometry_abi_layout_matches_linux_uapi() {
    assert_eq!(std::mem::size_of::<XfsGeometryV1>(), 112);
    assert_eq!(std::mem::align_of::<XfsGeometryV1>(), 8);
    assert_eq!(std::mem::offset_of!(XfsGeometryV1, flags), 92);
    assert_eq!(XFS_IOC_FSGEOMETRY_V1, 0x8070_5864);
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
fn stale_project_is_the_project_that_exists_and_refuses_two_different_ones() {
    let one = NonZeroU32::new(1).unwrap();
    let two = NonZeroU32::new(2).unwrap();
    [
        (None, None, Ok(None)),
        (Some(one), None, Ok(Some(one))),
        (None, Some(two), Ok(Some(two))),
        (Some(one), Some(one), Ok(Some(one))),
        (Some(one), Some(two), Err(ProjectMismatch)),
        (Some(two), Some(one), Err(ProjectMismatch)),
    ]
    .into_iter()
    .for_each(|(disk, reserved, expected)| {
        assert_eq!(
            stale_project(disk, reserved),
            expected,
            "disk {disk:?}, reserved {reserved:?}"
        );
    });
}

#[test]
fn has_project_identity_needs_a_project_id_or_the_inherit_flag() {
    let inherit = linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
    let attributes = |projid: u32, xflags: u32| {
        // SAFETY: `fsxattr` is a plain C struct of integers, and all zero bits are a valid
        // value of it.
        let mut attributes: linux_raw_sys::general::fsxattr = unsafe { std::mem::zeroed() };
        attributes.fsx_projid = projid;
        attributes.fsx_xflags = xflags;
        attributes
    };
    [
        (0, 0, false),
        (0, 0x1, false),
        (7, 0, true),
        (0, inherit, true),
        (7, inherit, true),
        (0, inherit | 0x1, true),
    ]
    .into_iter()
    .for_each(|(projid, xflags, expected)| {
        assert_eq!(
            has_project_identity(&attributes(projid, xflags)),
            expected,
            "project id {projid} with flags {xflags:#x}"
        );
    });
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("60s")]
async fn xfs_without_reflink_is_refused_at_startup() {
    let root = without_reflink_test_root();
    let error = bind_error(&reflink_storage(root.clone()));

    assert!(error.contains("validate XFS reflink support"), "{error}");
    assert!(error.contains("Operation not supported"), "{error}");
    assert!(!root.join(REFLINK_PROBE).exists());
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("60s")]
async fn reflink_xfs_refuses_a_volume_with_project_quota_accounting() {
    let error = bind_error(&reflink_storage(managed_test_root()));

    assert!(
        error.contains("verify XFS volume has no project quota accounting"),
        "{error}"
    );
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("60s")]
async fn reflink_xfs_refuses_a_root_with_a_project_identity() {
    let root = reflink_test_root();
    let directory = File::open(&root).unwrap();
    let original = get_fsxattr(&directory).unwrap();
    let _restored = RestoredProjectAttributes {
        directory: directory.try_clone().unwrap(),
        attributes: original,
    };
    let mut with_project = original;
    with_project.fsx_projid = 7;
    with_project.fsx_xflags |= linux_raw_sys::general::FS_XFLAG_PROJINHERIT;
    set_fsxattr(&directory, with_project).unwrap();

    let error = match FilesystemVolume::open(&reflink_storage(root.clone()), RetryConfig::default())
    {
        Ok(_) => panic!("a root with a project identity must be refused"),
        Err(error) => error.to_string(),
    };

    assert!(
        error.contains("verify XFS root has no project identity"),
        "{error}"
    );
}

impl Drop for RestoredProjectAttributes {
    fn drop(&mut self) {
        if let Err(error) = set_fsxattr(&self.directory, self.attributes) {
            eprintln!("failed to restore the project attributes of the test root: {error}");
        }
    }
}
