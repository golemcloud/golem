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
use std::io::{Seek, SeekFrom, Write};
use std::time::Duration;
use test_r::{test, timeout};

#[test]
fn managed_name_mode_proof_requires_xfs_filesystem_type() {
    let identity = FilesystemIdentity { device: 17 };
    let proof = validated_xfs_name_mode(XFS_SUPER_MAGIC, identity).unwrap();
    assert!(proof.matches_device(17));
    assert!(!proof.matches_device(18));
    assert!(validated_xfs_name_mode(0, identity).is_none());
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

fn project(id: u32) -> NonZeroU32 {
    NonZeroU32::new(id).unwrap()
}

fn reservations(entries: &[(u32, &str)]) -> HashMap<NonZeroU32, Box<Path>> {
    entries
        .iter()
        .map(|(id, owner)| (project(*id), Box::<Path>::from(Path::new(owner))))
        .collect()
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
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("60s")]
async fn managed_xfs_sandbox_filesystem_owns_allocation_limits_and_cleanup() {
    use std::collections::BTreeSet;

    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
        &FilesystemStorageMode::ManagedXfs {
            xfs_quotad_socket: crate::sandbox_filesystem::xfs::in_process_xfs_quotad(&root),
            root: root.clone().into(),
        },
        RetryConfig::default(),
    )
    .await
    .unwrap();
    assert!(
        SandboxFilesystemProvisioning::provision(
            &FilesystemStorageMode::ManagedXfs {
                xfs_quotad_socket: crate::sandbox_filesystem::xfs::in_process_xfs_quotad(&root),
                root: root.clone().into(),
            },
            RetryConfig::default()
        )
        .await
        .is_err()
    );
    assert_eq!(
        provisioning.agent_accounting(),
        AgentAccounting::ProjectQuotas
    );
    assert!(provisioning.volume().copies_on_write());
    assert!(directories.scratch.path().as_path().is_dir());
    assert!(directories.initial_files.path().as_path().is_dir());

    let sources = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("sandbox-filesystem-source"),
    )
    .await
    .unwrap();
    let source = sources
        .path()
        .child(std::ffi::OsStr::new("source"))
        .unwrap();
    std::fs::write(source.as_path(), vec![0x5a; 8192]).unwrap();
    let source_descriptor = File::open(source.as_path()).unwrap();
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
        &FilesystemStorageMode::Directory {
            root: unmanaged_parent.path().into(),
        },
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
    SandboxFilesystem::delete_and_verify(unmanaged)
        .await
        .unwrap();

    let project_id = filesystem.project_id_for_test();
    let copied = filesystem.root().join("copied");
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source: source.clone(),
            target: SandboxPath::at_root("copied"),
            access: SeedAccess::ReadWrite,
            placement: SeedPlacement::CreateNew,
        }]),
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
    assert_eq!(&std::fs::read(source.as_path()).unwrap()[..4], b"ZZZZ");
    assert_eq!(&std::fs::read(&copied).unwrap()[..4], b"COW!");

    let before_open_unlinked = settled_allocation(&filesystem).await;
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
    SandboxFilesystem::delete_and_verify(filesystem)
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
    SandboxFilesystem::delete_and_verify(second).await.unwrap();
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
    sources.discard().await.unwrap();
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

/// Whether two paths name the same directory. A host directory path on managed storage starts
/// at the descriptor of the volume root (`/proc/self/fd/<n>`), not at the mount path, so the
/// test compares the device and the inode.
fn is_same_directory(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let left = std::fs::metadata(left).unwrap();
    let right = std::fs::metadata(right).unwrap();
    left.is_dir() && (left.dev(), left.ino()) == (right.dev(), right.ino())
}

fn managed_test_root() -> PathBuf {
    std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root")
}

fn managed_test_name(filesystem: &str) -> SandboxFilesystemName {
    SandboxFilesystemName::new(
        "native-test-environment".to_string(),
        "native-test-component".to_string(),
        format!("{filesystem}-{}", std::process::id()),
    )
    .unwrap()
}

