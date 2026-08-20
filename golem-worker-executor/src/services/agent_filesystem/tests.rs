use super::*;
use crate::services::active_workers::MemoryGrant;
use crate::services::agent_resource_billing::{AgentResourceBilling, FilesystemUsageObserver};
use crate::services::agent_storage_meter::FilesystemUsageObservation;
use crate::services::linear_memory::LinearMemoryTracker;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::{AgentFilePath, AgentFilePermissions, ComponentId};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentId, OwnedAgentId};
use golem_common::widen_infallible;
use golem_service_base::replayable_stream::ReplayableStream as _;
use golem_service_base::service::initial_agent_files::InitialAgentFilesService;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use test_r::test;

struct CountingUsageObserver {
    active: AtomicBool,
    begun: AtomicUsize,
    completed: AtomicUsize,
    failed: AtomicUsize,
    reject_failures: AtomicBool,
    completed_at: std::sync::Mutex<Option<Instant>>,
}

impl Default for CountingUsageObserver {
    fn default() -> Self {
        Self {
            active: AtomicBool::new(true),
            begun: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            reject_failures: AtomicBool::new(false),
            completed_at: std::sync::Mutex::new(None),
        }
    }
}

impl FilesystemUsageObserver for CountingUsageObserver {
    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    fn begin_observation(&self) -> FilesystemUsageObservation {
        let sequence = self.begun.fetch_add(1, Ordering::AcqRel) as u64 + 1;
        FilesystemUsageObservation {
            generation: 1,
            sequence,
        }
    }

    fn complete_observation(
        &self,
        _observation: FilesystemUsageObservation,
        _usage: Option<AgentFilesystemUsage>,
        now: Instant,
    ) {
        *self.completed_at.lock().unwrap() = Some(now);
        self.completed.fetch_add(1, Ordering::AcqRel);
    }

    fn fail_observation(&self, _observation: FilesystemUsageObservation) -> bool {
        self.failed.fetch_add(1, Ordering::AcqRel);
        !self.reject_failures.load(Ordering::Acquire)
    }
}

fn agent_id() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId::from_agent_name_string(ComponentId::new(), "agent").unwrap(),
    )
}

async fn file_loader_with_content(
    environment_id: EnvironmentId,
    cache_parent: Option<&Path>,
    content: &[u8],
) -> (
    Arc<FileLoader>,
    golem_common::model::agent::AgentFileContentHash,
) {
    let service = Arc::new(InitialAgentFilesService::new(Arc::new(
        InMemoryBlobStorage::new(),
    )));
    let hash = service
        .put_if_not_exists(
            environment_id,
            content
                .to_vec()
                .map_error(widen_infallible::<anyhow::Error>)
                .map_item(|item| item.map_err(widen_infallible::<anyhow::Error>)),
        )
        .await
        .unwrap();
    (
        Arc::new(FileLoader::new(service, cache_parent).unwrap()),
        hash,
    )
}

fn initial_file(
    content_hash: golem_common::model::agent::AgentFileContentHash,
    path: &str,
    permissions: AgentFilePermissions,
    size: u64,
) -> InitialAgentFile {
    InitialAgentFile {
        content_hash,
        path: AgentFilePath::from_abs_str(path).unwrap(),
        permissions,
        size,
    }
}

#[cfg(target_os = "linux")]
#[test]
async fn mutation_failure_preserves_guest_results_and_effect_evidence() {
    let runtime = AgentFilesystemRuntime::new_for_test();

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::Guest("not-found"),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::PreserveGuest("not-found")
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<&str>::Io(std::io::Error::from_raw_os_error(libc::EBUSY)),
                MutationEffect::KnownCompletedPrefix { bytes: 7 },
            )
            .await,
        MutationDecision::BoundedRetry
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<&str>::Io(std::io::Error::other("unclassified")),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::BoundedRetry
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<&str>::Io(std::io::Error::from_raw_os_error(libc::EINTR)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::BoundedRetry
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<&str>::Io(std::io::Error::from_raw_os_error(libc::EAGAIN)),
                MutationEffect::DesiredPostconditionSatisfied,
            )
            .await,
        MutationDecision::Success
    );

    let unknown_guest_runtime = AgentFilesystemRuntime::new_for_test();
    assert_eq!(
        unknown_guest_runtime
            .classify_mutation_failure(MutationFailure::Guest("access"), MutationEffect::Unknown)
            .await,
        MutationDecision::Invalidate
    );
    assert!(unknown_guest_runtime.begin_effect().await.is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn native_write_errors_only_claim_no_effect_for_explicitly_safe_causes() {
    assert_eq!(
        native_write_failure_effect(&std::io::Error::from_raw_os_error(libc::EAGAIN), 0),
        MutationEffect::ProvenNoEffect
    );
    assert_eq!(
        native_write_failure_effect(&std::io::Error::from_raw_os_error(libc::EBUSY), 7),
        MutationEffect::KnownCompletedPrefix { bytes: 7 }
    );
    assert_eq!(
        native_write_failure_effect(&std::io::Error::from_raw_os_error(libc::ENOSPC), 0),
        MutationEffect::ProvenNoEffect
    );
    assert_eq!(
        native_write_failure_effect(&std::io::Error::from_raw_os_error(libc::EINTR), 0),
        MutationEffect::Unknown
    );
    assert_eq!(
        native_write_failure_effect(&std::io::Error::from(std::io::ErrorKind::TimedOut), 3),
        MutationEffect::Unknown
    );
    assert_eq!(
        native_write_failure_effect(&std::io::Error::other("unclassified"), 0),
        MutationEffect::Unknown
    );
}

#[test]
async fn unexplained_raw_permission_failure_invalidates_runtime() {
    let runtime = AgentFilesystemRuntime::new_for_test();

    assert_eq!(
        runtime
            .classify_mutation_failure::<()>(
                MutationFailure::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied,)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Invalidate
    );
    assert!(runtime.begin_effect().await.is_err());
}

#[cfg(target_os = "linux")]
#[test]
async fn stale_or_disappeared_backing_device_invalidates_runtime() {
    for errno in [libc::ESTALE, libc::ENODEV] {
        let runtime = AgentFilesystemRuntime::new_for_test();
        assert_eq!(
            runtime
                .classify_mutation_failure(
                    MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(errno)),
                    MutationEffect::ProvenNoEffect,
                )
                .await,
            MutationDecision::Invalidate
        );
        assert!(runtime.begin_effect().await.is_err());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn wrapped_terminal_probe_errors_are_terminal() {
    for errno in [libc::EIO, libc::ESTALE, libc::ENODEV] {
        let error = FilesystemStorageError::io(
            "probe runtime filesystem",
            Path::new("<test>"),
            std::io::Error::from_raw_os_error(errno),
        );
        assert!(error.is_terminal_failure());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn changed_or_missing_live_xfs_limits_are_terminal() {
    let installed = ResolvedAgentFilesystemLimits {
        allocated_bytes: 1024 * 1024,
        filesystem_objects: 8192,
        filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
    };
    for observed in [
        None,
        Some(ResolvedAgentFilesystemLimits {
            allocated_bytes: 2 * 1024 * 1024,
            ..installed
        }),
    ] {
        let error = quota::validate_observed_limits(
            Path::new("<test-managed-xfs>"),
            Some(installed),
            observed,
        )
        .unwrap_err();
        assert!(error.is_terminal_failure());
    }
}

#[cfg(target_os = "linux")]
#[test]
async fn terminal_cause_invalidates_even_when_postcondition_is_satisfied() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EIO)),
                MutationEffect::DesiredPostconditionSatisfied,
            )
            .await,
        MutationDecision::Invalidate
    );
    assert!(runtime.begin_effect().await.is_err());
}

