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
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::{test, timeout};
use tokio::sync::oneshot;

struct SignalOnDrop(Option<oneshot::Sender<()>>);

impl Drop for SignalOnDrop {
    fn drop(&mut self) {
        let _ = self.0.take().unwrap().send(());
    }
}

#[test]
#[timeout("30s")]
async fn aborted_admission_joins_without_waiting_for_replay_progress() {
    let worker = WorkerTasks::default();
    let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
    let spawner = owner.spawner();
    let (dropped, observed_drop) = oneshot::channel();
    let guard = SignalOnDrop(Some(dropped));
    spawner
        .spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
            Ok(())
        })
        .unwrap();
    owner.abort_and_join().await;
    observed_drop.await.unwrap();
    assert!(spawner.spawn(async { Ok(()) }).is_err());
    drop(owner);
    worker.stop_and_wait().await.unwrap();
}

#[test]
#[timeout("30s")]
async fn reusable_drain_joins_registered_work_before_first_poll_and_after_wakeup() {
    let worker = WorkerTasks::default();
    let owner = NativeReconstructionTasks::new(&worker).unwrap();
    let monitor = owner.monitor();
    for _ in 0..2 {
        let (release, released) = oneshot::channel();
        let (handoff, received) = oneshot::channel();
        owner
            .spawner()
            .spawn(async move {
                released.await.unwrap();
                handoff.send(()).unwrap();
                Ok(())
            })
            .unwrap();
        let drain = monitor.drain();
        tokio::pin!(drain);
        assert!(futures::poll!(drain.as_mut()).is_pending());
        release.send(()).unwrap();
        drain.await.unwrap();
        received.await.unwrap();
    }
    owner
        .spawner()
        .spawn(async { Err(WorkerExecutorError::runtime("admission interrupted")) })
        .unwrap();
    let failure = monitor.drain().await.unwrap_err();
    assert_eq!(
        failure,
        WorkerExecutorError::runtime("admission interrupted")
    );
    assert_eq!(monitor.drain().await.unwrap_err(), failure);
    drop(owner);
    worker.stop_and_wait().await.unwrap();
}

#[test]
#[timeout("30s")]
async fn cancelled_operation_does_not_cancel_body_reconstruction() {
    let worker = WorkerTasks::default();
    let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
    let spawner = owner.spawner();
    let (release, released) = oneshot::channel();
    let (receipt, operation) = oneshot::channel();
    let restored = Arc::new(AtomicBool::new(false));
    let effect = restored.clone();
    spawner
        .spawn(async move {
            released.await.unwrap();
            effect.store(true, Ordering::SeqCst);
            let _ = receipt.send(());
            Ok(())
        })
        .unwrap();
    drop(operation);
    {
        let drain = owner.seal_and_drain();
        tokio::pin!(drain);
        assert!(futures::poll!(drain.as_mut()).is_pending());
        assert!(spawner.spawn(async { Ok(()) }).is_err());
        assert!(!restored.load(Ordering::SeqCst));
        release.send(()).unwrap();
        drain.await.unwrap();
    }
    assert!(restored.load(Ordering::SeqCst));
    drop(owner);
    worker.stop_and_wait().await.unwrap();
}

#[test]
#[timeout("30s")]
async fn body_failure_reporting_does_not_abort_or_hide_reconstruction() {
    for fail_restore in [false, true] {
        let worker = WorkerTasks::default();
        let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
        let spawner = owner.spawner();
        let monitor = owner.monitor();
        let (release, released) = oneshot::channel();
        let restored = Arc::new(AtomicBool::new(false));
        let effect = restored.clone();
        spawner
            .spawn(async move {
                released.await.unwrap();
                effect.store(true, Ordering::SeqCst);
                if fail_restore {
                    Err(WorkerExecutorError::runtime("restore failed"))
                } else {
                    Ok(())
                }
            })
            .unwrap();
        let admission = crate::durable_host::tool_attachment_live_admission_rejected_error();
        let reported = admission.clone();
        spawner
            .report_body_failure(async move { reported })
            .unwrap();
        assert_eq!(monitor.wait_for_body_failure().await, admission);
        assert!(monitor.take_body_failure().is_none());
        assert!(!restored.load(Ordering::SeqCst));
        release.send(()).unwrap();
        let result = owner.seal_and_drain().await;
        assert!(restored.load(Ordering::SeqCst));
        if fail_restore {
            assert_eq!(
                result.unwrap_err(),
                WorkerExecutorError::runtime("restore failed")
            );
        } else {
            result.unwrap();
        }
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }

    let worker = WorkerTasks::default();
    let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
    let monitor = owner.monitor();
    let (release, released) = oneshot::channel();
    owner
        .spawner()
        .report_body_failure(async move {
            released.await.unwrap();
            WorkerExecutorError::runtime("late body failure")
        })
        .unwrap();
    {
        let drain = owner.seal_and_drain();
        tokio::pin!(drain);
        assert!(futures::poll!(drain.as_mut()).is_pending());
        release.send(()).unwrap();
        drain.await.unwrap();
    }
    assert_eq!(
        monitor.take_body_failure(),
        Some(WorkerExecutorError::runtime("late body failure"))
    );
    drop(owner);
    worker.stop_and_wait().await.unwrap();
}

