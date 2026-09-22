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

//! Records the filesystem of the benchmark volume, without privileges.
//!
//! The record holds the filesystem type, the mount, the project quota flags of the mount, the
//! answer of `quotactl_fd` with `Q_XGETQSTATV`, and the XFS geometry that `xfs_info` shows. The
//! verdict uses only the filesystem type and the mount options.

use serde_json::{Value, json};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

const XFS_SUPER_MAGIC: u64 = 0x5846_5342;

/// The quota type and the command of `quotactl_fd`, as the executor uses them to check the
/// project quota state at its start.
const XQM_PRJQUOTA: u32 = 2;
const Q_XGETQSTATV: u32 = (b'X' as u32) << 8 | 8;
const FS_QSTATV_VERSION1: i8 = 1;
const FS_QUOTA_PDQ_ACCT: u16 = 1 << 4;
const FS_QUOTA_PDQ_ENFD: u16 = 1 << 5;

/// The size of `struct xfs_fsop_geom` of `XFS_IOC_FSGEOMETRY`, and of `struct xfs_fsop_geom_v4`
/// of `XFS_IOC_FSGEOMETRY_V4`.
const GEOMETRY_BYTES: usize = 256;
const GEOMETRY_V4_BYTES: usize = 112;

/// The names of the bits of `xfs_fsop_geom.flags`, `XFS_FSOP_GEOM_FLAGS_*`.
const GEOMETRY_FLAGS: [(u32, &str); 26] = [
    (1 << 0, "attr"),
    (1 << 1, "nlink"),
    (1 << 2, "quota"),
    (1 << 3, "ialign"),
    (1 << 4, "dalign"),
    (1 << 5, "shared"),
    (1 << 6, "extflg"),
    (1 << 7, "dirv2"),
    (1 << 8, "logv2"),
    (1 << 9, "sector"),
    (1 << 10, "attr2"),
    (1 << 11, "projid32"),
    (1 << 12, "dirv2ci"),
    (1 << 14, "lazysb"),
    (1 << 15, "v5sb"),
    (1 << 16, "ftype"),
    (1 << 17, "finobt"),
    (1 << 18, "spinodes"),
    (1 << 19, "rmapbt"),
    (1 << 20, "reflink"),
    (1 << 21, "bigtime"),
    (1 << 22, "inobtcnt"),
    (1 << 23, "nrext64"),
    (1 << 24, "exchange_range"),
    (1 << 25, "parent"),
    (1 << 26, "metadir"),
];

/// One line of `/proc/self/mountinfo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mount {
    pub(super) mount_point: Box<Path>,
    pub(super) filesystem_type: Box<str>,
    pub(super) source: Box<str>,
    pub(super) mount_options: Box<str>,
    pub(super) super_options: Box<str>,
}

/// The project quota flags of a mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProjectQuota {
    pub(super) accounting: bool,
    pub(super) enforcement: bool,
}

/// Records the volume that holds `path`.
pub(super) fn check(path: &Path) -> Value {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let magic = rustix::fs::statfs(&path).map(|statfs| statfs.f_type as u64);
    let mount = std::fs::read_to_string("/proc/self/mountinfo")
        .ok()
        .and_then(|text| mount_of(&text, &path));
    let quota = mount
        .as_ref()
        .map(|mount| project_quota(&mount.super_options));
    let directory = File::open(&path);
    json!({
        "path": path.display().to_string(),
        "filesystem_type": magic.as_ref().map(|magic| filesystem_name(*magic)).unwrap_or("unknown"),
        "filesystem_magic": magic.as_ref().map(|magic| format!("{magic:#x}")).unwrap_or_else(|error| error.to_string()),
        "mount": mount.as_ref().map(|mount| json!({
            "mount_point": mount.mount_point.display().to_string(),
            "filesystem_type": mount.filesystem_type,
            "source": mount.source,
            "mount_options": mount.mount_options,
            "super_options": mount.super_options,
        })),
        "project_quota": quota.map(|quota| json!({
            "accounting": quota.accounting,
            "enforcement": quota.enforcement,
            "source": "mount options",
        })),
        "quotactl": match &directory {
            Ok(directory) => quota_state(directory),
            Err(error) => json!({ "error": error.to_string() }),
        },
        "xfs_geometry": match &directory {
            Ok(directory) => geometry(directory),
            Err(error) => json!({ "error": error.to_string() }),
        },
        "check": verdict(magic.ok(), quota),
    })
}

