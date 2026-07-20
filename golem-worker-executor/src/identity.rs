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

//! Process/pod identity for this worker-executor instance.
//!
//! The identity is derived from the `POD_NAME` env var, falling back to
//! `HOSTNAME`, then `"unknown"`, resolved once and cached for the lifetime of
//! the process. It is used both as the `executor_id` metric label and anywhere
//! else the running instance needs to identify itself.

/// Returns the stable identity of this worker-executor instance.
///
/// Resolved once on first call and cached for the lifetime of the process.
pub fn executor_id() -> &'static str {
    static EXECUTOR_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    EXECUTOR_ID.get_or_init(|| {
        std::env::var("POD_NAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "unknown".to_string())
    })
}