#[cfg(target_os = "linux")]
#[test]
async fn byte_mutation_ignores_exhausted_physical_inode_dimension() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
        None,
        None,
        FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 50,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        },
    );
    assert_eq!(
        runtime
            .classify_mutation_failure_for(
                MutationOperation::Write,
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EDQUOT)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn storage_exhaustion_uses_fresh_quota_and_capacity_observations() {
    let exhausted = FilesystemCapacity {
        total_bytes: 100,
        available_bytes: 0,
        total_filesystem_objects: 100,
        available_filesystem_objects: 0,
    };
    let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
        Some(AgentFilesystemUsage {
            allocated_bytes: 50,
            filesystem_objects: 10,
        }),
        Some(ResolvedAgentFilesystemLimits {
            allocated_bytes: 50,
            filesystem_objects: 10,
            filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
        }),
        exhausted,
    );

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );

    let unmanaged = AgentFilesystemRuntime::new_for_test_with_observations(None, None, exhausted);
    assert_eq!(
        unmanaged
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EDQUOT)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );
    assert_eq!(
        unmanaged
            .classify_mutation_failure(
                MutationFailure::StorageExhaustion {
                    guest: (),
                    quota_hint: true,
                },
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn unexplained_storage_exhaustion_preserves_errno_mapping() {
    let healthy = FilesystemCapacity {
        total_bytes: 100,
        available_bytes: 50,
        total_filesystem_objects: 100,
        available_filesystem_objects: 50,
    };
    let runtime = AgentFilesystemRuntime::new_for_test_with_observations(None, None, healthy);

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EDQUOT)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::InsufficientSpace
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn quota_classification_uses_the_operation_relevant_limit() {
    let capacity = FilesystemCapacity {
        total_bytes: 100,
        available_bytes: 50,
        total_filesystem_objects: 100,
        available_filesystem_objects: 50,
    };
    let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
        Some(AgentFilesystemUsage {
            allocated_bytes: 50,
            filesystem_objects: 10,
        }),
        Some(ResolvedAgentFilesystemLimits {
            allocated_bytes: 100,
            filesystem_objects: 10,
            filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
        }),
        capacity,
    );

    assert_eq!(
        runtime
            .classify_mutation_failure_for(
                MutationOperation::Write,
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::InsufficientSpace
    );
    assert_eq!(
        runtime
            .classify_mutation_failure_for(
                MutationOperation::Create,
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Quota
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn storage_probe_failure_preserves_errno_when_effect_is_known() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_capacity_observation_failure();

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::ENOSPC)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::InsufficientSpace
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EDQUOT)),
                MutationEffect::KnownCompletedPrefix { bytes: 3 },
            )
            .await,
        MutationDecision::Quota
    );
    assert!(runtime.begin_effect().await.is_ok());
}

#[test]
async fn proven_no_effect_guest_failure_does_not_observe_usage() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::Guest("not-found"),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::PreserveGuest("not-found")
    );
    assert_eq!(observer.begun.load(Ordering::Acquire), 0);
    assert_eq!(observer.completed.load(Ordering::Acquire), 0);
    assert_eq!(observer.failed.load(Ordering::Acquire), 0);
}

#[test]
async fn completed_prefix_is_observed_before_early_invalidation() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    let observed_at_invalidation = Arc::new(AtomicUsize::new(0));
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.set_invalidation_callback(Some({
        let observer = Arc::clone(&observer);
        let observed_at_invalidation = Arc::clone(&observed_at_invalidation);
        Arc::new(move || {
            let completed = observer.completed.load(Ordering::Acquire);
            let observed_at_invalidation = Arc::clone(&observed_at_invalidation);
            Box::pin(async move {
                observed_at_invalidation.store(completed, Ordering::Release);
            })
        })
    }));

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Infrastructure(std::io::Error::other(
                    "terminal backend failure",
                )),
                MutationEffect::KnownCompletedPrefix { bytes: 3 },
            )
            .await,
        MutationDecision::Invalidate
    );
    assert_eq!(observer.completed.load(Ordering::Acquire), 1);
    assert_eq!(observed_at_invalidation.load(Ordering::Acquire), 1);
    assert!(runtime.begin_effect().await.is_err());
}

#[test]
async fn known_effect_preserves_classifier_behavior_without_billing_observer() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::Guest("not-found"),
                MutationEffect::KnownCompletedPrefix { bytes: 3 },
            )
            .await,
        MutationDecision::PreserveGuest("not-found")
    );
    assert!(runtime.begin_effect().await.is_ok());
}

#[test]
async fn billing_usage_failure_invalidates_before_classification() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::Guest("not-found"),
                MutationEffect::KnownCompletedPrefix { bytes: 3 },
            )
            .await,
        MutationDecision::Invalidate
    );
    assert_eq!(observer.completed.load(Ordering::Acquire), 0);
    assert_eq!(observer.failed.load(Ordering::Acquire), 1);
    assert!(runtime.begin_effect().await.is_err());
}

