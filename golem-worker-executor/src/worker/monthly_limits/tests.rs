use super::*;
use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
use crate::services::golem_config::ResourceUsageMeteringConfig;
use crate::services::linear_memory::LinearMemoryTracker;
use crate::services::resource_limits::AtomicResourceEntry;
use crate::services::resource_usage_metering::ResourceUsageAccount;
use golem_common::model::AgentId;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentMode;
use golem_common::model::component::ComponentId;
use std::sync::atomic::{AtomicBool, Ordering};
use test_r::{test, timeout};

async fn unmetered_window() -> (ExecutionWindow, Arc<AtomicBool>) {
    let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
    let memory = LinearMemoryTracker::new_with_metering(
        0,
        0,
        AgentMode::Durable,
        false,
        entry.clone(),
        Arc::new(Mutex::new(MemoryGrant::inert(0))),
        false,
    );
    let account = ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone());
    let meter = resource_usage_metering::create_unbound_meter(
        ResourceUsageMeteringConfig {
            compute: false,
            memory: false,
            filesystem: false,
        },
        account,
    );
    let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
    let account_id = AccountId(Uuid::new_v4());
    scheduler.register_account(account_id, entry).await;
    let permit = scheduler
        .acquire(
            account_id,
            AgentId {
                component_id: ComponentId(Uuid::new_v4()),
                agent_id: "unmetered-close".to_string(),
            },
        )
        .await;
    let mut window = resource_usage_metering::open_window(&meter, permit)
        .await
        .unwrap();
    let held = Arc::new(AtomicBool::new(false));
    window.track_permit_for_test(held.clone());
    assert!(held.load(Ordering::Acquire));
    (ExecutionWindow::unmonitored(window), held)
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_spawns_no_task_and_completes_its_seal() {
    let (window, held) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert!(!held.load(Ordering::Acquire));
    assert!(closing.now_or_never().unwrap().is_ok());
    assert!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .is_ok()
    );
}

#[test]
#[timeout("5s")]
async fn all_off_pending_close_retains_task_and_permit_after_observer_drop() {
    let (window, held) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let (release, wait) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { wait.await.unwrap() }.boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    let successor = progress.lock().unwrap().release.clone().unwrap();
    assert_eq!(progress.lock().unwrap().window_close_tasks, 1);
    drop(closing);
    assert!(held.load(Ordering::Acquire));
    assert!(successor.clone().now_or_never().is_none());
    release.send(Ok(())).unwrap();
    successor.await.unwrap();
    assert!(!held.load(Ordering::Acquire));
}

#[test]
#[timeout("5s")]
async fn stopping_instance_reports_real_permit_release_before_state_transition() {
    let (window, held) = unmetered_window().await;
    let instance = WorkerInstance::Stopping(super::super::StoppingWorker {
        notify: golem_common::one_shot::OneShotEvent::new(),
        final_state: super::super::FinalWorkerState::Unloaded {
            startup_failure: None,
        },
        pending_live_invocations: super::super::PendingLiveInvocationDisposition::Preserve,
        concurrent_agent_permit_held: held.clone(),
    });
    let progress = window.stop_progress.clone();
    let (release, wait) = tokio::sync::oneshot::channel();
    progress
        .lock()
        .unwrap()
        .tasks
        .push(async move { wait.await.unwrap() }.boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    let successor = progress.lock().unwrap().release.clone().unwrap();
    assert!(held.load(Ordering::Acquire));
    assert!(instance.concurrent_agent_permit_is_held());
    assert!(successor.clone().now_or_never().is_none());
    release.send(Ok(())).unwrap();
    successor.await.unwrap();
    closing.await.unwrap();
    assert!(matches!(instance, WorkerInstance::Stopping(_)));
    assert!(!held.load(Ordering::Acquire));
    assert!(!instance.concurrent_agent_permit_is_held());
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_converts_panic_and_keeps_the_failed_seal() {
    let (window, held) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    progress.lock().unwrap().tasks.push(
        async { panic!("injected unmetered stop cleanup panic") }
            .boxed()
            .shared(),
    );
    let error = window
        .close(Instant::now() + Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected unmetered stop cleanup panic")
    );
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert!(!held.load(Ordering::Acquire));
    assert!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .is_err()
    );
}

#[test]
#[timeout("5s")]
async fn all_off_ready_close_retains_prior_cleanup_error() {
    let (window, held) = unmetered_window().await;
    let progress = window.stop_progress.clone();
    let error = WorkerExecutorError::runtime("prior stop cleanup failed");
    progress
        .lock()
        .unwrap()
        .tasks
        .push(std::future::ready(Err(error.clone())).boxed().shared());
    let closing = window.close(Instant::now() + Duration::from_secs(1));
    assert_eq!(progress.lock().unwrap().window_close_tasks, 0);
    assert_eq!(closing.await.unwrap_err(), error);
    assert!(!held.load(Ordering::Acquire));
    assert_eq!(
        progress
            .lock()
            .unwrap()
            .release
            .clone()
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err(),
        error
    );
}
