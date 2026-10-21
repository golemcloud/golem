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
use cap_fs_ext::MetadataExt as _;

#[cfg(windows)]
fn windows_directory_is_case_sensitive(directory: &cap_std::fs::Dir) -> std::io::Result<bool> {
    use std::mem::{MaybeUninit, size_of};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_CASE_SENSITIVE_INFO, FileCaseSensitiveInfo, GetFileInformationByHandleEx,
    };

    let mut information = MaybeUninit::<FILE_CASE_SENSITIVE_INFO>::zeroed();
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle() as _,
            FileCaseSensitiveInfo,
            information.as_mut_ptr().cast(),
            size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 || code == ERROR_NOT_SUPPORTED as i32
        ) {
            return Ok(false);
        }
        return Err(error);
    }
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;
    Ok(unsafe { information.assume_init() }.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

#[cfg(windows)]
fn windows_names_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

    let left = left.encode_wide().collect::<Vec<_>>();
    let right = right.encode_wide().collect::<Vec<_>>();
    unsafe {
        CompareStringOrdinal(
            left.as_ptr(),
            left.len() as i32,
            right.as_ptr(),
            right.len() as i32,
            1,
        ) == CSTR_EQUAL
    }
}

#[cfg(all(test, not(windows)))]
fn windows_names_equal(left: &std::ffi::OsStr, right: &std::ffi::OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

impl PartialEq for NativeNameCoordinationKey {
    fn eq(&self, other: &Self) -> bool {
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, NativeNameComparisonMode::Conservative) => {
                true
            }
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            _ => false,
        }
    }
}

impl Eq for NativeNameCoordinationKey {}

impl Hash for NativeNameCoordinationKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mode.hash(state);
        if self.mode == NativeNameComparisonMode::Exact {
            self.name.hash(state);
        }
    }
}

impl NativeNameCoordinationKey {
    fn may_be_equivalent(&self, other: &Self) -> bool {
        match (self.mode, other.mode) {
            (NativeNameComparisonMode::Exact, NativeNameComparisonMode::Exact) => {
                self.name == other.name
            }
            (NativeNameComparisonMode::Conservative, _)
            | (_, NativeNameComparisonMode::Conservative) => true,
            #[cfg(any(test, windows))]
            (
                NativeNameComparisonMode::WindowsInsensitive,
                NativeNameComparisonMode::WindowsInsensitive,
            ) => windows_names_equal(&self.name, &other.name),
            #[cfg(any(test, windows))]
            _ => true,
        }
    }
}

impl SandboxNamespaceCoordinationKey {
    /// Reports whether two resolved names may denote the same entry in one parent directory.
    ///
    /// Conservative name modes return conflicts rather than risk concurrent aliasing edits.
    pub(crate) fn may_conflict_with(&self, other: &Self) -> bool {
        self.parent == other.parent && self.name.may_be_equivalent(&other.name)
    }

    #[cfg(test)]
    pub(crate) fn scripted_exact(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Exact)
    }

    #[cfg(test)]
    pub(crate) fn scripted_conservative(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(parent, name.into(), NativeNameComparisonMode::Conservative)
    }

    #[cfg(test)]
    pub(super) fn scripted_windows_insensitive(parent: u64, name: impl Into<OsString>) -> Self {
        Self::scripted(
            parent,
            name.into(),
            NativeNameComparisonMode::WindowsInsensitive,
        )
    }

    #[cfg(test)]
    fn scripted(parent: u64, name: OsString, mode: NativeNameComparisonMode) -> Self {
        Self {
            parent: SandboxDirectoryCoordinationKey(NativeFileIdentity::Scripted(format!(
                "comparison-parent-{parent}"
            ))),
            name: NativeNameCoordinationKey { name, mode },
        }
    }
}

impl PartialEq<SandboxDirectoryCoordinationKey> for SandboxNamespaceCoordinationKey {
    fn eq(&self, other: &SandboxDirectoryCoordinationKey) -> bool {
        self.parent == *other
    }
}

impl PartialEq<SandboxNamespaceCoordinationKey> for SandboxDirectoryCoordinationKey {
    fn eq(&self, other: &SandboxNamespaceCoordinationKey) -> bool {
        other == self
    }
}

impl SandboxResolvedNamespaceTarget {
    /// Reports a positive proof that lookup in this parent requires the exact stored spelling.
    /// False means no such proof. This does not establish any ancestor's spelling.
    pub(crate) fn has_byte_exact_lookup_proof(&self) -> bool {
        self.parent_byte_exact_lookup_proven
    }