#[test]
async fn successful_usage_is_installed_before_capacity_observation_failure() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_capacity_observation_failure();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));

    let decision = runtime
        .classify_mutation_failure(
            MutationFailure::TransientGuest("busy"),
            MutationEffect::ProvenNoEffect,
        )
        .await;

    assert_eq!(decision, MutationDecision::PreserveGuest("busy"));
    assert_eq!(observer.completed.load(Ordering::Acquire), 1);
    assert_eq!(observer.failed.load(Ordering::Acquire), 0);
    assert!(runtime.begin_effect().await.is_ok());
}

#[cfg(target_os = "linux")]
#[test]
async fn invalidating_mutation_failure_seals_runtime_and_notifies_once() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let notifications = Arc::new(AtomicUsize::new(0));
    runtime.set_invalidation_callback(Some({
        let notifications = Arc::clone(&notifications);
        Arc::new(move || {
            let notifications = Arc::clone(&notifications);
            Box::pin(async move {
                notifications.fetch_add(1, Ordering::AcqRel);
            })
        })
    }));

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EINTR)),
                MutationEffect::Unknown,
            )
            .await,
        MutationDecision::Invalidate
    );
    assert!(runtime.begin_effect().await.is_err());
    assert_eq!(notifications.load(Ordering::Acquire), 1);

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EIO)),
                MutationEffect::DesiredPostconditionSatisfied,
            )
            .await,
        MutationDecision::Invalidate
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Infrastructure(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "backend policy rejected access",
                )),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::Invalidate
    );
    assert_eq!(notifications.load(Ordering::Acquire), 1);
}

#[cfg(target_os = "linux")]
#[test]
async fn pending_worker_interrupt_suppresses_mutation_retry() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    runtime.set_retry_callback(Some(Arc::new(|| Box::pin(async { false }))));

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::TransientGuest("busy"),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::PreserveGuest("busy")
    );
    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EAGAIN)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::PreserveRaw
    );
}

#[cfg(target_os = "linux")]
#[test]
async fn failed_health_probe_suppresses_transient_retry() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_capacity_observation_failure();

    assert_eq!(
        runtime
            .classify_mutation_failure(
                MutationFailure::<()>::Io(std::io::Error::from_raw_os_error(libc::EAGAIN)),
                MutationEffect::ProvenNoEffect,
            )
            .await,
        MutationDecision::PreserveRaw
    );
    assert!(runtime.begin_effect().await.is_ok());
}

#[cfg(target_os = "linux")]
#[test]
async fn unmanaged_runtime_observes_fresh_physical_capacity() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();

    let capacity = filesystem.runtime().capacity().await.unwrap();

    assert!(capacity.total_bytes > 0);
    assert!(capacity.available_bytes <= capacity.total_bytes);
    assert!(capacity.total_filesystem_objects > 0);
    assert!(capacity.available_filesystem_objects <= capacity.total_filesystem_objects);
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn short_effect_batch_is_observed_on_the_bounded_cadence() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));
    let first = runtime.begin_effect().await.unwrap();
    let second = runtime.begin_effect().await.unwrap();

    drop(first);
    drop(second);
    runtime.drain().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    assert_eq!(observer.completed.load(Ordering::Acquire), 1);
    assert_eq!(observer.begun.load(Ordering::Acquire), 1);
}

#[test]
async fn short_effect_final_sample_is_debounced_from_drain() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));
    // Start the sampler after the drain so scheduler load cannot race the debounce assertion.
    runtime.inner.usage_sampling.store(true, Ordering::Release);
    let effect = runtime.begin_effect().await.unwrap();

    let drained_at = Instant::now();
    drop(effect);
    runtime.drain().await;
    runtime.inner.usage_sampling.store(false, Ordering::Release);
    runtime.inner.schedule_usage_sampling();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(observer.completed.load(Ordering::Acquire), 1);
    assert!(
        observer.completed_at.lock().unwrap().unwrap() - drained_at
            >= std::time::Duration::from_millis(10)
    );
}

#[test]
async fn paused_effect_admission_drains_existing_effects_and_rejects_new_ones() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let effect = runtime.begin_effect().await.unwrap();
    let admission_pause = runtime.pause_effect_admission();

    assert!(runtime.begin_effect().await.is_err());
    drop(effect);
    runtime.drain().await;
    drop(admission_pause);

    assert!(runtime.begin_effect().await.is_ok());
}

#[test]
async fn update_effect_waits_for_paused_admission_to_resume() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let admission_pause = runtime.pause_effect_admission();
    let update = tokio::spawn({
        let runtime = runtime.clone();
        async move { runtime.begin_update_effect().await }
    });

    tokio::task::yield_now().await;
    assert!(!update.is_finished());
    drop(admission_pause);

    assert!(update.await.unwrap().is_ok());
}

#[test]
async fn sampler_exit_hands_off_to_an_effect_admitted_during_teardown() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.inner.usage_sampling.store(true, Ordering::Release);
    let effect = runtime.begin_effect().await.unwrap();

    runtime.inner.finish_usage_sampling(0);

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(effect);
}

#[test]
async fn sampler_exit_does_not_restart_an_inactive_window() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    observer.active.store(false, Ordering::Release);
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.inner.usage_sampling.store(true, Ordering::Release);
    let effect = runtime.begin_effect().await.unwrap();

    runtime.inner.finish_usage_sampling(0);

    assert!(!runtime.inner.usage_sampling.load(Ordering::Acquire));
    assert_eq!(observer.begun.load(Ordering::Acquire), 0);
    drop(effect);
}

#[test]
fn sampler_exit_does_not_restart_without_pending_effects() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.inner.usage_sampling.store(true, Ordering::Release);

    runtime.inner.finish_usage_sampling(0);

    assert!(!runtime.inner.usage_sampling.load(Ordering::Acquire));
    assert_eq!(observer.begun.load(Ordering::Acquire), 0);
}

#[test]
async fn sustained_effects_use_a_slower_cadence_until_completion() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let observer = Arc::new(CountingUsageObserver::default());
    runtime.set_usage_observer(Some(observer.clone()));
    let effect = runtime.begin_effect().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let first_sample_at = observer.completed_at.lock().unwrap().unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second_sample_at = observer.completed_at.lock().unwrap().unwrap();
    assert!(second_sample_at - first_sample_at >= std::time::Duration::from_millis(100));
    assert!(runtime.has_active_effects());

    let samples_before_completion = observer.completed.load(Ordering::Acquire);
    drop(effect);
    runtime.drain().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.completed.load(Ordering::Acquire) <= samples_before_completion {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[test]
async fn failed_scheduled_usage_observation_invalidates_runtime() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();
    let observer = Arc::new(CountingUsageObserver::default());
    let invalidated = Arc::new(AtomicBool::new(false));
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.set_invalidation_callback(Some({
        let invalidated = Arc::clone(&invalidated);
        Arc::new(move || {
            let invalidated = Arc::clone(&invalidated);
            Box::pin(async move {
                invalidated.store(true, Ordering::Release);
            })
        })
    }));
    let effect = runtime.begin_effect().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !invalidated.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    assert_eq!(observer.failed.load(Ordering::Acquire), 1);
    assert!(runtime.begin_effect().await.is_err());
    drop(effect);
}

