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
use test_r::{test, timeout};

#[test]
#[timeout("5s")]
async fn waiting_stop_retains_admission_until_waiter_resolution_finishes() {
    let (started, ready) = oneshot::channel();
    let (exited, mut exit) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _exited = exited;
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    ready.await.unwrap();
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::WaitingForPermit(WaitingWorker {
            handle,
            start_attempt: Uuid::new_v4(),
        }),
    };
    let final_state = FinalWorkerState::Unloaded {
        startup_failure: Some(WorkerExecutorError::runtime("startup stopped")),
    };
    let Some(StopPhase::WaitingForPermit(waiting)) =
        lifecycle.begin_stop(StopOrigin::External, &final_state)
    else {
        panic!("expected permit admission stop");
    };
    assert!(exit.try_recv().unwrap().is_none());
    let completed = waiting.complete(final_state);
    tokio::task::yield_now().await;
    assert!(
        matches!(completed.final_state(), FinalWorkerState::Unloaded { startup_failure: Some(error) }
        if error.to_string().contains("startup stopped"))
    );
    assert!(matches!(
        lifecycle.state(),
        WorkerInstance::Unloaded {
            startup_failure: Some(_)
        }
    ));
    assert!(exit.try_recv().unwrap().is_none());
    drop(completed);
    assert!(exit.await.is_err());
}

#[test]
#[timeout("5s")]
async fn loop_stop_retains_running_resources_until_waiter_resolution_finishes() {
    let mut phase = crate::worker::lifecycle_tests::running_lifecycle(
        Arc::new(Interrupts::default()),
        Uuid::new_v4(),
        false,
    );
    let (sender, mut commands) = tokio::sync::mpsc::unbounded_channel();
    let WorkerInstance::Running(running) = &mut phase else {
        panic!("expected running fixture");
    };
    running.sender = sender;
    let mut lifecycle = WorkerLifecycle { phase };
    let final_state = FinalWorkerState::Unloaded {
        startup_failure: None,
    };
    let Some(StopPhase::Running(RunningStop::InvocationLoop(running))) =
        lifecycle.begin_stop(StopOrigin::InvocationLoop, &final_state)
    else {
        panic!("expected invocation-loop stop");
    };
    assert!(!commands.is_closed());
    let completed = running.complete(final_state);
    tokio::task::yield_now().await;
    assert!(matches!(
        completed.final_state(),
        FinalWorkerState::Unloaded { .. }
    ));
    assert!(matches!(lifecycle.state(), WorkerInstance::Unloaded { .. }));
    assert!(!commands.is_closed());
    drop(completed);
    assert!(commands.recv().await.is_none());
}

#[test]
async fn cancelled_stop_retains_cleanup_failure_inside_and_outside_deletion() {
    for origin in [StopOrigin::InvocationLoop, StopOrigin::External] {
        for deleting in [false, true] {
            let mut lifecycle = WorkerLifecycle {
                phase: WorkerInstance::Stopping(StoppingWorker {
                    notify: OneShotEvent::new(),
                    final_state: FinalWorkerState::CleanupFailed(WorkerExecutorError::runtime(
                        "original cleanup failure",
                    )),
                    pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
                }),
            };
            let _deletion = deleting.then(|| lifecycle.claim_deletion());
            let mut stop = Box::pin(async {
                let _stop = lifecycle.begin_stop(
                    origin,
                    &FinalWorkerState::Unloaded {
                        startup_failure: None,
                    },
                );
                std::future::pending::<()>().await;
            });
            assert!(futures::poll!(stop.as_mut()).is_pending());
            drop(stop);
            assert!(matches!(lifecycle.state(), WorkerInstance::Deleting(_)) == deleting);
            assert!(
                lifecycle
                    .retained_cleanup_failure()
                    .unwrap()
                    .to_string()
                    .contains("original cleanup failure")
            );
        }
    }
}

