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
#[cfg(target_os = "linux")]
use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
#[cfg(target_os = "linux")]
use crate::services::golem_config::FilesystemStorageMode;
use std::collections::{HashMap, VecDeque};
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use test_r::{test, timeout};
use tokio::sync::Notify;

struct ScriptedRecovery {
    observations: Mutex<VecDeque<Result<FilesystemSpace, &'static str>>>,
    candidates: Mutex<Option<Vec<u8>>>,
    unloads: Mutex<HashMap<u8, ScriptedUnload>>,
    events: Mutex<Vec<String>>,
    active_unloads: AtomicUsize,
    maximum_active_unloads: AtomicUsize,
}

enum ScriptedUnload {
    Outcome(EvictionStopOutcome),
    Wait(Arc<Notify>),
}

impl ScriptedRecovery {
    fn new(
        observations: impl IntoIterator<Item = FilesystemSpace>,
        candidates: Vec<u8>,
        unloads: impl IntoIterator<Item = (u8, EvictionStopOutcome)>,
    ) -> Self {
        Self {
            observations: Mutex::new(observations.into_iter().map(Ok).collect()),
            candidates: Mutex::new(Some(candidates)),
            unloads: Mutex::new(
                unloads
                    .into_iter()
                    .map(|(candidate, outcome)| (candidate, ScriptedUnload::Outcome(outcome)))
                    .collect(),
            ),
            events: Mutex::new(Vec::new()),
            active_unloads: AtomicUsize::new(0),
            maximum_active_unloads: AtomicUsize::new(0),
        }
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    fn candidate_lookups(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.as_str() == "candidates")
            .count()
    }

    fn unload_order(&self) -> Vec<u8> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| event.strip_prefix("unload:")?.parse().ok())
            .collect()
    }
}

#[async_trait]
impl CapacityRecoverySource for ScriptedRecovery {
    type Candidate = u8;

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError> {
        self.events.lock().unwrap().push("observe".to_string());
        match self.observations.lock().unwrap().pop_front().unwrap() {
            Ok(observation) => Ok(observation),
            Err(message) => Err(FilesystemPressureError::Observation(
                StorageVolumeError::verification(
                    message,
                    std::path::Path::new("<scripted-volume>"),
                ),
            )),
        }
    }

    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate> {
        self.events.lock().unwrap().push("candidates".to_string());
        self.candidates.lock().unwrap().take().unwrap()
    }