#[test]
async fn rejected_scheduled_usage_failure_does_not_invalidate_runtime() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();
    let observer = Arc::new(CountingUsageObserver::default());
    observer.reject_failures.store(true, Ordering::Release);
    let invalidated = Arc::new(AtomicBool::new(false));
    runtime.set_usage_observer(Some(observer.clone()));
    runtime.set_invalidation_callback(Some({
        let invalidated = Arc::clone(&invalidated);
        Arc::new(move || {
            let invalidated = Arc::clone(&invalidated);
            Box::pin(async move {
                invalidated.store(true, Ordering::Release);
            })
        })
    }));
    let effect = runtime.begin_effect().await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while observer.failed.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(effect);
    runtime.drain().await;

    assert!(!invalidated.load(Ordering::Acquire));
    assert!(runtime.begin_effect().await.is_ok());
}

#[test]
async fn rejected_forced_usage_failure_is_ignored() {
    let runtime = AgentFilesystemRuntime::new_for_test_with_failed_observations();
    let observer = Arc::new(CountingUsageObserver::default());
    observer.reject_failures.store(true, Ordering::Release);
    runtime.set_usage_observer(Some(observer.clone()));

    runtime.observe_usage_for_billing().await.unwrap();

    assert_eq!(observer.failed.load(Ordering::Acquire), 1);
    assert!(runtime.begin_effect().await.is_ok());
}

#[test]
fn pressure_policy_uses_independent_minimum_and_target_watermarks() {
    let policy = FilesystemPressureConfig {
        minimum_available_bytes: 10,
        target_available_bytes: 20,
        minimum_available_filesystem_objects: 2,
        target_available_filesystem_objects: 4,
        ..FilesystemPressureConfig::default()
    };
    let byte_pressure = policy
        .pressure(
            MutationOperation::Write,
            FilesystemCapacity {
                total_bytes: 100,
                available_bytes: 10,
                total_filesystem_objects: 100,
                available_filesystem_objects: 100,
            },
        )
        .unwrap();
    assert!(!policy.target_reached(
        byte_pressure,
        FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 19,
            total_filesystem_objects: 100,
            available_filesystem_objects: 100,
        }
    ));
    assert!(policy.target_reached(
        byte_pressure,
        FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 20,
            total_filesystem_objects: 100,
            available_filesystem_objects: 0,
        }
    ));

    let object_capacity = FilesystemCapacity {
        total_bytes: 100,
        available_bytes: 100,
        total_filesystem_objects: 100,
        available_filesystem_objects: 2,
    };
    assert!(
        policy
            .pressure(MutationOperation::Write, object_capacity)
            .is_none()
    );
    let object_pressure = policy
        .pressure(MutationOperation::Create, object_capacity)
        .unwrap();
    assert!(policy.target_reached(
        object_pressure,
        FilesystemCapacity {
            available_filesystem_objects: 4,
            ..object_capacity
        }
    ));
}

#[test]
fn pressure_policy_rejects_targets_below_minimums() {
    assert!(
        FilesystemPressureConfig {
            minimum_available_bytes: 2,
            target_available_bytes: 1,
            minimum_available_filesystem_objects: 1,
            target_available_filesystem_objects: 1,
            ..FilesystemPressureConfig::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        FilesystemPressureConfig {
            minimum_available_bytes: 1,
            target_available_bytes: 1,
            minimum_available_filesystem_objects: 1,
            target_available_filesystem_objects: 1,
            ..FilesystemPressureConfig::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        FilesystemPressureConfig {
            minimum_available_bytes: 1,
            target_available_bytes: 101,
            minimum_available_filesystem_objects: 1,
            target_available_filesystem_objects: 1,
            ..FilesystemPressureConfig::default()
        }
        .validate_capacity(FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 100,
            total_filesystem_objects: 1,
            available_filesystem_objects: 1,
        })
        .is_err()
    );
}

#[test]
fn default_object_limit_policy_resolves_storage_levels() {
    let policy = FilesystemObjectLimitPolicyConfig::default();

    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 128 * 1024 * 1024,
            },)
            .unwrap(),
        ResolvedAgentFilesystemLimits {
            allocated_bytes: 128 * 1024 * 1024,
            filesystem_objects: 8_192,
            filesystem_object_limit_policy_version: 2,
        }
    );
    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 384 * 1024 * 1024,
            },)
            .unwrap()
            .filesystem_objects,
        12_288
    );
    assert_eq!(
        policy
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: 1024 * 1024 * 1024,
            },)
            .unwrap()
            .filesystem_objects,
        32_768
    );
}

