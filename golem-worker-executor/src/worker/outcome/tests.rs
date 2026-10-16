use super::*;
use crate::worker::interrupt::Interrupts;
use crate::worker::{PendingWorkerInterrupt, UnloadReason, UnloadRequest};
use golem_common::model::Timestamp;
use test_r::test;

#[test]
fn marker_generation_capture_requires_started_matching_attempt() {
    let attempt = Uuid::new_v4();
    for started in [false, true] {
        let lifecycle = crate::worker::stop::tests::running_lifecycle(
            std::sync::Arc::default(),
            attempt,
            started,
        );
        assert_eq!(
            started_marker_generation(&lifecycle, attempt, 7),
            started.then_some(7)
        );
        assert_eq!(
            started_marker_generation(&lifecycle, Uuid::new_v4(), 7),
            None
        );
    }
    assert_eq!(
        started_marker_generation(
            &WorkerInstance::Unloaded {
                startup_failure: None,
            },
            attempt,
            7,
        ),
        None
    );
}

#[test]
fn only_fifo_suspended_status_of_the_established_owner_reuses_a_suspend_marker() {
    let kind = InterruptKind::Suspend(Timestamp::from(60_u64));
    for established in [None, Some(6), Some(7), Some(8)] {
        for status in [
            AgentStatus::Idle,
            AgentStatus::Running,
            AgentStatus::Retrying,
            AgentStatus::Suspended,
        ] {
            let marker = unloaded_lifecycle_marker(kind, status, 7, established);
            if established == Some(7) && status == AgentStatus::Suspended {
                assert!(marker.is_none());
            } else {
                assert!(matches!(marker, Some(OplogEntry::Suspend { .. })));
            }
        }
    }
}

#[test]
fn pre_owner_attempt_does_not_reuse_the_previous_suspend_with_an_unchanged_counter() {
    let old_attempt = Uuid::new_v4();
    let new_attempt = Uuid::new_v4();
    let previous =
        crate::worker::stop::tests::running_lifecycle(std::sync::Arc::default(), old_attempt, true);
    let next = crate::worker::stop::tests::running_lifecycle(
        std::sync::Arc::default(),
        new_attempt,
        false,
    );
    assert_eq!(
        started_marker_generation(&previous, old_attempt, 7),
        Some(7)
    );
    let established = started_marker_generation(&next, new_attempt, 7);
    assert_eq!(established, None);
    assert!(matches!(
        unloaded_lifecycle_marker(
            InterruptKind::Suspend(Timestamp::from(61_u64)),
            AgentStatus::Suspended,
            7,
            established,
        ),
        Some(OplogEntry::Suspend { .. })
    ));
}

#[test]
fn unloaded_marker_does_not_create_results_or_markers_for_restart_jump_or_lost_shard() {
    for kind in [
        InterruptKind::Restart,
        InterruptKind::Jump,
        InterruptKind::ShardLost,
    ] {
        assert!(unloaded_lifecycle_marker(kind, AgentStatus::Running, 7, Some(7)).is_none());
    }
    assert!(matches!(
        unloaded_lifecycle_marker(
            InterruptKind::Interrupt(Timestamp::from(62_u64)),
            AgentStatus::Suspended,
            7,
            Some(7),
        ),
        Some(OplogEntry::Interrupted { .. })
    ));
}

#[test]
async fn a_stop_after_the_ordinary_terminal_check_remains_for_the_invocation_loop() {
    let interrupts = Interrupts::default();
    assert!(interrupts.claim_pending_terminal().await.is_none());
    let kind = InterruptKind::Suspend(Timestamp::from(63_u64));
    {
        let mut guard = interrupts.lock().await;
        assert!(guard.queue(PendingWorkerInterrupt {
            kind,
            reacquire_permits: false,
            unload_request: UnloadRequest::ordinary(UnloadReason::Suspend),
        }));
        guard.freeze();
        guard.publish();
    }
    assert_eq!(interrupts.take().await.unwrap().kind, kind);
    assert!(interrupts.take().await.is_none());
}

#[test]
fn monthly_mapping_requires_exact_accepted_kind_and_ephemeral_mode() {
    let config = GolemConfig::default();
    let kind = InterruptKind::Suspend(Timestamp::from(50_u64));
    let other = InterruptKind::Suspend(Timestamp::from(51_u64));
    for exhaustion in [
        MonthlyResourceExhaustion::Compute,
        MonthlyResourceExhaustion::Memory,
        MonthlyResourceExhaustion::DurableStorage,
        MonthlyResourceExhaustion::EphemeralStorage,
    ] {
        let accepted = Some((kind, exhaustion));
        assert!(monthly_interrupt_error(kind, AgentMode::Durable, &accepted, &config).is_none());
        assert!(monthly_interrupt_error(other, AgentMode::Ephemeral, &accepted, &config).is_none());
        let actual =
            monthly_interrupt_error(kind, AgentMode::Ephemeral, &accepted, &config).unwrap();
        let expected = crate::worker::invocation_loop::monthly_resource_exhausted_invocation_error(
            &config, exhaustion,
        );
        assert_eq!(actual, expected);
    }
}
