use super::*;
use crate::worker::interrupt::Interrupts;
use crate::worker::{InterruptEstablishment, PendingWorkerInterrupt, UnloadReason, UnloadRequest};
use golem_common::model::Timestamp;
use test_r::test;

#[test]
fn invocation_rejection_lookup_preserves_the_typed_error() {
    let key = InvocationId(uuid::Uuid::new_v4());
    let status = AgentStatusRecord::default();
    for (error, expected) in [
        (
            AgentError::InvalidRequest("method not found".to_string()),
            WorkerExecutorError::invalid_request("method not found"),
        ),
        (
            AgentError::PermissionDenied("not authorized".to_string()),
            WorkerExecutorError::permission_denied("not authorized"),
        ),
    ] {
        let actual = lookup_result_from_cached_result(
            &status,
            &key,
            InvocationResult::Cached {
                result: Err(FailedInvocationResult::Trap {
                    trap_type: TrapType::Error {
                        error,
                        retry_from: OplogIndex::INITIAL,
                        semantic_trap_retry_override: None,
                        provenance: crate::model::FailureProvenance::Host,
                    },
                    stderr: String::new(),
                }),
            },
        );
        assert!(matches!(actual, LookupResult::Complete(Err(error)) if error == expected));
    }
    assert!(matches!(
        lookup_result_from_cached_result(&status, &key, InvocationResult::Cancelled),
        LookupResult::Complete(Err(WorkerExecutorError::InvocationCancelled))
    ));
}

fn initialization_position(
    terminal: u64,
    end: golem_common::model::InvocationEnd,
    operation: golem_common::model::InvocationOperation,
) -> golem_common::model::InvocationResultPosition {
    golem_common::model::InvocationResultPosition {
        terminal: OplogIndex::from_u64(terminal),
        request: Some(OplogIndex::from_u64(terminal - 1)),
        end,
        operation: Some(operation),
        accepted_at: Some(Timestamp::from(1_u64)),
        changed_at: Timestamp::from(2_u64),
    }
}

/// The status answers for a completed initialization whose terminal entry is below the retained
/// start, because a read of that entry is expired. A terminal entry at or after the start, a
/// result that did not complete and a result of another operation are read from the oplog.
#[test]
fn only_a_completed_initialization_below_the_start_is_answered_from_the_status() {
    use crate::services::oplog::retention::test_oplog::at;
    use golem_common::model::{InvocationEnd, InvocationOperation};

    let completed = initialization_position(
        5,
        InvocationEnd::Completed,
        InvocationOperation::Constructor,
    );
    assert!(initialization_finished_below_start(at(6), Some(&completed)));
    assert!(initialization_finished_below_start(at(9), Some(&completed)));
    assert!(!initialization_finished_below_start(
        at(5),
        Some(&completed)
    ));
    assert!(!initialization_finished_below_start(
        at(1),
        Some(&completed)
    ));
    assert!(!initialization_finished_below_start(at(6), None));
    [
        InvocationEnd::Failed,
        InvocationEnd::Retrying,
        InvocationEnd::Cancelled,
    ]
    .into_iter()
    .for_each(|end| {
        let position = initialization_position(5, end, InvocationOperation::Constructor);
        assert!(
            !initialization_finished_below_start(at(6), Some(&position)),
            "{end:?}"
        );
    });
    let method = initialization_position(
        5,
        InvocationEnd::Completed,
        InvocationOperation::Method("increment".to_string()),
    );
    assert!(!initialization_finished_below_start(at(6), Some(&method)));
}

/// A fork whose initialization is below the retained start finds the key of the initialization
/// in the invocation results of its status. With the start at index 1 the walk reads every entry,
/// so the status gives no key.
#[test]
fn the_key_of_an_initialization_below_the_start_comes_from_the_status() {
    use crate::services::oplog::retention::test_oplog::at;
    use golem_common::model::{InvocationEnd, InvocationOperation};

    let initialization = InvocationId(uuid::Uuid::from_u128(0x1455));
    let method = InvocationId(uuid::Uuid::from_u128(0x1456));
    let mut status = AgentStatusRecord::default();
    status.invocation_results.insert(
        method,
        initialization_position(
            9,
            InvocationEnd::Completed,
            InvocationOperation::Method("increment".to_string()),
        ),
    );
    assert_eq!(initialization_key_below_start(at(4), &status), None);
    status.invocation_results.insert(
        initialization,
        initialization_position(
            3,
            InvocationEnd::Completed,
            InvocationOperation::Constructor,
        ),
    );
    assert_eq!(
        initialization_key_below_start(at(4), &status),
        Some(initialization)
    );
    assert_eq!(initialization_key_below_start(at(1), &status), None);
}

fn initialization_started(key: InvocationId) -> OplogEntry {
    match crate::services::oplog::retention::test_oplog::invocation_started() {
        OplogEntry::AgentInvocationStarted {
            timestamp,
            payload,
            trace_id,
            trace_states,
            invocation_context,
            wallet_pin,
            ..
        } => OplogEntry::AgentInvocationStarted {
            timestamp,
            invocation_id: key,
            operation: Box::new(InvocationRequestOperation::AgentInitialization),
            payload,
            trace_id,
            trace_states,
            invocation_context,
            wallet_pin,
        },
        other => panic!("expected an invocation start, got {other:?}"),
    }
}

/// The walk for the initialization of a fork reads from the retained start. It finds an
/// initialization at or after the start, and gives no key, with no read error, for an
/// initialization below the start.
#[test]
async fn the_initialization_walk_reads_from_the_retained_start() {
    use crate::services::oplog::retention::test_oplog::{RetainedOplog, jump};

    let key = InvocationId(uuid::Uuid::from_u128(0x1455));
    let after = RetainedOplog::new().await;
    after
        .add([
            jump(1),
            jump(2),
            jump(3),
            initialization_started(key),
            jump(5),
        ])
        .await;
    after.advance(3).await;
    let status = AgentStatusRecord {
        oplog_idx: OplogIndex::from_u64(5),
        ..AgentStatusRecord::default()
    };
    assert_eq!(
        find_initialization_key(after.open().await.as_ref(), &status)
            .await
            .unwrap(),
        Some(key)
    );

    let below = RetainedOplog::new().await;
    below
        .add([
            jump(1),
            initialization_started(key),
            jump(3),
            jump(4),
            jump(5),
        ])
        .await;
    below.advance(3).await;
    assert_eq!(
        find_initialization_key(below.open().await.as_ref(), &status)
            .await
            .unwrap(),
        None
    );
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
            semantic_trap_retry_override: None, .. } if reason.contains("Monthly enforcement unavailable") && *anchor == retry_from));
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