    async fn request_unload(
        &self,
        candidate: Self::Candidate,
        _deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError> {
        self.events
            .lock()
            .unwrap()
            .push(format!("unload:{candidate}"));
        let active = self.active_unloads.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_active_unloads
            .fetch_max(active, Ordering::AcqRel);
        let unload = self.unloads.lock().unwrap().remove(&candidate).unwrap();
        let outcome = match unload {
            ScriptedUnload::Outcome(outcome) => outcome,
            ScriptedUnload::Wait(notify) => {
                notify.notified().await;
                EvictionStopOutcome::Unloaded
            }
        };
        self.active_unloads.fetch_sub(1, Ordering::AcqRel);
        Ok(outcome)
    }
}

fn space(available_bytes: u64, available_filesystem_objects: u64) -> FilesystemSpace {
    FilesystemSpace::Observed {
        total_bytes: 100,
        available_bytes,
        total_filesystem_objects: 100,
        available_filesystem_objects,
    }
}

fn bytes_target(available_bytes: u64) -> FilesystemCapacityTarget {
    FilesystemCapacityTarget::available_bytes(available_bytes, 2, Duration::ZERO)
}

fn object_target(available_filesystem_objects: u64) -> FilesystemCapacityTarget {
    FilesystemCapacityTarget::available_filesystem_objects(
        available_filesystem_objects,
        2,
        Duration::ZERO,
    )
}

fn write_policy() -> FilesystemWritePressurePolicy {
    FilesystemWritePressurePolicy::from_config(
        &FilesystemPressureConfig::new(5, 10, 5, 10, 2, Duration::ZERO).unwrap(),
    )
}

#[test]
fn write_pressure_policy_uses_validated_config_values() {
    let config = FilesystemPressureConfig::new(5, 10, 7, 12, 3, Duration::from_millis(4)).unwrap();

    let policy = FilesystemWritePressurePolicy::from_config(&config);

    assert_eq!(policy.minimum_available_bytes, 5);
    assert_eq!(policy.minimum_available_filesystem_objects, 7);
    assert_eq!(policy.target.available_bytes, Some(10));
    assert_eq!(policy.target.available_filesystem_objects, Some(12));
    assert_eq!(policy.target.reclamation_observation_attempts, 3);
    assert_eq!(
        policy.target.reclamation_observation_delay,
        Duration::from_millis(4)
    );
    assert!(!policy.physical_pressure(space(6, 8)));
    assert!(policy.physical_pressure(space(5, 8)));
    assert!(policy.physical_pressure(space(6, 7)));
}

#[test]
fn pressure_unload_request_preserves_reason_and_deadline() {
    let deadline = Instant::now() + Duration::from_secs(7);

    let request = filesystem_pressure_unload_request(deadline);

    assert_eq!(request.reason, UnloadReason::FilesystemPressure);
    assert_eq!(request.deadline, deadline);
}

#[test]
fn pressure_candidates_are_oldest_first_with_stable_ties() {
    let mut candidates = vec![
        (2, "a".to_string(), "new"),
        (1, "z".to_string(), "old-z"),
        (1, "a".to_string(), "old-a"),
    ];

    sort_filesystem_pressure_candidates(&mut candidates);

    assert_eq!(
        candidates
            .into_iter()
            .map(|(_, _, candidate)| candidate)
            .collect::<Vec<_>>(),
        vec!["old-a", "old-z", "new"]
    );
}

struct BlockedInitialObservation {
    candidate_lookups: AtomicUsize,
    unload_requests: AtomicUsize,
}

#[async_trait]
impl CapacityRecoverySource for BlockedInitialObservation {
    type Candidate = ();

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError> {
        std::future::pending().await
    }

    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate> {
        self.candidate_lookups.fetch_add(1, Ordering::AcqRel);
        vec![()]
    }

    async fn request_unload(
        &self,
        (): Self::Candidate,
        _deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError> {
        self.unload_requests.fetch_add(1, Ordering::AcqRel);
        Ok(EvictionStopOutcome::Unloaded)
    }
}

#[test]
#[timeout("1s")]
async fn blocked_initial_observation_returns_deadline_without_recovery_actions() {
    let source = BlockedInitialObservation {
        candidate_lookups: AtomicUsize::new(0),
        unload_requests: AtomicUsize::new(0),
    };

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        Instant::now() + Duration::from_millis(10),
    )
    .await;

    assert!(matches!(result, Err(FilesystemPressureError::Deadline)));
    assert_eq!(source.candidate_lookups.load(Ordering::Acquire), 0);
    assert_eq!(source.unload_requests.load(Ordering::Acquire), 0);
}

#[test]
async fn unlimited_returns_target_reached_without_candidate_lookup_or_retry() {
    let source = ScriptedRecovery::new([FilesystemSpace::Unlimited], vec![1], []);

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(u64::MAX),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert_eq!(result.final_observation, FilesystemSpace::Unlimited);
    assert_eq!(result.unloaded_agents, 0);
    assert!(result.target_reached);
    assert!(!result.permits_pressure_retry());
    assert_eq!(source.candidate_lookups(), 0);
    assert!(source.unload_order().is_empty());
    assert!(source.observations.lock().unwrap().is_empty());
}

#[test]
async fn already_satisfied_observation_returns_without_candidate_lookup() {
    let source = ScriptedRecovery::new([space(20, 20)], vec![1], []);

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(20),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert_eq!(result.unloaded_agents, 0);
    assert!(result.target_reached);
    assert!(result.permits_pressure_retry());
    assert_eq!(source.candidate_lookups(), 0);
}

#[test]
async fn write_recovery_requires_fresh_physical_pressure_evidence() {
    let source = ScriptedRecovery::new([space(6, 100)], vec![1], []);

    let outcome = recover_write_from(
        &source,
        &tokio::sync::Mutex::new(()),
        write_policy(),
        ScriptedRecovery::deadline(),
    )
    .await;

    assert_eq!(outcome, FilesystemWriteRecoveryOutcome::NotUnderPressure);
    assert_eq!(source.candidate_lookups(), 0);
    assert!(source.unload_order().is_empty());
}

#[test]
async fn write_recovery_delegates_proven_pressure_to_serialized_reclamation() {
    let source = ScriptedRecovery::new(
        [space(5, 100), space(5, 100), space(5, 100), space(10, 100)],
        vec![1],
        [(1, EvictionStopOutcome::Unloaded)],
    );

    let outcome = recover_write_from(
        &source,
        &tokio::sync::Mutex::new(()),
        write_policy(),
        ScriptedRecovery::deadline(),
    )
    .await;

    assert_eq!(outcome, FilesystemWriteRecoveryOutcome::Recovered);
    assert_eq!(source.candidate_lookups(), 1);
    assert_eq!(source.unload_order(), vec![1]);
    assert!(source.observations.lock().unwrap().is_empty());
}

#[test]
async fn byte_target_counts_only_verified_unloads_and_reobserves_after_each() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100), space(1, 100), space(10, 100)],
        vec![1],
        [(1, EvictionStopOutcome::Unloaded)],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert_eq!(result.unloaded_agents, 1);
    assert!(result.target_reached);
    assert_eq!(source.unload_order(), vec![1]);
    assert!(source.observations.lock().unwrap().is_empty());
}