#[test]
fn object_limit_policy_rejects_unrepresentable_inputs() {
    let policy = FilesystemObjectLimitPolicyConfig::default();

    assert!(
        policy
            .resolve(AgentFilesystemStorageLimit { allocated_bytes: 0 })
            .is_err()
    );
    let overflowing = FilesystemObjectLimitPolicyConfig {
        objects_per_gib: u64::MAX,
        maximum_objects: u64::MAX,
        ..policy.clone()
    };
    assert!(
        overflowing
            .resolve(AgentFilesystemStorageLimit {
                allocated_bytes: u64::MAX,
            })
            .is_err()
    );

    let invalid = FilesystemObjectLimitPolicyConfig {
        objects_per_gib: 0,
        ..policy
    };
    assert!(invalid.validate().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn managed_backend_fails_closed_on_non_xfs() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };

    let error = match AgentFilesystems::new(&settings) {
        Ok(_) => panic!("managed backend unexpectedly accepted a non-XFS root"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("validate managed XFS root"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
async fn managed_xfs_owns_observes_and_cleans_project_filesystem() {
    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let settings = FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root.clone()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();

    let second_owner = AgentFilesystems::new(&settings);
    assert!(second_owner.is_err());

    let escaped_id = agent_id();
    let outside = tempfile::tempdir().unwrap();
    let environment_link = root.join(escaped_id.environment_id.to_string());
    std::os::unix::fs::symlink(outside.path(), &environment_link).unwrap();
    assert!(filesystems.create_owned_empty(&escaped_id).await.is_err());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    std::fs::remove_file(environment_link).unwrap();

    let stale_file_id = agent_id();
    let backend = Arc::clone(filesystems.managed_xfs.as_ref().unwrap());
    let environment = stale_file_id.environment_id.to_string();
    let component = stale_file_id.agent_id.component_id.to_string();
    let agent = stale_file_id.agent_id.agent_name_encoded();
    let owner = PathBuf::from(&environment).join(&component).join(&agent);
    let parent = backend.open_agent_parent(&environment, &component).unwrap();
    let parent_path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd()));
    let stale_file = parent_path.join(&agent);
    let staging = parent_path.join(format!("{agent}.staging"));
    std::fs::create_dir(&staging).unwrap();
    let stale_project = backend.reserve_project(&owner).unwrap();
    let staging_directory = File::open(&staging).unwrap();
    backend
        .assign_project(&staging_directory, stale_project)
        .unwrap();
    std::fs::write(staging.join("file"), b"stale").unwrap();
    std::fs::rename(staging.join("file"), &stale_file).unwrap();
    drop(staging_directory);
    std::fs::remove_dir(staging).unwrap();
    drop(parent);

    let stale_file_replacement = filesystems
        .create_owned_empty(&stale_file_id)
        .await
        .unwrap();
    assert!(stale_file_replacement.path().is_dir());
    stale_file_replacement.close_and_delete().await.unwrap();
    assert_eq!(
        backend.usage(stale_project).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );

    let capacity = filesystems.capacity().await.unwrap();
    assert!(capacity.total_bytes > 0);
    assert!(capacity.available_bytes <= capacity.total_bytes);
    assert!(capacity.total_filesystem_objects > 0);
    assert!(capacity.available_filesystem_objects <= capacity.total_filesystem_objects);

    let materialized_id = agent_id();
    let content = vec![0x5a; 8192];
    let (file_loader, content_hash) = file_loader_with_content(
        materialized_id.environment_id,
        filesystems.initial_file_cache_root(),
        &content,
    )
    .await;
    let cached_source = file_loader
        .get_source(
            materialized_id.environment_id,
            content_hash,
            content.len() as u64,
        )
        .await
        .unwrap();
    let managed_backend = Arc::clone(filesystems.managed_xfs.as_ref().unwrap());
    assert_eq!(
        managed_backend
            .project_id(&File::open(cached_source.path()).unwrap())
            .unwrap(),
        None,
        "the shared cache source must not inherit an agent project"
    );
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: materialized_id.clone(),
            initial_files: vec![
                initial_file(
                    content_hash,
                    "/immutable-a",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/immutable-b",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let path = filesystem.path().to_path_buf();
    let (backend, project_id) = match &filesystem.storage {
        AgentFilesystemStorage::Managed {
            backend,
            project_id,
            ..
        } => (Arc::clone(backend), *project_id),
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    let materialized_usage = filesystem.usage().await.unwrap().unwrap();
    assert!(materialized_usage.allocated_bytes >= 3 * 8192);
    assert!(materialized_usage.filesystem_objects >= 4);
    assert_eq!(std::fs::read(path.join("immutable-a")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("immutable-b")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("writable")).unwrap(), content);
    let immutable_a = File::open(path.join("immutable-a")).unwrap();
    let immutable_b = File::open(path.join("immutable-b")).unwrap();
    let writable = File::open(path.join("writable")).unwrap();
    assert_eq!(backend.project_id(&immutable_a).unwrap(), Some(project_id));
    assert_eq!(backend.project_id(&immutable_b).unwrap(), Some(project_id));
    assert_eq!(backend.project_id(&writable).unwrap(), Some(project_id));
    drop((immutable_a, immutable_b, writable));

    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: 128 * 1024 * 1024,
        })
        .await
        .unwrap();

    filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            materialized_id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/immutable-a",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/immutable-c",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
        )
        .await
        .unwrap();
    assert!(!path.join("immutable-b").exists());
    assert_eq!(std::fs::read(path.join("immutable-c")).unwrap(), content);
    let immutable_c = File::open(path.join("immutable-c")).unwrap();
    assert_eq!(backend.project_id(&immutable_c).unwrap(), Some(project_id));
    drop(immutable_c);

    let usage_before_cow = filesystem.usage().await.unwrap().unwrap();
    std::fs::write(path.join("writable"), vec![0x6c; content.len()]).unwrap();
    assert_eq!(std::fs::read(path.join("immutable-a")).unwrap(), content);
    assert_eq!(std::fs::read(path.join("immutable-c")).unwrap(), content);
    let usage_after_cow = filesystem.usage().await.unwrap().unwrap();
    assert!(usage_after_cow.allocated_bytes >= usage_before_cow.allocated_bytes);

    use std::io::{Seek, SeekFrom, Write};
    let usage_before_sparse = filesystem.usage().await.unwrap().unwrap();
    let sparse_path = path.join("sparse");
    let mut sparse = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&sparse_path)
        .unwrap();
    sparse.seek(SeekFrom::Start(4 * 1024 * 1024)).unwrap();
    sparse.write_all(&[0x7d]).unwrap();
    sparse.sync_all().unwrap();
    rustix::fs::syncfs(&File::open(path).unwrap()).unwrap();
    let usage_after_sparse = filesystem.usage().await.unwrap().unwrap();
    assert_eq!(
        std::fs::metadata(&sparse_path).unwrap().len(),
        4 * 1024 * 1024 + 1
    );
    assert!(usage_after_sparse.allocated_bytes > usage_before_sparse.allocated_bytes);
    assert!(
        usage_after_sparse.allocated_bytes - usage_before_sparse.allocated_bytes < 1024 * 1024,
        "sparse logical extension must be charged by physical allocation"
    );

    let dense_path = path.join("dense");
    let mut dense = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&dense_path)
        .unwrap();
    dense.write_all(&vec![0x4e; 4096]).unwrap();
    dense.sync_all().unwrap();
    let capacity_during_allocation = filesystems.capacity().await.unwrap();
    assert!(capacity_during_allocation.available_bytes < capacity.available_bytes);
    assert!(
        capacity_during_allocation.available_filesystem_objects
            < capacity.available_filesystem_objects
    );
    let usage = filesystem.usage().await.unwrap().unwrap();
    assert!(usage.allocated_bytes > 4096);
    let limit_exceeded = Arc::new(AtomicBool::new(false));
    filesystem.runtime().set_limit_exceeded_callback(Some({
        let limit_exceeded = Arc::clone(&limit_exceeded);
        Arc::new(move |exceeded| {
            let limit_exceeded = Arc::clone(&limit_exceeded);
            Box::pin(async move {
                if exceeded {
                    limit_exceeded.store(true, Ordering::Release);
                }
            })
        })
    }));
    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: usage.allocated_bytes,
        })
        .await
        .unwrap();
    assert!(!limit_exceeded.load(Ordering::Acquire));
    dense.seek(SeekFrom::Start(0)).unwrap();
    dense.write_all(&vec![0x5f; 4096]).unwrap();
    dense.sync_all().unwrap();
    assert_eq!(
        filesystem.usage().await.unwrap().unwrap().allocated_bytes,
        usage.allocated_bytes,
        "overwriting allocated blocks at quota equality must not consume capacity"
    );
    let allocation_error = backend
        .materialize_initial_file(
            &path,
            project_id,
            cached_source.path(),
            &path.join("exact-limit-allocation"),
            false,
        )
        .expect_err("allocating at the exact byte limit must be denied");
    assert!(
        matches!(
            allocation_error.kind(),
            std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
        ),
        "unexpected exact-limit allocation error: {allocation_error:?}"
    );
    drop((sparse, dense));
    filesystem
        .runtime()
        .set_allocated_byte_limit(AgentFilesystemStorageLimit {
            allocated_bytes: usage.allocated_bytes - 4096,
        })
        .await
        .unwrap();
    assert!(limit_exceeded.load(Ordering::Acquire));

    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
    assert_eq!(
        backend.usage(project_id).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );
    let mut capacity_after_deletion = filesystems.capacity().await.unwrap();
    for _ in 0..50 {
        if capacity_after_deletion.available_bytes > capacity_during_allocation.available_bytes
            && capacity_after_deletion.available_filesystem_objects
                > capacity_during_allocation.available_filesystem_objects
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        capacity_after_deletion = filesystems.capacity().await.unwrap();
    }
    assert!(capacity_after_deletion.available_bytes > capacity_during_allocation.available_bytes);
    assert!(
        capacity_after_deletion.available_filesystem_objects
            > capacity_during_allocation.available_filesystem_objects
    );

    let over_limit_id = agent_id();
    let over_limit_content = vec![0x7b; 8192];
    let (over_limit_loader, over_limit_hash) = file_loader_with_content(
        over_limit_id.environment_id,
        filesystems.initial_file_cache_root(),
        &over_limit_content,
    )
    .await;
    let error = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: over_limit_id,
            initial_files: vec![initial_file(
                over_limit_hash,
                "/over-limit-initial-file",
                AgentFilePermissions::ReadOnly,
                over_limit_content.len() as u64,
            )],
            file_loader: over_limit_loader,
            resource_limits: Some(Arc::new(AtomicResourceEntry::new(
                u64::MAX,
                usize::MAX,
                usize::MAX,
                4096,
                u64::MAX,
            ))),
            limit_exceeded: None,
        })
        .await;
    assert!(
        error.is_err(),
        "initial files above the installed byte limit must prevent startup"
    );

    let object_limited = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let (object_backend, object_project) = match &object_limited.storage {
        AgentFilesystemStorage::Managed {
            backend,
            project_id,
            ..
        } => (Arc::clone(backend), *project_id),
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    object_backend
        .install_project_limits(
            object_project,
            ResolvedAgentFilesystemLimits {
                allocated_bytes: 128 * 1024 * 1024,
                filesystem_objects: 2,
                filesystem_object_limit_policy_version: FILESYSTEM_OBJECT_LIMIT_POLICY_VERSION,
            },
        )
        .unwrap();
    let object_path = object_limited.path().join("object");
    std::fs::write(&object_path, []).unwrap();
    std::fs::hard_link(&object_path, object_limited.path().join("alias")).unwrap();
    assert_eq!(
        object_limited
            .usage()
            .await
            .unwrap()
            .unwrap()
            .filesystem_objects,
        2
    );
    let object_error = std::fs::write(object_limited.path().join("exhausted"), [])
        .expect_err("a new inode must exceed the project object limit");
    assert_eq!(
        object_error.raw_os_error(),
        Some(rustix::io::Errno::NOSPC.raw_os_error())
    );

    let open_unlinked = File::open(&object_path).unwrap();
    std::fs::remove_file(&object_path).unwrap();
    std::fs::remove_file(object_limited.path().join("alias")).unwrap();
    assert_eq!(
        object_limited
            .usage()
            .await
            .unwrap()
            .unwrap()
            .filesystem_objects,
        2
    );
    drop(open_unlinked);
    object_limited.close_and_delete().await.unwrap();
    assert_eq!(
        object_backend.usage(object_project).unwrap(),
        AgentFilesystemUsage {
            allocated_bytes: 0,
            filesystem_objects: 0,
        }
    );

    let deferred = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let deferred_project = match &deferred.storage {
        AgentFilesystemStorage::Managed { project_id, .. } => *project_id,
        AgentFilesystemStorage::Unmanaged => panic!("managed mode fell back to unmanaged"),
    };
    let retained_root = File::open(deferred.path()).unwrap();
    drop(deferred);
    drop(retained_root);
    let mut released = false;
    for _ in 0..500 {
        if backend.usage(deferred_project).unwrap()
            == (AgentFilesystemUsage {
                allocated_bytes: 0,
                filesystem_objects: 0,
            })
        {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(released, "deferred managed project cleanup did not finish");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires the privileged managed XFS test runner"]
async fn managed_xfs_allocated_bytes_flow_through_resource_billing() {
    use std::io::{Seek, SeekFrom, Write};

    let root = std::env::var_os("GOLEM_MANAGED_XFS_TEST_ROOT")
        .map(PathBuf::from)
        .expect("GOLEM_MANAGED_XFS_TEST_ROOT must name the mounted XFS test root");
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig {
        managed_xfs_root_dir: Some(root),
        ..FilesystemStorageConfig::default()
    })
    .unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let runtime = filesystem.runtime();
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0));
    let now = Instant::now();
    let memory = LinearMemoryTracker::new(
        0,
        0,
        AgentMode::Durable,
        false,
        entry.clone(),
        Arc::new(std::sync::Mutex::new(MemoryGrant::inert(0))),
        now,
    );
    let meter = AgentResourceBilling::new(AgentMode::Durable, memory, entry.clone(), now);
    runtime.set_usage_observer(Some(Arc::new(meter.clone())));
    let opening_usage = runtime.usage().await.unwrap().unwrap();
    let window_started = Instant::now();
    meter.open(&runtime).await.unwrap();

    let effect = runtime.begin_effect().await.unwrap();
    let sparse_path = filesystem.path().join("billed-sparse-file");
    let mut sparse = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&sparse_path)
        .unwrap();
    sparse.seek(SeekFrom::Start(64 * 1024 * 1024)).unwrap();
    sparse.write_all(&[0x7d]).unwrap();
    sparse.sync_all().unwrap();
    drop((sparse, effect));
    runtime.drain().await;

    let logical_bytes = std::fs::metadata(&sparse_path).unwrap().len();
    let usage = runtime.usage().await.unwrap().unwrap();
    assert!(usage.allocated_bytes > 0);
    assert!(usage.allocated_bytes < logical_bytes);
    runtime.observe_usage_for_billing().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    meter.close(&runtime).await.unwrap();
    let elapsed = window_started.elapsed().as_secs_f64();
    meter.flush(Instant::now());
    let billed = entry.durable_byte_seconds_delta();
    let minimum = ((usage.allocated_bytes as f64 * 0.1).floor() as i64).max(1);
    let maximum_level = opening_usage.allocated_bytes.max(usage.allocated_bytes);
    let maximum = (maximum_level as f64 * (elapsed + 0.25)).ceil() as i64;
    assert!(
        billed >= minimum,
        "authoritative allocation produced too little billing: usage={usage:?}, billed={billed}"
    );
    assert!(
        billed <= maximum,
        "authoritative allocation produced too much billing: usage={usage:?}, elapsed={elapsed}, billed={billed}"
    );
    assert!(
        billed < logical_bytes as i64,
        "sparse logical length was billed instead of authoritative allocation"
    );

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    meter.flush(Instant::now());
    assert_eq!(entry.durable_byte_seconds_delta(), billed);

    runtime.set_usage_observer(None);
    filesystem.close_and_delete().await.unwrap();
}