fn filesystem_name(magic: u64) -> &'static str {
    match magic {
        XFS_SUPER_MAGIC => "xfs",
        0xef53 => "ext4",
        0x0102_1994 => "tmpfs",
        0x9123_683e => "btrfs",
        0x794c_7630 => "overlay",
        _ => "other",
    }
}

/// Gives the mount whose mount point is the longest prefix of `path`, from the text of
/// `/proc/self/mountinfo`.
pub(super) fn mount_of(mountinfo: &str, path: &Path) -> Option<Mount> {
    mountinfo
        .lines()
        .filter_map(parse_mount)
        .filter(|mount| path.starts_with(&mount.mount_point))
        .max_by_key(|mount| mount.mount_point.components().count())
}

fn parse_mount(line: &str) -> Option<Mount> {
    let (mount, filesystem) = line.split_once(" - ")?;
    let mount = mount.split(' ').collect::<Vec<_>>();
    let filesystem = filesystem.split(' ').collect::<Vec<_>>();
    Some(Mount {
        mount_point: PathBuf::from(unescape(mount.get(4)?)).into_boxed_path(),
        mount_options: (*mount.get(5)?).into(),
        filesystem_type: (*filesystem.first()?).into(),
        source: (*filesystem.get(1)?).into(),
        super_options: (*filesystem.get(2)?).into(),
    })
}

/// Replaces the octal escapes of mountinfo, such as `\040` for a space.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let (decoded, _) = bytes.iter().enumerate().fold(
        (Vec::with_capacity(bytes.len()), 0_usize),
        |(mut decoded, skip), (index, byte)| {
            if skip > 0 {
                return (decoded, skip - 1);
            }
            let escape = bytes
                .get(index + 1..index + 4)
                .filter(|digits| {
                    *byte == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
                })
                .and_then(|digits| std::str::from_utf8(digits).ok())
                .and_then(|digits| u8::from_str_radix(digits, 8).ok());
            match escape {
                Some(value) => {
                    decoded.push(value);
                    (decoded, 3)
                }
                None => {
                    decoded.push(*byte);
                    (decoded, 0)
                }
            }
        },
    );
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Gives the project quota flags from the super options of an XFS mount. The kernel shows
/// `prjquota` when it counts and enforces project quotas, and `pqnoenforce` when it only counts
/// them.
pub(super) fn project_quota(super_options: &str) -> ProjectQuota {
    super_options.split(',').fold(
        ProjectQuota {
            accounting: false,
            enforcement: false,
        },
        |quota, option| match option {
            "prjquota" | "pquota" => ProjectQuota {
                accounting: true,
                enforcement: true,
            },
            "pqnoenforce" => ProjectQuota {
                accounting: true,
                ..quota
            },
            _ => quota,
        },
    )
}

/// Tells whether the volume is XFS with project quota accounting and enforcement, and why not.
pub(super) fn verdict(magic: Option<u64>, quota: Option<ProjectQuota>) -> Value {
    let reasons = [
        (magic != Some(XFS_SUPER_MAGIC)).then(|| {
            format!(
                "the filesystem type is {}, not xfs",
                magic.map_or("unknown", filesystem_name)
            )
        }),
        match quota {
            None => Some("the mount of the volume is not in /proc/self/mountinfo".to_string()),
            Some(ProjectQuota {
                accounting: false, ..
            }) => Some("the mount has no project quota".to_string()),
            Some(ProjectQuota {
                enforcement: false, ..
            }) => Some("the mount counts project quotas but does not enforce them".to_string()),
            Some(_) => None,
        },
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if reasons.is_empty() {
        json!({ "status": "ok" })
    } else {
        json!({ "status": "mismatch", "reasons": reasons })
    }
}

/// The `fs_quota_statv` record of `Q_XGETQSTATV`, as in `linux/dqblk_xfs.h`.
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

/// Asks the kernel for the project quota state of the filesystem of the directory.
fn quota_state(directory: &File) -> Value {
    let mut state = FsQuotaStatV {
        qs_version: FS_QSTATV_VERSION1,
        ..FsQuotaStatV::default()
    };
    // SAFETY: `Q_XGETQSTATV` writes one `fs_quota_statv` record, which `state` holds for the
    // duration of the call.
    let result = unsafe {
        libc::syscall(
            libc::SYS_quotactl_fd,
            directory.as_raw_fd(),
            (Q_XGETQSTATV << 8) | XQM_PRJQUOTA,
            0,
            std::ptr::from_mut(&mut state).cast::<libc::c_void>(),
        )
    };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        json!({ "error": { "errno": error.raw_os_error(), "message": error.to_string() } })
    } else {
        json!({ "ok": {
            "flags": format!("{:#x}", state.qs_flags),
            "accounting": state.qs_flags & FS_QUOTA_PDQ_ACCT != 0,
            "enforcement": state.qs_flags & FS_QUOTA_PDQ_ENFD != 0,
        } })
    }
}