#[test]
async fn overlapping_stops_retain_failure_disposition_and_shared_notification() {
    let notify = OneShotEvent::new();
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::Stopping(StoppingWorker {
            notify: notify.clone(),
            final_state: FinalWorkerState::Deleting,
            pending_live_invocations: PendingLiveInvocationDisposition::Preserve,
        }),
    };
    let failure = FinalWorkerState::CleanupFailed(WorkerExecutorError::runtime("cleanup failed"));
    let Some(StopPhase::Stopping(StoppingStop::External(mut stopping))) =
        lifecycle.begin_stop(StopOrigin::External, &failure)
    else {
        panic!("expected external overlapping stop");
    };
    stopping.merge_request(failure, PendingLiveInvocationDisposition::Fail);
    let StopResult::AlreadyStopping { notify: joined } = stopping.join() else {
        panic!("expected shared stop notification");
    };
    let final_state = FinalWorkerState::Unloaded {
        startup_failure: None,
    };
    let Some(StopPhase::Stopping(StoppingStop::InvocationLoop(stopping))) =
        lifecycle.begin_stop(StopOrigin::InvocationLoop, &final_state)
    else {
        panic!("expected loop completion of overlapping stop");
    };
    let (phase, completed, pending) = stopping.complete(final_state);
    assert!(matches!(phase, FinalWorkerState::CleanupFailed(error)
        if error.to_string().contains("cleanup failed")));
    assert!(matches!(
        lifecycle.state(),
        WorkerInstance::CleanupFailed(_)
    ));
    assert_eq!(pending, PendingLiveInvocationDisposition::Fail);
    let mut joined_wait = Box::pin(joined.wait());
    let mut original_wait = Box::pin(notify.wait());
    assert!(futures::poll!(joined_wait.as_mut()).is_pending());
    assert!(futures::poll!(original_wait.as_mut()).is_pending());
    completed.set();
    assert!(futures::poll!(joined_wait.as_mut()).is_ready());
    assert!(futures::poll!(original_wait.as_mut()).is_ready());
}

#[test]
fn deletion_stop_leaves_a_stopped_runtime_without_a_second_stop() {
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::Unloaded {
            startup_failure: None,
        },
    };
    let Some((DeleteOutcome::Started(_), Some(_sender))) = lifecycle.claim_deletion() else {
        panic!("expected a started deletion");
    };
    let final_state = FinalWorkerState::Deleting;
    let Some(StopPhase::Unloaded(unloaded)) =
        lifecycle.begin_stop(StopOrigin::External, &final_state)
    else {
        panic!("expected the retained unloaded runtime");
    };
    assert!(matches!(
        unloaded.complete(final_state),
        FinalWorkerState::Deleting
    ));

    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::Deleting)
    ));
    assert!(lifecycle.ensure_not_deleting().is_err());
    assert!(
        lifecycle
            .begin_stop(
                StopOrigin::External,
                &FinalWorkerState::Unloaded {
                    startup_failure: None,
                },
            )
            .is_none()
    );
    assert!(matches!(
        lifecycle.claim_deletion(),
        Some((DeleteOutcome::AlreadyDeleting(_), None))
    ));
}