#[cfg(unix)]
#[test]
async fn unmanaged_materialization_creates_distinct_owned_files() {
    use std::os::unix::fs::MetadataExt;

    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"shared initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id,
            initial_files: vec![
                initial_file(
                    content_hash,
                    "/first/immutable",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/second/immutable",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
            file_loader,
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    let first = filesystem.path().join("first/immutable");
    let second = filesystem.path().join("second/immutable");
    let writable = filesystem.path().join("writable");
    assert_eq!(std::fs::read(&first).unwrap(), content);
    assert_eq!(std::fs::read(&second).unwrap(), content);
    assert_eq!(std::fs::read(&writable).unwrap(), content);
    assert_ne!(
        first.metadata().unwrap().ino(),
        second.metadata().unwrap().ino()
    );
    assert_ne!(
        first.metadata().unwrap().ino(),
        writable.metadata().unwrap().ino()
    );
    assert!(filesystem.runtime().is_read_only(&first));
    assert!(filesystem.runtime().is_read_only(&second));
    assert!(!filesystem.runtime().is_read_only(&writable));
    assert!(
        filesystem
            .runtime()
            .is_read_only(&filesystem.path().join("first/../first/immutable"))
    );
    std::os::unix::fs::symlink(&first, filesystem.path().join("immutable-link")).unwrap();
    assert!(
        filesystem
            .runtime()
            .is_read_only(&filesystem.path().join("immutable-link"))
    );
    assert!(
        !filesystem
            .runtime()
            .is_read_only_path(&filesystem.path().join("immutable-link"), false,)
    );
    tokio::fs::write(&writable, b"changed").await.unwrap();

    let path = filesystem.path().to_path_buf();
    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
}

#[test]
async fn failed_initial_file_update_preserves_current_files() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: vec![initial_file(
                content_hash,
                "/current",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    let result = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/new",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/invalid",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64 + 1,
                ),
            ],
        )
        .await;

    assert!(result.is_err());
    let current = filesystem.path().join("current");
    assert_eq!(std::fs::read(&current).unwrap(), content);
    assert!(filesystem.runtime().is_read_only(&current));
    assert!(!filesystem.path().join("new").exists());
    assert!(!filesystem.path().join("invalid").exists());
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_update_commits_staged_files_and_policy_together() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: vec![initial_file(
                content_hash,
                "/old",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();

    filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[
                initial_file(
                    content_hash,
                    "/new",
                    AgentFilePermissions::ReadOnly,
                    content.len() as u64,
                ),
                initial_file(
                    content_hash,
                    "/writable",
                    AgentFilePermissions::ReadWrite,
                    content.len() as u64,
                ),
            ],
        )
        .await
        .unwrap();

    let new = filesystem.path().join("new");
    let writable = filesystem.path().join("writable");
    assert!(!filesystem.path().join("old").exists());
    assert_eq!(std::fs::read(&new).unwrap(), content);
    assert_eq!(std::fs::read(&writable).unwrap(), content);
    assert!(filesystem.runtime().is_read_only(&new));
    assert!(!filesystem.runtime().is_read_only(&writable));
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_updates_are_exclusive_with_filesystem_effects() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let effect = runtime.begin_effect().await.unwrap();
    let update_runtime = runtime.clone();
    let update = tokio::spawn(async move { update_runtime.begin_update_effect().await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!update.is_finished());

    drop(effect);
    let update = update.await.unwrap();
    let effect_runtime = runtime.clone();
    let next_effect = tokio::spawn(async move { effect_runtime.begin_effect().await.unwrap() });
    tokio::task::yield_now().await;
    assert!(!next_effect.is_finished());

    drop(update);
    drop(next_effect.await.unwrap());
}

#[test]
fn dropped_initial_file_transaction_restores_backups() {
    let root = tempfile::tempdir().unwrap();
    let live = root.path().join("live");
    let staged = root.path().join("staged");
    let backup = root.path().join("backups");
    std::fs::write(&live, b"old").unwrap();
    std::fs::write(&staged, b"new").unwrap();
    std::fs::create_dir(&backup).unwrap();

    {
        let mut transaction = InitialFileUpdateTransaction::new(backup);
        transaction.back_up(&live).unwrap();
        transaction.install(&staged, &live).unwrap();
    }

    assert_eq!(std::fs::read(&live).unwrap(), b"old");
}

#[test]
async fn initial_file_update_rejects_guest_file_collision() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: Vec::new(),
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let collision = filesystem.path().join("collision");
    std::fs::write(&collision, b"guest data").unwrap();

    let result = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[initial_file(
                content_hash,
                "/collision",
                AgentFilePermissions::ReadOnly,
                content.len() as u64,
            )],
        )
        .await;

    assert!(result.is_err());
    assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn initial_file_update_preserves_guest_file_for_read_write_target() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let content = b"initial content";
    let (file_loader, content_hash) =
        file_loader_with_content(id.environment_id, None, content).await;
    let filesystem = filesystems
        .create_fresh(CreateAgentFilesystem {
            agent_id: id.clone(),
            initial_files: Vec::new(),
            file_loader: Arc::clone(&file_loader),
            resource_limits: None,
            limit_exceeded: None,
        })
        .await
        .unwrap();
    let collision = filesystem.path().join("collision");
    std::fs::write(&collision, b"guest data").unwrap();

    let update = filesystem
        .runtime()
        .update_initial_files(
            &file_loader,
            id.environment_id,
            &[initial_file(
                content_hash,
                "/collision",
                AgentFilePermissions::ReadWrite,
                content.len() as u64,
            )],
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read(collision).unwrap(), b"guest data");
    drop(update);
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn deterministic_creation_removes_existing_garbage() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();

    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    assert_eq!(filesystem.usage().await.unwrap(), None);
    let path = filesystem.path().to_path_buf();
    tokio::fs::write(path.join("garbage"), b"old")
        .await
        .unwrap();
    drop(filesystem);
    tokio::fs::create_dir_all(&path).await.unwrap();
    tokio::fs::write(path.join("garbage"), b"old")
        .await
        .unwrap();

    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    assert!(!filesystem.path().join("garbage").exists());
    filesystem.close_and_delete().await.unwrap();
    assert!(!path.exists());
}

