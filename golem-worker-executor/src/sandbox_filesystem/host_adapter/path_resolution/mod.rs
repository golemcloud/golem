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

use super::super::name_comparison::{
    NativeNameComparisonMode, NativeNameCoordinationKey, SandboxNameComparisonKey,
};
use super::super::object_identity::{
    NativeFileIdentity, SandboxDirectoryId, directory_identity, native_file_identity,
};
use super::super::open_handles::{NativeDirectoryHandle, SandboxDirectory, SandboxFilePermissions};
use super::super::sandbox_paths::{
    ResolvedPathEntry, SandboxPath, SandboxResolvedPathTarget, split_path_target,
};
use super::{NativeNameModeProbe, NativeNameModeSource, is_read_only_file};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(target_os = "linux")]
pub(in crate::sandbox_filesystem) const XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED: bool = true;

#[cfg(windows)]
pub(in crate::sandbox_filesystem) fn windows_directory_is_case_sensitive(
    directory: &cap_std::fs::Dir,
) -> std::io::Result<bool> {
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

pub(in crate::sandbox_filesystem) fn resolve_host_path_target(
    base: Arc<cap_std::fs::Dir>,
    target: SandboxPath,
    parent_path: PathBuf,
    name_mode_source: NativeNameModeSource,
    name_mode_probe: NativeNameModeProbe,
) -> std::io::Result<SandboxResolvedPathTarget> {
    let (relative_parent, name) = split_path_target(&target.path)?;
    let parent = if relative_parent.as_os_str().is_empty() {
        base
    } else {
        Arc::new(base.open_dir(relative_parent)?)
    };
    let parent_key = directory_identity(&parent)?;
    let name_mode =
        native_name_comparison_mode(&parent, &parent_key, name_mode_source, &name_mode_probe)?;
    let name_comparison_key = SandboxNameComparisonKey {
        parent: parent_key,
        name: NativeNameCoordinationKey {
            name: name.clone(),
            mode: name_mode,
        },
    };
    let entry = match parent.symlink_metadata(&name) {
        Ok(metadata) if metadata.is_symlink() => {
            let referent_read_only = match parent.metadata(&name) {
                Ok(metadata) => Ok(is_read_only_file(&metadata)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(Arc::new(error)),
            };
            ResolvedPathEntry::Symlink { referent_read_only }
        }
        Ok(metadata) if metadata.is_dir() => {
            ResolvedPathEntry::Directory(SandboxDirectoryId(native_file_identity(&metadata)?))
        }
        Ok(metadata) if metadata.is_file() => {
            ResolvedPathEntry::File(SandboxFilePermissions::from_metadata(&metadata))
        }
        Ok(_) => ResolvedPathEntry::Other,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ResolvedPathEntry::Missing,
        Err(error) => return Err(error),
    };
    Ok(SandboxResolvedPathTarget {
        parent: SandboxDirectory {
            handle: NativeDirectoryHandle::Host(parent),
            path: parent_path,
            directory_id: name_comparison_key.parent.clone(),
        },
        name: name.into_boxed_os_str(),
        parent_byte_exact_lookup_proven: name_comparison_key.name.mode
            == NativeNameComparisonMode::Exact,
        name_comparison_key,
        entry,
    })
}

pub(in crate::sandbox_filesystem) fn native_name_comparison_mode(
    directory: &cap_std::fs::Dir,
    parent: &SandboxDirectoryId,
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
pub(in crate::sandbox_filesystem) fn xfs_name_mode_shortcut_enabled() -> bool {
    let enabled = XFS_NAME_MODE_SHORTCUT_DEFAULT_ENABLED;
    #[cfg(test)]
    let enabled = enabled && !xfs_name_mode_shortcut_disabled_for_test();
    enabled
}

#[cfg(all(test, target_os = "linux"))]
pub(in crate::sandbox_filesystem) fn xfs_name_mode_shortcut_disabled_for_test() -> bool {
    static DISABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DISABLED.get_or_init(|| {
        std::env::var("GOLEM_FILESYSTEM_DISABLE_XFS_NAME_MODE_SHORTCUT").as_deref() == Ok("1")
    })
}

#[cfg(target_os = "linux")]
pub(in crate::sandbox_filesystem) fn linux_name_comparison_mode(
    source: NativeNameModeSource,
    parent: &SandboxDirectoryId,
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

#[cfg(test)]
mod tests;
