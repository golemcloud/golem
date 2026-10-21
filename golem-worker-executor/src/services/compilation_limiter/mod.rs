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

use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Bounds the number of concurrent local component compile fallback attempts.
///
/// Worker admission accounts for guest linear memory and a shared
/// per-component-revision charge derived from registry metadata. It does not
/// reserve the transient working set used when the worker-executor in-process
/// component cache and compiled artifact store both miss. That fallback buffers
/// the raw component bytes, compiles them with Wasmtime, serializes the compiled
/// artifact, and writes it to the compiled artifact store.
///
/// This limiter caps how many fallback attempts run at the same time; callers
/// beyond the limit wait for a permit.
/// A limit of `0` is treated as unlimited: the gate is bypassed entirely.
#[derive(Clone)]
pub struct CompilationLimiter {
    semaphore: Option<Arc<Semaphore>>,
}

impl CompilationLimiter {
    pub fn new(max_concurrent: usize) -> Self {
        let semaphore = if max_concurrent == 0 {
            None
        } else {
            Some(Arc::new(Semaphore::new(max_concurrent)))
        };
        Self { semaphore }
    }

    /// Runs `f` while holding a compilation permit, waiting for one if the
    /// limiter is at its concurrency limit. With an unlimited limiter the
    /// future runs immediately with no gating.
    pub async fn run<F, T>(&self, f: F) -> T
    where
        F: Future<Output = T>,
    {
        let _permit = match &self.semaphore {
            Some(semaphore) => Some(
                semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("compilation limiter semaphore closed"),
            ),
            None => None,
        };
        f.await
    }
}

#[cfg(test)]
mod tests;