#[test]
fn deletion_retry_retains_cleanup_failure_and_completed_stages() {
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::Stopping(StoppingWorker {
            notify: OneShotEvent::new(),
            final_state: FinalWorkerState::Deleting,
            pending_live_invocations: PendingLiveInvocationDisposition::Fail,
        }),
    };
    let Some((DeleteOutcome::Started(_), Some(first_attempt))) = lifecycle.claim_deletion() else {
        panic!("expected a started deletion");
    };
    lifecycle.complete_deletion_stage(WorkerDeletionStage::BarriersClosed);
    let failure = WorkerExecutorError::runtime("cleanup failed");
    let final_state = FinalWorkerState::CleanupFailed(failure.clone());
    let Some(StopPhase::Stopping(StoppingStop::InvocationLoop(stopping))) =
        lifecycle.begin_stop(StopOrigin::InvocationLoop, &final_state)
    else {
        panic!("expected the retained stopping runtime");
    };
    let (completed, _notify, _pending) = stopping.complete(final_state);
    assert!(matches!(completed, FinalWorkerState::CleanupFailed(_)));
    assert_eq!(lifecycle.cleanup_failure(), Some(&failure));
    assert!(lifecycle.ensure_not_deleting().is_err());

    first_attempt.send(Err(failure.clone())).unwrap();
    let Some((DeleteOutcome::Started(_), Some(_retry))) = lifecycle.claim_deletion() else {
        panic!("expected a retry after the failed attempt");
    };
    assert!(matches!(
        lifecycle.state(),
        WorkerInstance::Deleting(deleting)
            if deleting.completed_stage == WorkerDeletionStage::BarriersClosed
    ));
    assert_eq!(lifecycle.cleanup_failure(), Some(&failure));

    lifecycle.deletion_cleanup_completed();
    assert!(lifecycle.cleanup_failure().is_none());
    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::Deleting)
    ));
}

#[test]
fn deletion_claims_nothing_before_initialization_completes() {
    let mut lifecycle = WorkerLifecycle::unresolved();
    assert!(lifecycle.claim_deletion().is_none());
    assert!(matches!(lifecycle.state(), WorkerInstance::Unresolved));
    assert!(lifecycle.ensure_not_deleting().is_ok());
}

#[test]
fn ephemeral_startup_failure_stays_inactive_while_durable_failure_retries() {
    for (agent_mode, retries) in [(AgentMode::Durable, true), (AgentMode::Ephemeral, false)] {
        let mut lifecycle = WorkerLifecycle {
            phase: WorkerInstance::Unloaded {
                startup_failure: None,
            },
        };
        let LifecycleTransition::Unloaded(unloaded) = lifecycle.transition() else {
            panic!("expected unloaded fixture");
        };
        unloaded.startup_failed(StartupFailure::new(
            agent_mode,
            WorkerExecutorError::runtime("start failed"),
        ));
        assert!(
            lifecycle
                .startup_failure()
                .is_some_and(|error| error.to_string().contains("start failed"))
        );
        assert_eq!(
            matches!(lifecycle.transition(), LifecycleTransition::Unloaded(_)),
            retries
        );

        // An ordinary stop clears the failure of a durable agent but never makes an
        // ephemeral agent startable again, even when the stop is cancelled.
        let unloaded = FinalWorkerState::Unloaded {
            startup_failure: None,
        };
        drop(lifecycle.begin_stop(StopOrigin::External, &unloaded));
        assert_eq!(lifecycle.startup_failure().is_none(), retries);
        match lifecycle.begin_stop(StopOrigin::External, &unloaded) {
            Some(StopPhase::Unloaded(stop)) if retries => {
                stop.complete(unloaded);
            }
            Some(StopPhase::Inactive(stop)) if !retries => {
                stop.complete(unloaded);
            }
            _ => panic!("unexpected stop phase for {agent_mode:?}"),
        }
        assert_eq!(
            matches!(lifecycle.transition(), LifecycleTransition::Inactive(_)),
            !retries
        );
    }
}

#[test]
fn deletion_retains_inactivity_until_its_stop_and_cleanup_failure_replaces_it() {
    let inactive = || WorkerInstance::Inactive(WorkerExecutorError::runtime("inactive"));
    let unloaded = FinalWorkerState::Unloaded {
        startup_failure: None,
    };

    let mut lifecycle = WorkerLifecycle { phase: inactive() };
    assert!(matches!(
        lifecycle.claim_deletion(),
        Some((DeleteOutcome::Started(_), Some(_)))
    ));
    drop(lifecycle.begin_stop(StopOrigin::External, &unloaded));
    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::Inactive(_))
    ));
    let Some(StopPhase::Inactive(stop)) =
        lifecycle.begin_stop(StopOrigin::External, &FinalWorkerState::Deleting)
    else {
        panic!("expected the retained inactive runtime to stop");
    };
    assert!(matches!(
        stop.complete(FinalWorkerState::Deleting),
        FinalWorkerState::Deleting
    ));
    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::Deleting)
    ));

    let mut lifecycle = WorkerLifecycle { phase: inactive() };
    let failure = FinalWorkerState::CleanupFailed(WorkerExecutorError::runtime("cleanup failed"));
    let Some(StopPhase::Inactive(stop)) = lifecycle.begin_stop(StopOrigin::External, &failure)
    else {
        panic!("expected inactive stop");
    };
    stop.complete(failure);
    assert!(
        lifecycle
            .cleanup_failure()
            .is_some_and(|error| error.to_string().contains("cleanup failed"))
    );
}

