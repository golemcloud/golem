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

//! The file mode creation mask of the process, which keeps the owner write bit for every file that
//! an agent creates.
//!
//! The module uses only the standard library and `libc`, so a test binary of its own can include
//! it and change the mask of its process.

/// The owner write bit of a file mode creation mask.
const OWNER_WRITE_BIT: libc::mode_t = 0o200;

/// The mask that the fallback sets to read the mask of the process: every group and other bit,
/// and no owner bit.
///
/// A file that another thread creates while this mask is set gets permissions for its owner only.
/// So the probe never gives a group or other permission that the mask of the process takes away,
/// and it never gives a file mode 000, as the mask 0o777 does.
pub(super) const PROBE_MASK: libc::mode_t = 0o077;

/// Clears bit 0o200 of the file mode creation mask of the process, and keeps the other bits.
///
/// Each file that an agent creates then has write permission for its owner. The initial-file rule
/// counts a file without write permission at a read-only declared path as the initial file when
/// its content equals the declaration, so a file of an agent must always have this permission. The
/// call is idempotent, and it changes only the owner write bit of the mask.
///
/// On Linux the function reads the current mask from the `Umask:` line of
/// `/proc/thread-self/status`. That read does not change the mask. On other Unix platforms, and on
/// Linux when the line is not available, the function sets the mask two times: to
/// [`PROBE_MASK`], which gives the current mask, and then to that mask without the owner write
/// bit.
pub(crate) fn keep_owner_write_permission() {
    keep_owner_write_permission_with(current_file_creation_mask)
}

/// Sets the file mode creation mask of the process to the mask that `current` gives, without the
/// owner write bit.
pub(super) fn keep_owner_write_permission_with(current: impl FnOnce() -> libc::mode_t) {
    let mask = mask_without_owner_write(current());
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    unsafe {
        libc::umask(mask);
    }
}

/// Gives the current file mode creation mask of the calling thread. Where
/// `/proc/thread-self/status` has no `Umask:` line, the mask is [`PROBE_MASK`] after the call.
///
/// The function reads `/proc/thread-self/status`, not `/proc/self/status`. `/proc/self` is the
/// thread group leader, and a thread that does not share the filesystem attributes of the process
/// has its own mask. All threads of the executor share one mask, so there the read gives the mask
/// of the process.
#[cfg(target_os = "linux")]
fn current_file_creation_mask() -> libc::mode_t {
    std::fs::read_to_string("/proc/thread-self/status")
        .ok()
        .as_deref()
        .and_then(status_umask)
        .unwrap_or_else(probed_file_creation_mask)
}

/// Gives the current file mode creation mask of the process. The mask is [`PROBE_MASK`] after the
/// call.
#[cfg(not(target_os = "linux"))]
fn current_file_creation_mask() -> libc::mode_t {
    probed_file_creation_mask()
}

/// Sets the file mode creation mask of the process to [`PROBE_MASK`], and gives the mask before
/// the call.
pub(super) fn probed_file_creation_mask() -> libc::mode_t {
    // SAFETY: `umask` only replaces the mask of the process. It cannot fail.
    unsafe { libc::umask(PROBE_MASK) }
}

/// Gives `mask` without the owner write bit. Every other bit stays as it is.
pub(super) fn mask_without_owner_write(mask: libc::mode_t) -> libc::mode_t {
    mask & !OWNER_WRITE_BIT
}

/// Gives the file mode creation mask on the `Umask:` line of a `/proc/<pid>/status` text.
#[cfg(target_os = "linux")]
pub(super) fn status_umask(status: &str) -> Option<libc::mode_t> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Umask:"))
        .and_then(|value| libc::mode_t::from_str_radix(value.trim(), 8).ok())
}
