use super::*;
use crate::worker::interrupt::Interrupts;
use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt, UnloadReason, UnloadRequest};
use golem_common::model::Timestamp;
use test_r::test;

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
            establishment: InterruptEstablishment::ready(),
        }));
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
        let trap = monthly_failure::<crate::workerctx::default::Context>(
            &TrapType::Interrupt(kind),
            AgentMode::Ephemeral,
            &accepted,
            &config,
            false,
            OplogIndex::from_u64(37),
        )
        .expect("monthly exhaustion converts the accepted suspension");
        assert!(
            crate::durable_host::ephemeral_suspend_trap(
                AgentMode::Ephemeral,
                &trap,
                OplogIndex::from_u64(37)
            )
            .is_none(),
            "the active-suspend fallback must not replace the monthly error"
        );
        assert_eq!(trap.as_golem_error(""), Some(expected));
    }
}

#[test]
fn monthly_policy_outage_preserves_its_reason_in_ephemeral_suspend_failure() {
    let config = GolemConfig::default();
    let retry_from = OplogIndex::from_u64(37);
    let suspend = TrapType::Interrupt(InterruptKind::Suspend(Timestamp::from(50_u64)));
    let converted = monthly_failure::<crate::workerctx::default::Context>(
        &suspend,
        AgentMode::Ephemeral,
        &None,
        &config,
        true,
        retry_from,
    )
    .expect("an active ephemeral policy outage is terminal");
    assert!(matches!(&converted,
        TrapType::Error {
            error: AgentError::EphemeralCannotSuspend(EphemeralCannotSuspendError { reason }),
            retry_from: anchor,
            semantic_trap_retry_override: None,
        } if reason.contains("Monthly enforcement unavailable") && *anchor == retry_from));
    assert!(
        crate::durable_host::ephemeral_suspend_trap(AgentMode::Ephemeral, &converted, retry_from)
            .is_none(),
        "the generic suspension conversion must preserve the monthly reason"
    );
    for (mode, trap, unavailable) in [
        (AgentMode::Durable, suspend.clone(), true),
        (AgentMode::Ephemeral, suspend, false),
        (
            AgentMode::Ephemeral,
            TrapType::Interrupt(InterruptKind::Interrupt(Timestamp::from(51_u64))),
            true,
        ),
        (
            AgentMode::Ephemeral,
            TrapType::Interrupt(InterruptKind::ShardLost),
            true,
        ),
        (AgentMode::Ephemeral, TrapType::Exit, true),
    ] {
        assert!(
            monthly_failure::<crate::workerctx::default::Context>(
                &trap,
                mode,
                &None,
                &config,
                unavailable,
                retry_from,
            )
            .is_none()
        );
    }
}