#[test]
async fn seal_rejects_new_effects_without_waiting_for_existing_effects() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let runtime = filesystem.runtime();
    let effect = runtime.begin_effect().await.unwrap();

    filesystem.seal();
    assert!(runtime.begin_effect().await.is_err());
    assert!(filesystem.path().exists());
    drop(effect);
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn conditional_seal_is_atomic_with_effect_admission() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let effect = runtime.begin_effect().await.unwrap();

    assert!(!runtime.seal_if_no_active_effects());
    drop(effect);
    assert!(runtime.seal_if_no_active_effects());
    assert!(runtime.begin_effect().await.is_err());
}

#[test]
async fn conditional_seal_races_effect_admission_atomically() {
    for _ in 0..100 {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let effect_runtime = runtime.clone();
        let effect_barrier = Arc::clone(&barrier);
        let seal_runtime = runtime.clone();
        let seal_barrier = Arc::clone(&barrier);

        let effect = async move {
            effect_barrier.wait().await;
            effect_runtime.begin_effect().await
        };
        let seal = async move {
            seal_barrier.wait().await;
            seal_runtime.seal_if_no_active_effects()
        };
        let release = barrier.wait();
        let (effect, sealed, _) = tokio::join!(effect, seal, release);

        assert_ne!(effect.is_ok(), sealed);
    }
}