/// Asks XFS for the geometry of the filesystem of the directory.
fn geometry(directory: &File) -> Value {
    // SAFETY: The opcodes and the sizes come from `struct xfs_fsop_geom` and
    // `struct xfs_fsop_geom_v4` of `xfs_fs.h`, and the kernel writes at most that many bytes.
    let full = unsafe {
        rustix::ioctl::ioctl(
            directory,
            rustix::ioctl::Getter::<
                { rustix::ioctl::opcode::read::<[u8; GEOMETRY_BYTES]>(b'X', 126) },
                [u8; GEOMETRY_BYTES],
            >::new(),
        )
    };
    let result = full.map(|bytes| bytes.to_vec()).or_else(|error| {
        if error == rustix::io::Errno::NOTTY {
            // SAFETY: As above, for the older record.
            unsafe {
                rustix::ioctl::ioctl(
                    directory,
                    rustix::ioctl::Getter::<
                        { rustix::ioctl::opcode::read::<[u8; GEOMETRY_V4_BYTES]>(b'X', 124) },
                        [u8; GEOMETRY_V4_BYTES],
                    >::new(),
                )
            }
            .map(|bytes| bytes.to_vec())
        } else {
            Err(error)
        }
    });
    match result {
        Ok(bytes) => decode_geometry(&bytes)
            .unwrap_or_else(|| json!({ "error": "the geometry record is too short" })),
        Err(error) => {
            json!({ "error": { "errno": error.raw_os_error(), "message": error.to_string() } })
        }
    }
}