fn volume_root(provisioning: &SandboxFilesystemProvisioning) -> Arc<File> {
    Arc::clone(
        provisioning
            .volume()
            .copy_on_write_root()
            .expect("managed test provisioning must have a managed volume"),
    )
}

fn filesystem_block_bytes(root: &File) -> u64 {
    fstatfs(root).unwrap().f_bsize as u64
}

/// Reads the allocation of the project of `filesystem` after XFS frees its unlinked inodes.
///
/// XFS keeps an unlinked inode, with its blocks, charged to its project until background
/// inactivation frees it. The reading is the first one whose object count equals the inodes
/// that are reachable from the root. The readings are 25 ms apart. After 5 s the function
/// panics with the last reading and the reachable count.
async fn settled_allocation(filesystem: &SandboxFilesystem) -> FilesystemAllocation {
    use futures::StreamExt as _;

    let reachable = reachable_inodes(filesystem.root());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let last = futures::stream::unfold(Some(Duration::ZERO), move |delay| async move {
        tokio::time::sleep(delay?).await;
        let reading = filesystem.observe_allocation().await.unwrap().unwrap();
        let next = (reading.filesystem_objects != reachable
            && tokio::time::Instant::now() < deadline)
            .then_some(Duration::from_millis(25));
        Some((reading, next))
    })
    .fold(None, |_, reading| std::future::ready(Some(reading)))
    .await
    .expect("the first allocation reading always happens");
    assert_eq!(
        last.filesystem_objects, reachable,
        "project allocation did not settle within 5 s: last reading {last:?}, reachable inodes {reachable}"
    );
    last
}

/// Counts the root and every entry below it, without following symlinks.
fn reachable_inodes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .fold(1, |count, entry| {
            match entry.file_type().unwrap().is_dir() {
                true => count + reachable_inodes(&entry.path()),
                false => count + 1,
            }
        })
}