#[test]
async fn seal_rejects_admitted_effects_waiting_for_operation_lock() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let update = runtime.begin_update_effect().await.unwrap();
    let admitted = runtime.admit_effect().unwrap();
    let waiting = tokio::spawn(async move { admitted.begin().await });
    tokio::task::yield_now().await;
    assert!(!waiting.is_finished());

    runtime.seal();
    drop(update);
    assert!(waiting.await.unwrap().is_err());
}

#[test]
async fn close_waits_for_an_existing_effect_before_deleting() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let path = filesystem.path().to_path_buf();
    let effect = filesystem.runtime().begin_effect().await.unwrap();

    let close = tokio::spawn(filesystem.close_and_delete());
    tokio::task::yield_now().await;
    assert!(!close.is_finished());
    assert!(path.exists());
    drop(effect);
    close.await.unwrap().unwrap();
    assert!(!path.exists());
}

#[test]
async fn reconstruction_settlement_waits_for_existing_effects() {
    let filesystems = AgentFilesystems::new(&FilesystemStorageConfig::default()).unwrap();
    let filesystem = filesystems.create_owned_empty(&agent_id()).await.unwrap();
    let effect = filesystem.runtime().begin_effect().await.unwrap();
    {
        let settle = filesystem.settle_reconstruction();
        tokio::pin!(settle);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), &mut settle)
                .await
                .is_err()
        );
        drop(effect);
        settle.await.unwrap();
    }
    filesystem.close_and_delete().await.unwrap();
}

#[test]
async fn dropped_owner_defers_cleanup_and_retains_lifecycle_until_effects_finish() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let filesystem = filesystems.create_owned_empty(&id).await.unwrap();
    let path = filesystem.path().to_path_buf();
    let effect = filesystem.runtime().begin_effect().await.unwrap();
    drop(filesystem);

    let replacement = tokio::spawn({
        let filesystems = filesystems.clone();
        let id = id.clone();
        async move { filesystems.create_owned_empty(&id).await }
    });
    tokio::task::yield_now().await;
    assert!(!replacement.is_finished());
    assert!(path.exists());

    drop(effect);
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(5), replacement)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    replacement.close_and_delete().await.unwrap();
}

#[test]
async fn deterministic_creation_is_exclusive_for_the_full_owner_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let settings = FilesystemStorageConfig {
        deterministic_root_dir: Some(root.path().to_path_buf()),
        ..FilesystemStorageConfig::default()
    };
    let filesystems = AgentFilesystems::new(&settings).unwrap();
    let id = agent_id();
    let first = filesystems.create_owned_empty(&id).await.unwrap();
    tokio::fs::write(first.path().join("owned"), b"first")
        .await
        .unwrap();

    let second = tokio::spawn({
        let filesystems = filesystems.clone();
        let id = id.clone();
        async move { filesystems.create_owned_empty(&id).await }
    });
    tokio::task::yield_now().await;
    assert!(!second.is_finished());
    assert!(first.path().join("owned").exists());

    first.close_and_delete().await.unwrap();
    let second = second.await.unwrap().unwrap();
    assert!(!second.path().join("owned").exists());
    second.close_and_delete().await.unwrap();
}

#[test]
async fn positioned_effect_does_not_wait_for_active_append() {
    let runtime = AgentFilesystemRuntime::new_for_test();
    let append = runtime.begin_append_effect().await.unwrap();

    let positioned = runtime.begin_effect().await.unwrap();

    drop(positioned);
    drop(append);
}