#[test]
async fn filesystem_object_target_uses_object_observations() {
    let source = ScriptedRecovery::new(
        [space(100, 1), space(100, 1), space(100, 7)],
        vec![1],
        [(1, EvictionStopOutcome::Unloaded)],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        object_target(7),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert!(result.target_reached);
    assert_eq!(result.unloaded_agents, 1);
}

#[test]
async fn combined_target_requires_both_dimensions() {
    let source = ScriptedRecovery::new(
        [
            space(1, 1),
            space(1, 1),
            space(10, 1),
            space(10, 2),
            space(10, 10),
        ],
        vec![1, 2],
        [
            (1, EvictionStopOutcome::Unloaded),
            (2, EvictionStopOutcome::Unloaded),
        ],
    );
    let target = FilesystemCapacityTarget::combined(10, 10, 2, Duration::ZERO);

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        target,
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert!(result.target_reached);
    assert_eq!(result.unloaded_agents, 2);
    assert_eq!(source.unload_order(), vec![1, 2]);
}

#[test]
async fn ineligible_victim_is_skipped_without_counting_or_reobservation() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100), space(10, 100)],
        vec![1, 2],
        [
            (1, EvictionStopOutcome::Ineligible),
            (2, EvictionStopOutcome::Unloaded),
        ],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert_eq!(result.unloaded_agents, 1);
    assert_eq!(source.unload_order(), vec![1, 2]);
    assert!(source.observations.lock().unwrap().is_empty());
}

#[test]
async fn delayed_reclamation_is_observed_before_selecting_another_victim() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100), space(5, 100), space(10, 100)],
        vec![1, 2],
        [
            (1, EvictionStopOutcome::Unloaded),
            (2, EvictionStopOutcome::Unloaded),
        ],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert!(result.target_reached);
    assert_eq!(result.unloaded_agents, 1);
    assert_eq!(source.unload_order(), vec![1]);
}

#[test]
async fn cleanup_failure_returns_no_reclamation_result() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100), space(100, 100)],
        vec![1, 2],
        [
            (1, EvictionStopOutcome::CleanupFailed),
            (2, EvictionStopOutcome::Unloaded),
        ],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await;

    assert!(matches!(
        result,
        Err(FilesystemPressureError::CleanupFailed)
    ));
    assert_eq!(source.unload_order(), vec![1]);
    assert_eq!(source.observations.lock().unwrap().len(), 1);
}

#[test]
async fn post_unload_observation_failure_returns_no_reclamation_result() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100)],
        vec![1],
        [(1, EvictionStopOutcome::Unloaded)],
    );
    source
        .observations
        .lock()
        .unwrap()
        .push_back(Err("post-deletion observation failed"));

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await;

    assert!(matches!(
        result,
        Err(FilesystemPressureError::Observation(_))
    ));
    assert_eq!(source.unload_order(), vec![1]);
}

