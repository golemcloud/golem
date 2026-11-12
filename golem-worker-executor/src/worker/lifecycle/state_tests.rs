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
        matches!(completed.state(), WorkerInstance::Unloaded { startup_failure: Some(error) }
        if error.to_string().contains("startup stopped"))
    );
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
    assert!(matches!(completed.state(), WorkerInstance::Unloaded { .. }));
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
    assert!(matches!(phase, WorkerInstance::CleanupFailed(error)
        if error.to_string().contains("cleanup failed")));
    assert_eq!(pending, PendingLiveInvocationDisposition::Fail);
    let mut joined_wait = Box::pin(joined.wait());
    let mut original_wait = Box::pin(notify.wait());
    assert!(futures::poll!(joined_wait.as_mut()).is_pending());
    assert!(futures::poll!(original_wait.as_mut()).is_pending());
    completed.set();
    assert!(futures::poll!(joined_wait.as_mut()).is_ready());
    assert!(futures::poll!(original_wait.as_mut()).is_ready());
}
