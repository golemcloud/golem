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

//! `xfs-quotad`: the only Golem process that calls `quotactl_fd`.
//!
//! Linux allows the project quota calls of `ManagedXfs` storage only with `CAP_SYS_ADMIN`. The
//! worker executor runs guest Wasm, so it must not hold that capability. It sends each quota call
//! to `xfs-quotad` over a Unix socket instead ([`XfsQuotadClient`]). `xfs-quotad` serves one XFS
//! root, runs no guest code, and accepts three calls: read the quota state of the volume, read the
//! quota of a project, and set the limits of a project.
//!
//! The operator runs `xfs-quotad` beside the executor, for example as a sidecar container or a
//! second service, gives it `CAP_SYS_ADMIN`, and configures its socket in `ManagedXfs` mode. Its
//! image gives the binary the file capability `cap_sys_admin+ep`, so it can run as a user other
//! than root. `xfs-quotad` does not change its capabilities; it refuses to start without
//! `CAP_SYS_ADMIN`.
//!
//! `xfs-quotad` accepts a connection only from a process of one uid, its own unless `--allow-uid`
//! names another. On connect, it sends a greeting with the protocol version and the device of its
//! root, and the client refuses a server of another version or of another filesystem.

#[cfg(test)]
test_r::enable!();

#[cfg(target_os = "linux")]
mod quotad;

#[cfg(target_os = "linux")]
pub use quotad::*;