    /// Returns the semantic key used to coordinate access to this namespace entry.
    pub(crate) fn coordination_key(&self) -> SandboxNamespaceCoordinationKey {
        self.coordination_key.clone()
    }

    /// Returns the identity of the final directory when the resolved entry is a directory.
    pub(crate) fn final_directory_key(&self) -> Option<SandboxDirectoryCoordinationKey> {
        self.final_directory_key.clone()
    }

    /// Rebuilds a path relative to the parent capability pinned during resolution.
    ///
    /// Namespace operations should use this instead of resolving the original ambient path again.
    pub(crate) fn target(&self) -> SandboxPath {
        SandboxPath::at(self.parent.clone(), PathBuf::from(self.name.as_ref()))
    }

    /// Tells whether the resolved object is a regular file without write permission.
    ///
    /// `follow` selects the final symlink or its referent. A missing object is not a read-only
    /// file. A failure to read the referent gives that failure as a storage error.
    pub(crate) fn is_read_only_file(
        &self,
        follow: SandboxFollow,
    ) -> Result<bool, FilesystemStorageError> {
        match (follow, &self.followed_read_only_file) {
            (SandboxFollow::No, _) => Ok(self.read_only_file),
            (SandboxFollow::Yes, NativeReadOnlyResolution::Resolved(read_only)) => Ok(*read_only),
            (
                SandboxFollow::Yes,
                NativeReadOnlyResolution::Failed {
                    kind,
                    raw_os_error,
                    message,
                },
            ) => {
                let source = raw_os_error.map_or_else(
                    || std::io::Error::new(*kind, message.clone()),
                    std::io::Error::from_raw_os_error,
                );
                Err(FilesystemStorageError::io(
                    "resolve sandbox filesystem target permissions",
                    &self.parent.path.join(self.name.as_ref()),
                    source,
                ))
            }
        }
    }
}

pub(super) fn resolve_host_namespace_target(
    base: Arc<cap_std::fs::Dir>,
    target: SandboxPath,
    parent_path: PathBuf,
    name_mode_source: NativeNameModeSource,
    name_mode_probe: NativeNameModeProbe,
) -> std::io::Result<SandboxResolvedNamespaceTarget> {
    let (relative_parent, name) = split_namespace_target(&target.path)?;
    let parent = if relative_parent.as_os_str().is_empty() {
        base
    } else {
        Arc::new(base.open_dir(relative_parent)?)
    };
    let parent_key = native_directory_coordination_key(&parent)?;
    let name_mode =
        native_name_comparison_mode(&parent, &parent_key, name_mode_source, &name_mode_probe)?;
    let coordination_key = SandboxNamespaceCoordinationKey {
        parent: parent_key,
        name: NativeNameCoordinationKey {
            name: name.clone(),
            mode: name_mode,
        },
    };
    let (final_directory_key, read_only_file, followed_read_only_file) =
        match parent.symlink_metadata(&name) {
            Ok(metadata) => {
                let directory = (metadata.is_dir() && !metadata.file_type().is_symlink())
                    .then(|| native_file_identity(&metadata).map(SandboxDirectoryCoordinationKey))
                    .transpose()?;
                let read_only_file = is_read_only_file(&metadata);
                let followed = if metadata.file_type().is_symlink() {
                    match parent.metadata(&name) {
                        Ok(metadata) => {
                            NativeReadOnlyResolution::Resolved(is_read_only_file(&metadata))
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            NativeReadOnlyResolution::Resolved(false)
                        }
                        Err(error) => NativeReadOnlyResolution::Failed {
                            kind: error.kind(),
                            raw_os_error: error.raw_os_error(),
                            message: error.to_string(),
                        },
                    }
                } else {
                    NativeReadOnlyResolution::Resolved(read_only_file)
                };
                (directory, read_only_file, followed)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (None, false, NativeReadOnlyResolution::Resolved(false))
            }
            Err(error) => return Err(error),
        };
    Ok(SandboxResolvedNamespaceTarget {
        parent: SandboxDirectory {
            handle: NativeDirectoryHandle::Host(parent),
            path: parent_path,
            coordination_key: coordination_key.parent.clone(),
        },
        name: name.into_boxed_os_str(),
        parent_byte_exact_lookup_proven: coordination_key.name.mode
            == NativeNameComparisonMode::Exact,
        coordination_key,
        final_directory_key,
        read_only_file,
        followed_read_only_file,
    })
}