/// Reads the fields of `struct xfs_fsop_geom` that `xfs_info` shows.
pub(super) fn decode_geometry(bytes: &[u8]) -> Option<Value> {
    let u32_at = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .and_then(|field| field.try_into().ok())
            .map(u32::from_ne_bytes)
    };
    let u64_at = |offset: usize| {
        bytes
            .get(offset..offset + 8)
            .and_then(|field| field.try_into().ok())
            .map(u64::from_ne_bytes)
    };
    let flags = u32_at(92)?;
    Some(json!({
        "blocksize": u32_at(0)?,
        "rtextsize": u32_at(4)?,
        "agblocks": u32_at(8)?,
        "agcount": u32_at(12)?,
        "logblocks": u32_at(16)?,
        "sectsize": u32_at(20)?,
        "inodesize": u32_at(24)?,
        "imaxpct": u32_at(28)?,
        "datablocks": u64_at(32)?,
        "rtblocks": u64_at(40)?,
        "rtextents": u64_at(48)?,
        "logstart": u64_at(56)?,
        "sunit": u32_at(80)?,
        "swidth": u32_at(84)?,
        "version": u32_at(88)?,
        "flags": format!("{flags:#x}"),
        "flag_names": GEOMETRY_FLAGS
            .iter()
            .filter(|(bit, _)| flags & bit != 0)
            .map(|(_, name)| *name)
            .collect::<Vec<_>>(),
        "logsectsize": u32_at(96)?,
        "rtsectsize": u32_at(100)?,
        "dirblocksize": u32_at(104)?,
        "logsunit": u32_at(108)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        ProjectQuota, XFS_SUPER_MAGIC, check, decode_geometry, mount_of, project_quota, verdict,
    };
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::path::Path;
    use test_r::test;

    const MOUNTINFO: &str = "\
22 1 259:1 / / rw,relatime shared:1 - overlay overlay rw,lowerdir=/a,upperdir=/b
30 22 259:2 / /data rw,relatime shared:2 - xfs /dev/nvme1n1 rw,attr2,inode64,logbufs=8,logbsize=32k,prjquota
31 22 259:3 / /data2 rw,relatime - xfs /dev/nvme2n1 rw,attr2,inode64,pqnoenforce
32 22 259:4 / /with\\040space rw - ext4 /dev/sda1 rw
33 30 0:5 / /data/proc rw - proc proc rw
";

    #[test]
    fn the_mount_of_a_path_is_the_one_with_the_longest_mount_point() {
        let mount_point = |path: &str| {
            mount_of(MOUNTINFO, Path::new(path)).map(|mount| {
                (
                    mount.mount_point.display().to_string(),
                    mount.filesystem_type.to_string(),
                )
            })
        };

        assert_eq!(
            (
                mount_point("/data/tree"),
                mount_point("/data"),
                mount_point("/data2/x"),
                mount_point("/database"),
                mount_point("/with space/x"),
                mount_point("/data/proc/1"),
            ),
            (
                Some(("/data".to_string(), "xfs".to_string())),
                Some(("/data".to_string(), "xfs".to_string())),
                Some(("/data2".to_string(), "xfs".to_string())),
                Some(("/".to_string(), "overlay".to_string())),
                Some(("/with space".to_string(), "ext4".to_string())),
                Some(("/data/proc".to_string(), "proc".to_string())),
            )
        );
    }

    #[test]
    fn the_super_options_give_the_project_quota_flags() {
        assert_eq!(
            [
                "rw,attr2,inode64,prjquota",
                "rw,attr2,pqnoenforce",
                "rw,attr2,inode64",
                "rw,usrquota",
            ]
            .map(project_quota),
            [
                ProjectQuota {
                    accounting: true,
                    enforcement: true
                },
                ProjectQuota {
                    accounting: true,
                    enforcement: false
                },
                ProjectQuota {
                    accounting: false,
                    enforcement: false
                },
                ProjectQuota {
                    accounting: false,
                    enforcement: false
                },
            ]
        );
    }

    #[test]
    fn the_verdict_accepts_only_xfs_that_counts_and_enforces_project_quotas() {
        let enforced = ProjectQuota {
            accounting: true,
            enforcement: true,
        };
        let counted = ProjectQuota {
            accounting: true,
            enforcement: false,
        };

        assert_eq!(
            [
                verdict(Some(XFS_SUPER_MAGIC), Some(enforced)),
                verdict(Some(XFS_SUPER_MAGIC), Some(counted)),
                verdict(Some(0xef53), Some(enforced)),
                verdict(None, None),
            ]
            .map(|verdict| verdict["status"].as_str().map(str::to_string)),
            [
                Some("ok".to_string()),
                Some("mismatch".to_string()),
                Some("mismatch".to_string()),
                Some("mismatch".to_string()),
            ]
        );
    }

    #[test]
    fn the_geometry_gives_the_fields_that_xfs_info_shows() {
        let bytes = [
            (0, 4096_u64, 4),
            (12, 16, 4),
            (32, 1_000_000, 8),
            (80, 8, 4),
            (92, (1 << 20) | (1 << 15) | (1 << 21), 4),
            (104, 4096, 4),
        ]
        .iter()
        .fold(vec![0_u8; 256], |mut bytes, (offset, value, size)| {
            bytes[*offset..offset + size].copy_from_slice(&value.to_ne_bytes()[..*size]);
            bytes
        });

        let geometry = decode_geometry(&bytes).unwrap();

        assert_eq!(
            (
                &geometry["blocksize"],
                &geometry["agcount"],
                &geometry["datablocks"],
                &geometry["sunit"],
                &geometry["flag_names"],
                &geometry["dirblocksize"],
                decode_geometry(&bytes[..100]).is_none(),
            ),
            (
                &json!(4096),
                &json!(16),
                &json!(1_000_000),
                &json!(8),
                &json!(["v5sb", "reflink", "bigtime"]),
                &json!(4096),
                true,
            )
        );
    }

    #[test]
    fn a_check_of_a_directory_records_each_part() {
        let directory = tempfile::tempdir().unwrap();

        let record = check(directory.path());

        assert_eq!(
            [
                "filesystem_type",
                "mount",
                "project_quota",
                "quotactl",
                "xfs_geometry",
                "check"
            ]
            .map(|key| record.get(key).is_some()),
            [true; 6]
        );
    }
}