#[test]
#[timeout("5s")]
async fn deletion_retains_waiting_admission_until_its_stop_completes() {
    let (exited, mut exit) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _exited = exited;
        std::future::pending::<()>().await;
    });
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::WaitingForPermit(WaitingWorker {
            handle,
            start_attempt: Uuid::new_v4(),
        }),
    };
    let Some((DeleteOutcome::Started(_), Some(_sender))) = lifecycle.claim_deletion() else {
        panic!("expected a started deletion");
    };
    assert!(lifecycle.runtime_active());
    assert!(lifecycle.ensure_not_deleting().is_err());

    let final_state = FinalWorkerState::Deleting;
    let Some(StopPhase::WaitingForPermit(waiting)) =
        lifecycle.begin_stop(StopOrigin::External, &final_state)
    else {
        panic!("expected the retained permit admission");
    };
    assert!(exit.try_recv().unwrap().is_none());
    let completed = waiting.complete(final_state);
    assert!(matches!(
        completed.final_state(),
        FinalWorkerState::Deleting
    ));
    assert!(!lifecycle.runtime_active());
    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::Deleting)
    ));
    tokio::task::yield_now().await;
    assert!(exit.try_recv().unwrap().is_none());
    drop(completed);
    assert!(exit.await.is_err());
}

#[test]
fn deletion_retry_retains_a_cleanup_failure_from_before_the_claim() {
    let failure = WorkerExecutorError::runtime("cleanup failed before deletion");
    let mut lifecycle = WorkerLifecycle {
        phase: WorkerInstance::CleanupFailed(failure.clone()),
    };
    let Some((DeleteOutcome::Started(_), Some(first_attempt))) = lifecycle.claim_deletion() else {
        panic!("expected a started deletion");
    };
    assert_eq!(lifecycle.cleanup_failure(), Some(&failure));
    lifecycle.complete_deletion_stage(WorkerDeletionStage::BarriersClosed);

    let final_state = FinalWorkerState::Deleting;
    let Some(StopPhase::CleanupFailed(failed)) =
        lifecycle.begin_stop(StopOrigin::External, &final_state)
    else {
        panic!("expected the retained cleanup failure");
    };
    failed.retain();
    assert_eq!(lifecycle.cleanup_failure(), Some(&failure));
    assert!(matches!(
        lifecycle.stop_outcome(),
        Some(FinalWorkerState::CleanupFailed(error)) if error == failure
    ));

    first_attempt.send(Err(failure.clone())).unwrap();
    let Some((DeleteOutcome::Started(_), Some(_retry))) = lifecycle.claim_deletion() else {
        panic!("expected a retry after the failed attempt");
    };
    assert!(matches!(
        lifecycle.state(),
        WorkerInstance::Deleting(deleting)
            if deleting.completed_stage == WorkerDeletionStage::BarriersClosed
    ));
    assert_eq!(lifecycle.cleanup_failure(), Some(&failure));

    lifecycle.deletion_cleanup_completed();
    assert!(lifecycle.cleanup_failure().is_none());
    assert!(
        lifecycle
            .begin_stop(StopOrigin::External, &FinalWorkerState::Deleting)
            .is_none()
    );
}