#[test]
#[timeout("30s")]
async fn active_monitor_retains_failure_and_panic_without_operation_observer() {
    for panic in [false, true] {
        let worker = WorkerTasks::default();
        let mut owner = NativeReconstructionTasks::new(&worker).unwrap();
        let spawner = owner.spawner();
        let monitor = owner.monitor();
        let (parked, parked_rx) = oneshot::channel();
        let failure = tokio::spawn(async move {
            let failure = monitor.wait_for_failure();
            tokio::pin!(failure);
            let mut parked = Some(parked);
            poll_fn(|cx| {
                let result = failure.as_mut().poll(cx);
                if result.is_pending()
                    && let Some(parked) = parked.take()
                {
                    let _ = parked.send(());
                }
                result
            })
            .await
        });
        parked_rx.await.unwrap();
        let (receipt, operation) = oneshot::channel::<()>();
        drop(operation);
        spawner
            .spawn(async move {
                drop(receipt);
                if panic {
                    panic!("injected reconstruction panic");
                }
                Err(WorkerExecutorError::runtime(
                    "injected reconstruction failure",
                ))
            })
            .unwrap();
        let error = failure.await.unwrap();
        assert!(error.to_string().contains(if panic {
            "injected reconstruction panic"
        } else {
            "injected reconstruction failure"
        }));
        assert!(spawner.spawn(async { Ok(()) }).is_err());
        assert_eq!(owner.seal_and_drain().await.unwrap_err(), error);
        drop(owner);
        worker.stop_and_wait().await.unwrap();
    }
}

#[test]
#[timeout("30s")]
async fn body_teardown_revokes_escaped_handles_and_joins_owned_cleanup() {
    let worker = WorkerTasks::default();
    let owner = NativeReconstructionTasks::new(&worker).unwrap();
    let spawner = owner.spawner();
    let monitor = owner.monitor();
    let (started, started_rx) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let (finished, finished_rx) = oneshot::channel();
    let (child_dropped, child_dropped_rx) = oneshot::channel();
    let child_worker = worker.clone();
    spawner
        .spawn(async move {
            let _dropped = SignalOnDrop(Some(child_dropped));
            let task = tokio::spawn(async move {
                started.send(()).unwrap();
                released.await.unwrap();
                finished.send(()).unwrap();
            });
            child_worker.finish_on_drop(task).await.unwrap();
            Ok(())
        })
        .unwrap();
    started_rx.await.unwrap();
    let failure = monitor.wait_for_failure();
    tokio::pin!(failure);
    assert!(futures::poll!(failure.as_mut()).is_pending());
    let stop = worker.stop_and_wait();
    tokio::pin!(stop);
    assert!(futures::poll!(stop.as_mut()).is_pending());
    assert!(NativeReconstructionTasks::new(&worker).is_err());
    drop(owner);
    assert!(spawner.spawn(async { Ok(()) }).is_err());
    let _closed = failure.await;
    child_dropped_rx.await.unwrap();
    assert!(futures::poll!(stop.as_mut()).is_pending());
    release.send(()).unwrap();
    stop.await.unwrap();
    finished_rx.await.unwrap();
    assert!(NativeReconstructionTasks::new(&worker).is_err());
}
