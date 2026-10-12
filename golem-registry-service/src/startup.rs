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

use std::fmt::{Display, Formatter};
use std::sync::{Arc, Mutex};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinArtifactKind {
    Plugins,
    Tools,
}

impl Display for BuiltinArtifactKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BuiltinArtifactKind::Plugins => write!(f, "plugins"),
            BuiltinArtifactKind::Tools => write!(f, "tools"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupPhase {
    Initializing,
    FetchingBuiltinArtifacts {
        kind: BuiltinArtifactKind,
        completed: usize,
        total: usize,
        downloaded_bytes: u64,
    },
    InspectingBuiltinArtifacts {
        kind: BuiltinArtifactKind,
        completed: usize,
        total: usize,
    },
    ProvisioningBuiltinArtifacts {
        kind: BuiltinArtifactKind,
        completed: usize,
        total: usize,
    },
    StartingServices,
}

impl Display for StartupPhase {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupPhase::Initializing => write!(f, "initializing"),
            StartupPhase::FetchingBuiltinArtifacts {
                kind,
                completed,
                total,
                downloaded_bytes,
            } => {
                write!(f, "fetching built-in {kind} ({completed}/{total} ready")?;
                if *downloaded_bytes > 0 {
                    write!(
                        f,
                        ", {:.1} MB downloaded",
                        *downloaded_bytes as f64 / 1_000_000.0
                    )?;
                }
                write!(f, ")")
            }
            StartupPhase::InspectingBuiltinArtifacts {
                kind,
                completed,
                total,
            } => write!(f, "inspecting built-in {kind} ({completed}/{total})"),
            StartupPhase::ProvisioningBuiltinArtifacts {
                kind,
                completed,
                total,
            } => write!(f, "provisioning built-in {kind} ({completed}/{total})"),
            StartupPhase::StartingServices => write!(f, "starting services"),
        }
    }
}

/// Shared, observable progress of a server that is still starting up.
///
/// Phase changes are logged; progress within a phase (completed steps, downloaded bytes) is only
/// visible through [`StartupProgress::current`].
#[derive(Debug, Clone)]
pub struct StartupProgress {
    phase: Arc<Mutex<StartupPhase>>,
}

impl Default for StartupProgress {
    fn default() -> Self {
        Self {
            phase: Arc::new(Mutex::new(StartupPhase::Initializing)),
        }
    }
}

impl StartupProgress {
    pub fn current(&self) -> StartupPhase {
        self.phase.lock().unwrap().clone()
    }

    pub fn set_phase(&self, phase: StartupPhase) {
        let mut current = self.phase.lock().unwrap();
        if *current != phase {
            info!(phase = %phase, "Server startup progress");
        }
        *current = phase;
    }

    /// Counts one completed step of the current phase, if the phase counts steps.
    pub fn step_completed(&self) {
        match &mut *self.phase.lock().unwrap() {
            StartupPhase::FetchingBuiltinArtifacts { completed, .. }
            | StartupPhase::InspectingBuiltinArtifacts { completed, .. }
            | StartupPhase::ProvisioningBuiltinArtifacts { completed, .. } => *completed += 1,
            StartupPhase::Initializing | StartupPhase::StartingServices => {}
        }
    }

    pub fn builtin_artifact_bytes_downloaded(&self, bytes: u64) {
        if let StartupPhase::FetchingBuiltinArtifacts {
            downloaded_bytes, ..
        } = &mut *self.phase.lock().unwrap()
        {
            *downloaded_bytes += bytes;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn progress_is_tracked_within_a_phase() {
        let progress = StartupProgress::default();
        progress.step_completed();
        assert_eq!(progress.current(), StartupPhase::Initializing);
        assert_eq!(progress.current().to_string(), "initializing");

        progress.set_phase(StartupPhase::FetchingBuiltinArtifacts {
            kind: BuiltinArtifactKind::Tools,
            completed: 0,
            total: 3,
            downloaded_bytes: 0,
        });
        assert_eq!(
            progress.current().to_string(),
            "fetching built-in tools (0/3 ready)"
        );
        progress.builtin_artifact_bytes_downloaded(1_500_000);
        progress.builtin_artifact_bytes_downloaded(800_000);
        progress.step_completed();
        assert_eq!(
            progress.current().to_string(),
            "fetching built-in tools (1/3 ready, 2.3 MB downloaded)"
        );

        progress.set_phase(StartupPhase::InspectingBuiltinArtifacts {
            kind: BuiltinArtifactKind::Tools,
            completed: 0,
            total: 14,
        });
        progress.builtin_artifact_bytes_downloaded(10);
        progress.step_completed();
        progress.step_completed();
        assert_eq!(
            progress.current().to_string(),
            "inspecting built-in tools (2/14)"
        );

        progress.set_phase(StartupPhase::ProvisioningBuiltinArtifacts {
            kind: BuiltinArtifactKind::Plugins,
            completed: 0,
            total: 1,
        });
        progress.step_completed();
        assert_eq!(
            progress.current().to_string(),
            "provisioning built-in plugins (1/1)"
        );

        progress.set_phase(StartupPhase::StartingServices);
        progress.step_completed();
        assert_eq!(progress.current(), StartupPhase::StartingServices);
    }
}