#[test]
async fn candidate_exhaustion_returns_latest_observation_without_retry() {
    let source = ScriptedRecovery::new(
        [space(1, 100), space(1, 100), space(2, 100), space(3, 100)],
        vec![1],
        [(1, EvictionStopOutcome::Unloaded)],
    );

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        ScriptedRecovery::deadline(),
    )
    .await
    .unwrap();

    assert_eq!(result.final_observation, space(3, 100));
    assert_eq!(result.unloaded_agents, 1);
    assert!(!result.target_reached);
    assert!(!result.permits_pressure_retry());
}

#[test]
async fn deadline_before_recovery_does_not_lookup_or_unload_candidates() {
    let source = ScriptedRecovery::new([space(1, 100)], vec![1], []);

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        Instant::now(),
    )
    .await;

    assert!(matches!(result, Err(FilesystemPressureError::Deadline)));
    assert_eq!(source.candidate_lookups(), 0);
    assert!(source.unload_order().is_empty());
}

#[test]
#[timeout("1s")]
async fn deadline_during_unload_returns_no_reclamation_result() {
    let notify = Arc::new(Notify::new());
    let source = ScriptedRecovery::new([space(1, 100), space(1, 100)], vec![1], []);
    source
        .unloads
        .lock()
        .unwrap()
        .insert(1, ScriptedUnload::Wait(notify));

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        bytes_target(10),
        Instant::now() + Duration::from_millis(10),
    )
    .await;

    assert!(matches!(result, Err(FilesystemPressureError::Deadline)));
    assert!(source.observations.lock().unwrap().is_empty());
}

struct SharedCapacityRecovery {
    available_bytes: Arc<AtomicU64>,
    candidate: u8,
    first_unload: Arc<Notify>,
    candidate_lookups: AtomicUsize,
    active_unloads: AtomicUsize,
    maximum_active_unloads: AtomicUsize,
}

#[async_trait]
impl CapacityRecoverySource for SharedCapacityRecovery {
    type Candidate = u8;

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError> {
        Ok(space(self.available_bytes.load(Ordering::Acquire), 100))
    }

    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate> {
        self.candidate_lookups.fetch_add(1, Ordering::AcqRel);
        vec![self.candidate]
    }

