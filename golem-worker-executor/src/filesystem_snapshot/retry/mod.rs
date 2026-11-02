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

//! Retry-delay calculation shared by the store driver and its test limiters.

use golem_common::model::RetryConfig;
use golem_common::retries::get_delay;
use std::time::Duration;

/// The wait after `failed_runs` failures, or `None` when the script allows no next run. The
/// drawn jitter grows the wait up to `max_delay`; negative, invalid and excessive factors do
/// not panic.
pub(crate) fn run_delay(retry: &RetryConfig, failed_runs: u32, jitter: f64) -> Option<Duration> {
    let without_jitter = RetryConfig {
        max_jitter_factor: None,
        ..retry.clone()
    };
    let base = get_delay(&without_jitter, failed_runs)?;
    let grown = base.as_secs_f64() * (1.0 + jitter.max(0.0));
    Some(
        Duration::try_from_secs_f64(grown)
            .unwrap_or(retry.max_delay)
            .min(retry.max_delay),
    )
}

#[cfg(test)]
mod tests;