/// Tells whether `metadata` describes a regular file without write permission.
pub(super) fn is_read_only_file(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_file() && metadata.permissions().readonly()
}

pub(super) fn split_namespace_target(path: &Path) -> std::io::Result<(PathBuf, OsString)> {
    if path.as_os_str().is_empty() {
        return Ok((PathBuf::new(), OsString::from(".")));
    }
    let component = path.components().next_back().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "namespace target has no final component",
        )
    })?;
    let name = match component {
        std::path::Component::Normal(name) => name.to_os_string(),
        std::path::Component::CurDir => OsString::from("."),
        std::path::Component::ParentDir => OsString::from(".."),
        std::path::Component::Prefix(_) | std::path::Component::RootDir => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "namespace target must be relative to its directory capability",
            ));
        }
    };
    Ok((
        path.parent().unwrap_or_else(|| Path::new("")).to_path_buf(),
        name,
    ))
}

pub(super) fn native_directory_coordination_key(
    directory: &cap_std::fs::Dir,
) -> std::io::Result<SandboxDirectoryCoordinationKey> {
    let metadata = directory.dir_metadata()?;
    Ok(SandboxDirectoryCoordinationKey(native_file_identity(
        &metadata,
    )?))
}

pub fn native_file_identity(
    metadata: &cap_std::fs::Metadata,
) -> std::io::Result<NativeFileIdentity> {
    #[cfg(unix)]
    return Ok(NativeFileIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    });
    #[cfg(windows)]
    return Ok(NativeFileIdentity::Windows {
        volume_serial_number: u32::try_from(metadata.dev()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "native volume serial number does not fit its opaque identity",
            )
        })?,
        file_index: metadata.ino(),
    });
    #[cfg(not(any(unix, windows)))]
    return Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "native file identity is unavailable on this target",
    ));
}

fn native_name_comparison_mode(
    directory: &cap_std::fs::Dir,
    parent: &SandboxDirectoryCoordinationKey,
    source: NativeNameModeSource,
    probe: &NativeNameModeProbe,
) -> std::io::Result<NativeNameComparisonMode> {
    #[cfg(target_os = "linux")]
    {
        Ok(linux_name_comparison_mode(
            source,
            parent,
            xfs_name_mode_shortcut_enabled(),
            || {
                probe.record();
                rustix::fs::ioctl_getflags(directory)
                    .ok()
                    .map(|flags| flags.bits())
            },
        ))
    }
    #[cfg(windows)]
    {
        let _ = (parent, source, probe);
        return Ok(if windows_directory_is_case_sensitive(directory)? {
            NativeNameComparisonMode::Exact
        } else {
            NativeNameComparisonMode::WindowsInsensitive
        });
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        // Case sensitivity does not prove byte-exact lookup. APFS also aliases Unicode spellings.
        let _ = (directory, parent, source, probe);
        Ok(NativeNameComparisonMode::Conservative)
    }
}

#[cfg(target_os = "linux")]
fn xfs_name_mode_shortcut_enabled() -> bool {
    let enabled = XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED;
    #[cfg(test)]
    let enabled = enabled && !xfs_name_mode_shortcut_disabled_for_test();
    enabled
}

#[cfg(all(test, target_os = "linux"))]
fn xfs_name_mode_shortcut_disabled_for_test() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_DISABLE_XFS_NAME_MODE_SHORTCUT").as_deref() == Ok("1")
    })
}

#[cfg(target_os = "linux")]
pub(super) fn linux_name_comparison_mode(
    source: NativeNameModeSource,
    parent: &SandboxDirectoryCoordinationKey,
    shortcut_enabled: bool,
    detect_flags: impl FnOnce() -> Option<u32>,
) -> NativeNameComparisonMode {
    const FS_CASEFOLD_FL: u32 = 0x4000_0000;
    let parent_device = match &parent.0 {
        NativeFileIdentity::Unix { device, .. } => *device,
        #[allow(unreachable_patterns)]
        _ => return NativeNameComparisonMode::Conservative,
    };
    if shortcut_enabled
        && matches!(source, NativeNameModeSource::ValidatedXfs(proof) if proof.matches_device(parent_device))
    {
        return NativeNameComparisonMode::Exact;
    }
    match detect_flags() {
        Some(flags) if flags & FS_CASEFOLD_FL == 0 => NativeNameComparisonMode::Exact,
        Some(_) | None => NativeNameComparisonMode::Conservative,
    }
}