    async fn request_unload(
        &self,
        candidate: Self::Candidate,
        _deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError> {
        let active = self.active_unloads.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_active_unloads
            .fetch_max(active, Ordering::AcqRel);
        if candidate == 1 {
            self.first_unload.notified().await;
            self.available_bytes.store(10, Ordering::Release);
        }
        self.active_unloads.fetch_sub(1, Ordering::AcqRel);
        Ok(EvictionStopOutcome::Unloaded)
    }
}

#[test]
async fn completed_concurrent_recovery_is_reobserved_before_candidate_lookup() {
    let recovery = Arc::new(tokio::sync::Mutex::new(()));
    let first_unload = Arc::new(Notify::new());
    let available_bytes = Arc::new(AtomicU64::new(1));
    let first = Arc::new(SharedCapacityRecovery {
        available_bytes: Arc::clone(&available_bytes),
        candidate: 1,
        first_unload: Arc::clone(&first_unload),
        candidate_lookups: AtomicUsize::new(0),
        active_unloads: AtomicUsize::new(0),
        maximum_active_unloads: AtomicUsize::new(0),
    });
    let second = Arc::new(SharedCapacityRecovery {
        available_bytes,
        candidate: 2,
        first_unload: Arc::clone(&first_unload),
        candidate_lookups: AtomicUsize::new(0),
        active_unloads: AtomicUsize::new(0),
        maximum_active_unloads: AtomicUsize::new(0),
    });

    let first_task = tokio::spawn({
        let recovery = Arc::clone(&recovery);
        let source = Arc::clone(&first);
        async move {
            reclaim_filesystem_capacity_from(
                source.as_ref(),
                recovery.as_ref(),
                bytes_target(10),
                ScriptedRecovery::deadline(),
            )
            .await
        }
    });
    while first.active_unloads.load(Ordering::Acquire) == 0 {
        tokio::task::yield_now().await;
    }
    let second_task = tokio::spawn({
        let recovery = Arc::clone(&recovery);
        let source = Arc::clone(&second);
        async move {
            reclaim_filesystem_capacity_from(
                source.as_ref(),
                recovery.as_ref(),
                bytes_target(10),
                ScriptedRecovery::deadline(),
            )
            .await
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(first.maximum_active_unloads.load(Ordering::Acquire), 1);
    assert_eq!(second.candidate_lookups.load(Ordering::Acquire), 0);
    assert_eq!(second.active_unloads.load(Ordering::Acquire), 0);
    first_unload.notify_one();

    assert!(first_task.await.unwrap().unwrap().target_reached);
    assert!(second_task.await.unwrap().unwrap().target_reached);
    assert_eq!(second.candidate_lookups.load(Ordering::Acquire), 0);
    assert_eq!(second.maximum_active_unloads.load(Ordering::Acquire), 0);
}

#[test]
fn stricter_targets_never_turn_an_unsatisfied_observation_into_satisfied() {
    for available_bytes in 0..=20 {
        for lower_target in 0..=20 {
            for higher_target in lower_target..=20 {
                let observation = space(available_bytes, 100);
                if bytes_target(higher_target).reached(observation) {
                    assert!(bytes_target(lower_target).reached(observation));
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
struct RealVolumeRecovery {
    volume: FilesystemVolume,
    filler: Mutex<Option<PathBuf>>,
}

#[async_trait]
#[cfg(target_os = "linux")]
impl CapacityRecoverySource for RealVolumeRecovery {
    type Candidate = ();

    async fn observe_space(&self) -> Result<FilesystemSpace, FilesystemPressureError> {
        observe_space(&self.volume)
            .await
            .map_err(FilesystemPressureError::Observation)
    }

    async fn eligible_loaded_idle_victims(&self) -> Vec<Self::Candidate> {
        vec![()]
    }

    async fn request_unload(
        &self,
        (): Self::Candidate,
        _deadline: Instant,
    ) -> Result<EvictionStopOutcome, FilesystemPressureError> {
        let filler = self.filler.lock().unwrap().take().unwrap();
        tokio::fs::remove_file(filler).await.unwrap();
        Ok(EvictionStopOutcome::Unloaded)
    }
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires GOLEM_MANAGED_XFS_TEST_ROOT on a privileged XFS project-quota mount"]
async fn managed_xfs_reobserves_fresh_space_after_verified_deletion() {
    let root = PathBuf::from(std::env::var("GOLEM_MANAGED_XFS_TEST_ROOT").unwrap());
    let provisioning = SandboxFilesystemProvisioning::new(
        &FilesystemStorageMode::ManagedXfs {
            xfs_quotad_socket: crate::storage_volume::in_process_xfs_quotad(&root),
            root: root.clone().into(),
        },
        golem_common::model::RetryConfig::default(),
    )
    .unwrap();
    let before = observe_space(provisioning.volume()).await.unwrap();
    let filler = root.join(format!("pressure-filler-{}", uuid::Uuid::new_v4()));
    let mut file = std::fs::File::create(&filler).unwrap();
    file.write_all(&vec![0xa5; 16 * 1024 * 1024]).unwrap();
    file.sync_all().unwrap();
    let constrained = observe_space(provisioning.volume()).await.unwrap();
    drop(file);
    let (
        FilesystemSpace::Observed {
            available_bytes: before_bytes,
            ..
        },
        FilesystemSpace::Observed {
            available_bytes: constrained_bytes,
            ..
        },
    ) = (before, constrained)
    else {
        panic!("managed XFS must report observed space");
    };
    assert!(constrained_bytes < before_bytes);
    let target_bytes = constrained_bytes + (before_bytes - constrained_bytes).max(1) / 2;
    let source = RealVolumeRecovery {
        volume: provisioning.volume().clone(),
        filler: Mutex::new(Some(filler)),
    };

    let result = reclaim_filesystem_capacity_from(
        &source,
        &tokio::sync::Mutex::new(()),
        FilesystemCapacityTarget::available_bytes(target_bytes, 10, Duration::from_millis(25)),
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert!(result.target_reached);
    assert_eq!(result.unloaded_agents, 1);
    assert!(result.permits_pressure_retry());
}