async fn available_bytes(provisioning: &SandboxFilesystemProvisioning) -> u64 {
    rustix::fs::syncfs(&*volume_root(provisioning)).unwrap();
    match crate::sandbox_filesystem::observe_space(provisioning.volume())
        .await
        .unwrap()
    {
        FilesystemSpace::Observed {
            available_bytes, ..
        } => available_bytes,
        FilesystemSpace::Unlimited => panic!("managed volume must observe space"),
    }
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_copy_contents_shares_extents_and_charges_no_agent_quota() {
    use std::os::unix::fs::PermissionsExt;

    const FILE_COUNT: usize = 48;
    const FILE_BYTES: usize = 64 * 1024;

    let root = managed_test_root();
    [".scratch", ".initial-files"].into_iter().for_each(|name| {
        let stale = root.join(name).join("stale-copy");
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("garbage"), b"stale").unwrap();
    });
    let inherited_project = NonZeroU32::new(0x7fff_0001).unwrap();
    assign_project(&File::open(&root).unwrap(), inherited_project).unwrap();
    let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
        &FilesystemStorageMode::ManagedXfs {
            xfs_quotad_socket: crate::sandbox_filesystem::xfs::in_process_xfs_quotad(&root),
            root: root.clone().into(),
        },
        RetryConfig::default(),
    )
    .await
    .unwrap();
    let HostDirectories {
        scratch: copies,
        initial_files,
    } = directories;
    let copies_root = root.join(".scratch");
    assert!(
        is_same_directory(copies.path().as_path(), &copies_root),
        "the scratch directory must be .scratch under the mount"
    );
    assert!(
        is_same_directory(initial_files.path().as_path(), &root.join(".initial-files")),
        "the initial-files directory must be .initial-files under the mount"
    );
    [copies_root.clone(), root.join(".initial-files")]
        .into_iter()
        .for_each(|host_directory| {
            assert!(
                std::fs::read_dir(&host_directory).unwrap().next().is_none(),
                "{} must hold nothing that an earlier process left",
                host_directory.display()
            );
            let attributes = get_fsxattr(&File::open(&host_directory).unwrap()).unwrap();
            assert_eq!(attributes.fsx_projid, 0);
            assert_eq!(
                attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT,
                0
            );
        });
    let root_attributes = get_fsxattr(&File::open(&root).unwrap()).unwrap();
    assert_eq!(root_attributes.fsx_projid, 0);
    assert_eq!(
        root_attributes.fsx_xflags & linux_raw_sys::general::FS_XFLAG_PROJINHERIT,
        0
    );
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-copy"))
        .await
        .unwrap();
    let project_id = filesystem.project_id_for_test();
    let agent_root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(agent_root.join("data/nested")).unwrap();
    std::fs::create_dir(agent_root.join("static")).unwrap();
    (0..FILE_COUNT).for_each(|index| {
        std::fs::write(
            agent_root.join(format!("data/file-{index}")),
            vec![index as u8; FILE_BYTES],
        )
        .unwrap();
    });
    std::fs::write(agent_root.join("data/nested/hidden"), b"excluded directory").unwrap();
    std::fs::write(agent_root.join("static/asset.bin"), b"excluded file").unwrap();
    std::fs::write(agent_root.join("config.toml"), b"[config]").unwrap();
    std::fs::set_permissions(
        agent_root.join("config.toml"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    std::os::unix::fs::symlink("data/file-0", agent_root.join("link")).unwrap();
    std::fs::hard_link(agent_root.join("link"), agent_root.join("link-name")).unwrap();
    std::fs::hard_link(
        agent_root.join("data/file-1"),
        agent_root.join("data/file-1-name"),
    )
    .unwrap();
    rustix::fs::syncfs(&*volume_root).unwrap();

    let unsynced = vec![0xa5; FILE_BYTES];
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(agent_root.join("data/file-0"))
            .unwrap();
        file.write_all(&unsynced).unwrap();
    }
    let available_before = match crate::sandbox_filesystem::observe_space(provisioning.volume())
        .await
        .unwrap()
    {
        FilesystemSpace::Observed {
            available_bytes, ..
        } => available_bytes,
        FilesystemSpace::Unlimited => panic!("managed volume must observe space"),
    };
    let allocation_before = filesystem.observe_allocation().await.unwrap().unwrap();

    let excluded = Arc::new(TreeExclusions::new([
        PathBuf::from("static/asset.bin"),
        PathBuf::from("data/nested"),
    ]));
    let copy = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        excluded,
        copy.path(),
    )
    .await
    .unwrap();
    let copy_root = copy.path().as_path().to_path_buf();

    let available_after = available_bytes(&provisioning).await;
    let copy_bytes = available_before.saturating_sub(available_after);
    let tree_bytes = (FILE_COUNT * FILE_BYTES) as u64;
    println!("COPY_METADATA_BYTES={copy_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        copy_bytes <= 32 * block_bytes,
        "the copy consumed {copy_bytes} bytes for a {tree_bytes} byte tree"
    );
    let allocation_after = filesystem.observe_allocation().await.unwrap().unwrap();
    assert_eq!(allocation_after, allocation_before);

    assert_eq!(copy_root.parent(), Some(copies.path().as_path()));
    assert_eq!(
        std::fs::read(copy_root.join("data/file-0")).unwrap(),
        unsynced
    );
    (1..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(copy_root.join(format!("data/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "data/file-{index} in the copy must have the bytes of the agent file"
        );
    });
    assert_eq!(
        groups.as_ref(),
        [
            LinkGroup {
                first: Path::new("data/file-1").into(),
                others: Box::new([Box::from(Path::new("data/file-1-name"))]),
            },
            LinkGroup {
                first: Path::new("link").into(),
                others: Box::new([Box::from(Path::new("link-name"))]),
            },
        ]
    );
    let mut expected = tree_copy::tree_listing(&agent_root);
    [
        "static/asset.bin",
        "data/nested",
        "data/nested/hidden",
        "data/file-1-name",
        "link-name",
    ]
    .into_iter()
    .for_each(|absent| {
        assert!(
            expected.remove(absent),
            "{absent} must be in the agent tree listing"
        );
    });
    assert_eq!(tree_copy::tree_listing(&copy_root), expected);
    assert_eq!(
        std::fs::metadata(copy_root.join("config.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    assert_eq!(
        std::fs::read_link(copy_root.join("link")).unwrap(),
        PathBuf::from("data/file-0")
    );
    ["data", "data/file-1", "config.toml"]
        .into_iter()
        .for_each(|relative| {
            assert_eq!(
                file_project_id(&File::open(copy_root.join(relative)).unwrap()).unwrap(),
                None,
                "{relative} in the copy must belong to no project"
            );
        });
    assert_eq!(
        file_project_id(&File::open(agent_root.join("data/file-1")).unwrap()).unwrap(),
        Some(project_id)
    );

    copy.discard().await.unwrap();
    assert!(!copy_root.exists());
    copies.discard().await.unwrap();
    assert!(!copies_root.exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    assert_eq!(
        provisioning
            .project_allocation_for_test(project_id)
            .unwrap(),
        FilesystemAllocation {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_write_after_copy_contents_consumes_one_cow_extent() {
    const FILE_BYTES: usize = 1024 * 1024;

    let root = managed_test_root();
    let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
        &FilesystemStorageMode::ManagedXfs {
            xfs_quotad_socket: crate::sandbox_filesystem::xfs::in_process_xfs_quotad(&root),
            root: root.clone().into(),
        },
        RetryConfig::default(),
    )
    .await
    .unwrap();
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);
    let cow_extent_size = cow_extent_size_hint(&volume_root, block_bytes).unwrap();

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-cow"))
        .await
        .unwrap();
    let agent_file = filesystem.root().join("db");
    std::fs::write(&agent_file, vec![0x11; FILE_BYTES]).unwrap();
    let copies = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("native-test-cow-copies"),
    )
    .await
    .unwrap();
    let copy = HostDirectory::create_in(copies.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::default()),
        copy.path(),
    )
    .await
    .unwrap();
    let available_before = available_bytes(&provisioning).await;
    let allocation_before = filesystem.observe_allocation().await.unwrap().unwrap();

    let written = vec![0x22u8; block_bytes as usize];
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&agent_file)
        .unwrap();
    file.write_all(&written).unwrap();
    file.sync_all().unwrap();

    let available_while_open = available_bytes(&provisioning).await;
    let consumed = available_before.saturating_sub(available_while_open);
    let allocation_after = filesystem.observe_allocation().await.unwrap().unwrap();
    let charged = allocation_after
        .allocated_bytes
        .saturating_sub(allocation_before.allocated_bytes);
    drop(file);
    let available_after_close = available_bytes(&provisioning).await;
    let kept_after_close = available_before.saturating_sub(available_after_close);
    println!(
        "COW_EXTENT_SIZE_BYTES={consumed} charged_to_agent={charged} kept_after_close={kept_after_close} volume_hint={cow_extent_size} block_bytes={block_bytes}"
    );
    assert_eq!(
        consumed, cow_extent_size,
        "a {block_bytes} byte write into a shared extent must consume one copy-on-write extent"
    );
    // The agent project gains at most the new extent. The overwritten block stays allocated
    // because the copy still maps it.
    assert!(
        charged > 0 && charged <= cow_extent_size,
        "the agent project must pay for the copy-on-write extent and nothing more: charged={charged}"
    );
    assert!(
        (block_bytes..=cow_extent_size).contains(&kept_after_close),
        "the changed extent must stay allocated after the file is closed"
    );
    assert_eq!(
        &std::fs::read(&agent_file).unwrap()[..block_bytes as usize],
        &written[..]
    );
    assert_eq!(
        std::fs::read(copy.path().as_path().join("db")).unwrap(),
        vec![0x11; FILE_BYTES]
    );

    copy.discard().await.unwrap();
    copies.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
}

#[test]
#[ignore = "requires the privileged managed XFS test runner"]
#[timeout("120s")]
async fn managed_xfs_seed_charges_the_project_and_follows_the_placement() {
    let root = managed_test_root();
    let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
        &FilesystemStorageMode::ManagedXfs {
            xfs_quotad_socket: crate::sandbox_filesystem::xfs::in_process_xfs_quotad(&root),
            root: root.clone().into(),
        },
        RetryConfig::default(),
    )
    .await
    .unwrap();
    let sources = HostDirectory::create_in(
        directories.scratch.path(),
        std::ffi::OsStr::new("native-test-seed-sources"),
    )
    .await
    .unwrap();
    let sources_root = sources.path().as_path().to_path_buf();
    assert_eq!(
        file_project_id(&File::open(&sources_root).unwrap()).unwrap(),
        None
    );
    let tree = sources.path().child(std::ffi::OsStr::new("tree")).unwrap();
    std::fs::create_dir_all(tree.as_path().join("data")).unwrap();
    std::fs::write(tree.as_path().join("data/large"), vec![0x31; 256 * 1024]).unwrap();
    std::fs::write(tree.as_path().join("data/small"), vec![0x32; 64 * 1024]).unwrap();
    std::os::unix::fs::symlink("data/small", tree.as_path().join("link")).unwrap();
    let replacement = sources
        .path()
        .child(std::ffi::OsStr::new("replacement"))
        .unwrap();
    std::fs::write(replacement.as_path(), vec![0x33; 128 * 1024]).unwrap();
    let entry = |source: &HostPath, target: &str, access, placement| SeedEntry {
        source: source.clone(),
        target: SandboxPath::at_root(target),
        access,
        placement,
    };

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-seed"))
        .await
        .unwrap();
    let project_id = filesystem.project_id_for_test();
    let allocation_before = settled_allocation(&filesystem).await;

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap();

    let allocation_after = settled_allocation(&filesystem).await;
    assert!(
        allocation_after.allocated_bytes >= allocation_before.allocated_bytes + 320 * 1024,
        "seed did not charge the agent project: before={allocation_before:?}, after={allocation_after:?}"
    );
    assert_eq!(
        allocation_after.filesystem_objects,
        allocation_before.filesystem_objects + 4
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/large")).unwrap(),
        vec![0x31; 256 * 1024]
    );
    assert_eq!(
        std::fs::read_link(filesystem.root().join("link")).unwrap(),
        PathBuf::from("data/small")
    );
    ["data", "data/large", "data/small"]
        .into_iter()
        .for_each(|relative| {
            assert_eq!(
                file_project_id(&File::open(filesystem.root().join(relative)).unwrap()).unwrap(),
                Some(project_id),
                "{relative} must belong to the agent project"
            );
        });
    assert!(tree.as_path().join("data/large").is_file());

    let existing = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap_err();
    assert_eq!(existing.io_kind(), Some(std::io::ErrorKind::AlreadyExists));
    assert_eq!(settled_allocation(&filesystem).await, allocation_after);

    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &replacement,
            "data/small",
            SeedAccess::ReadOnly,
            SeedPlacement::Replace,
        )]),
    )
    .await
    .unwrap();
    let allocation_replaced = settled_allocation(&filesystem).await;
    assert_eq!(
        std::fs::read(filesystem.root().join("data/small")).unwrap(),
        vec![0x33; 128 * 1024]
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/large")).unwrap(),
        vec![0x31; 256 * 1024]
    );
    assert!(
        std::fs::metadata(filesystem.root().join("data/small"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(
        file_project_id(&File::open(filesystem.root().join("data/small")).unwrap()).unwrap(),
        Some(project_id)
    );
    assert!(
        allocation_replaced.allocated_bytes >= allocation_after.allocated_bytes + 64 * 1024,
        "a replacement with more bytes must charge the agent project: before={allocation_after:?}, after={allocation_replaced:?}"
    );
    assert_eq!(
        allocation_replaced.filesystem_objects,
        allocation_after.filesystem_objects
    );

    std::fs::write(filesystem.root().join("data/agent"), vec![0x34; 32 * 1024]).unwrap();
    let allocation_before_replace = settled_allocation(&filesystem).await;
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::Replace,
        )]),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(filesystem.root().join("data/small")).unwrap(),
        vec![0x32; 64 * 1024],
        "replace must put the source file below a merged directory"
    );
    assert_eq!(
        std::fs::read(filesystem.root().join("data/agent")).unwrap(),
        vec![0x34; 32 * 1024],
        "a merge must leave a path that only the sandbox has"
    );
    assert_eq!(
        file_project_id(&File::open(filesystem.root().join("data/small")).unwrap()).unwrap(),
        Some(project_id)
    );
    let allocation_restored = settled_allocation(&filesystem).await;
    assert!(
        allocation_restored.allocated_bytes < allocation_before_replace.allocated_bytes,
        "a replacement with fewer bytes must release project bytes: before={allocation_before_replace:?}, after={allocation_restored:?}"
    );
    assert_eq!(
        allocation_restored.filesystem_objects,
        allocation_before_replace.filesystem_objects
    );

    let limited = provisioning
        .create_fresh(managed_test_name("native-test-seed-limited"))
        .await
        .unwrap();
    let limited_project_id = limited.project_id_for_test();
    limited
        .install_limits(FilesystemLimits {
            allocated_bytes: 128 * 1024,
            filesystem_objects: 64,
        })
        .await
        .unwrap();
    let exhausted = <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &limited,
        Box::new([entry(
            &tree,
            "",
            SeedAccess::FromSource,
            SeedPlacement::CreateNew,
        )]),
    )
    .await
    .unwrap_err();
    assert!(
        exhausted.is_storage_exhaustion(),
        "seed into a small quota must fail with a quota error: {exhausted}"
    );

    sources.discard().await.unwrap();
    assert!(!sources_root.exists());
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
    SandboxFilesystem::delete_and_verify(limited).await.unwrap();
    [project_id, limited_project_id]
        .into_iter()
        .for_each(|project| {
            assert_eq!(
                provisioning.project_allocation_for_test(project).unwrap(),
                FilesystemAllocation {
                    allocated_bytes: 0,
                    filesystem_objects: 0,
                },
                "project {project} must have no allocation after cleanup"
            );
        });
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
    match SandboxFilesystemProvisioning::new(storage, RetryConfig::default()) {
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

impl Drop for RestoredProjectAttributes {
    fn drop(&mut self) {
        if let Err(error) = set_fsxattr(&self.directory, self.attributes) {
            eprintln!("failed to restore the project attributes of the test root: {error}");
        }
    }
}

#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("120s")]
async fn reflink_xfs_copy_contents_and_seed_share_extents() {
    const FILE_COUNT: usize = 48;
    const FILE_BYTES: usize = 64 * 1024;

    let root = reflink_test_root();
    let (provisioning, directories) = SandboxFilesystemProvisioning::provision(
        &reflink_storage(root.clone()),
        RetryConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        provisioning.agent_accounting(),
        AgentAccounting::Unaccounted
    );
    assert!(provisioning.volume().copies_on_write());
    [root.join(".scratch"), root.join(".initial-files")]
        .into_iter()
        .for_each(|host_directory| {
            let attributes = get_fsxattr(&File::open(&host_directory).unwrap()).unwrap();
            assert!(
                !has_project_identity(&attributes),
                "{} must have no project identity",
                host_directory.display()
            );
        });
    let volume_root = volume_root(&provisioning);
    let block_bytes = filesystem_block_bytes(&volume_root);

    let filesystem = provisioning
        .create_fresh(managed_test_name("native-test-reflink-copy"))
        .await
        .unwrap();
    let agent_root = filesystem.root().to_path_buf();
    std::fs::create_dir_all(agent_root.join("data")).unwrap();
    (0..FILE_COUNT).for_each(|index| {
        std::fs::write(
            agent_root.join(format!("data/file-{index}")),
            vec![index as u8; FILE_BYTES],
        )
        .unwrap();
    });
    std::fs::hard_link(
        agent_root.join("data/file-1"),
        agent_root.join("data/file-1-name"),
    )
    .unwrap();
    rustix::fs::syncfs(&*volume_root).unwrap();
    let tree_bytes = (FILE_COUNT * FILE_BYTES) as u64;

    let available_before_copy = available_bytes(&provisioning).await;
    let copy = HostDirectory::create_in(directories.scratch.path(), std::ffi::OsStr::new("copy"))
        .await
        .unwrap();
    let groups = <SandboxFilesystem as SandboxFilesystemAdapter>::copy_contents(
        &filesystem,
        SandboxPath::at_root(""),
        Arc::new(TreeExclusions::new(std::iter::empty::<PathBuf>())),
        copy.path(),
    )
    .await
    .unwrap();
    let copy_bytes = available_before_copy.saturating_sub(available_bytes(&provisioning).await);
    println!("REFLINK_COPY_BYTES={copy_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        copy_bytes <= 32 * block_bytes,
        "the copy consumed {copy_bytes} bytes for a {tree_bytes} byte tree"
    );
    assert_eq!(groups.len(), 1);
    (0..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(copy.path().as_path().join(format!("data/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "data/file-{index} in the copy must have the bytes of the agent file"
        );
    });

    let source = copy.path().child(std::ffi::OsStr::new("data")).unwrap();
    let available_before_seed = available_bytes(&provisioning).await;
    <SandboxFilesystem as SandboxFilesystemAdapter>::seed(
        &filesystem,
        Box::new([SeedEntry {
            source,
            target: SandboxPath::at_root("seeded"),
            access: SeedAccess::FromSource,
            placement: SeedPlacement::CreateNew,
        }]),
    )
    .await
    .unwrap();
    let seed_bytes = available_before_seed.saturating_sub(available_bytes(&provisioning).await);
    println!("REFLINK_SEED_BYTES={seed_bytes} tree_bytes={tree_bytes} block_bytes={block_bytes}");
    assert!(
        seed_bytes <= 32 * block_bytes,
        "the seed consumed {seed_bytes} bytes for a {tree_bytes} byte tree"
    );
    (0..FILE_COUNT).for_each(|index| {
        assert_eq!(
            std::fs::read(agent_root.join(format!("seeded/file-{index}"))).unwrap(),
            vec![index as u8; FILE_BYTES],
            "seeded/file-{index} must have the bytes of the copy"
        );
    });

    copy.discard().await.unwrap();
    SandboxFilesystem::delete_and_verify(filesystem)
        .await
        .unwrap();
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

    let error = match SandboxFilesystemProvisioning::provision(
        &reflink_storage(root.clone()),
        RetryConfig::default(),
    )
    .await
    {
        Ok(_) => panic!("a root with a project identity must be refused"),
        Err(error) => error.to_string(),
    };

    assert!(
        error.contains("verify XFS root has no project identity"),
        "{error}"
    );
}

/// A host directory that inherits a project identity from the root after the binding fails
/// the setup of the host directories, because reflink XFS storage checks that each host
/// directory has no project identity.
#[test]
#[ignore = "requires the unprivileged reflink XFS test runner"]
#[timeout("60s")]
async fn reflink_xfs_refuses_a_host_directory_with_a_project_identity() {
    let root = reflink_test_root();
    let provisioning =
        SandboxFilesystemProvisioning::new(&reflink_storage(root.clone()), RetryConfig::default())
            .unwrap();
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

    let error =
        match crate::sandbox_filesystem::host_directory::make_host_directories(&provisioning).await
        {
            Ok(_) => panic!("a host directory with a project identity must be refused"),
            Err(error) => error.to_string(),
        };

    assert!(
        error.contains("verify XFS host directory has no project identity"),
        "{error}"
    );
}
