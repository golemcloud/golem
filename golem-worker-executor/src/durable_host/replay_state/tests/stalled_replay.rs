use super::*;
use golem_common::model::oplog::{
    HostRequestCliEnvironmentGetEnvironment, HostRequestMonotonicClockDuration,
    HostResponseCliEnvironmentGetEnvironment, HostResponseP3MonotonicClockUnit,
};
use test_r::test;

fn environment_pair() -> [OplogEntry; 2] {
    let environment = vec![("GOLEM_AGENT_ID".to_string(), "cached-agent-id".to_string())];
    let mut start = start_named(HostFunctionName::WasiCliEnvironmentGetEnvironment);
    let OplogEntry::Start { request, .. } = &mut start else {
        unreachable!();
    };
    *request = Some(OplogPayload::Inline(Box::new(
        HostRequest::CliEnvironmentGetEnvironment(HostRequestCliEnvironmentGetEnvironment {
            environment: environment.clone(),
        }),
    )));
    let mut end = end_for(2, 0);
    let OplogEntry::End { response, .. } = &mut end else {
        unreachable!();
    };
    *response = Some(OplogPayload::Inline(Box::new(
        HostResponse::CliEnvironmentGetEnvironment(HostResponseCliEnvironmentGetEnvironment {
            environment,
        }),
    )));
    [start, end]
}

async fn replay_with_delivered_timer() -> ReplayState {
    // Suffix after the timer's preliminary clock read: the accessor timer was delivered live,
    // then the guest made a direct clock read. Replay must observe that delivery before it can
    // claim the clock read. All request/response payloads match their real host functions.
    let mut start = start_named(HostFunctionName::P3MonotonicClockWaitFor);
    let OplogEntry::Start { request, .. } = &mut start else {
        unreachable!();
    };
    *request = Some(OplogPayload::Inline(Box::new(
        HostRequest::MonotonicClockDuration(HostRequestMonotonicClockDuration {
            duration_in_nanos: 50_000_000,
        }),
    )));
    let mut end = end_for(2, 0);
    let OplogEntry::End { response, .. } = &mut end else {
        unreachable!();
    };
    *response = Some(OplogPayload::Inline(Box::new(
        HostResponse::P3MonotonicClockUnit(HostResponseP3MonotonicClockUnit {}),
    )));
    replay_state_over(vec![
        noop(),                // 1: already consumed prefix
        start,                 // 2: wait-for Start
        end,                   // 3: wait-for End
        delivered_for(2),      // 4: recorded guest delivery
        start_now(),           // 5: subsequent direct clock read
        end_for(5, 42),        // 6: clock result
        invocation_finished(), // 7: no missing terminal or truncated history
    ])
    .await
}

#[test]
#[test_r::timeout("10s")]
async fn stalled_replay_omitted_direct_environment_call_can_progress() {
    // Model the call omitted after libc cached the environment during snapshot load. The
    // environment call is direct, so its recorded End must not be given a P3 delivery marker.
    let [environment_start, environment_end] = environment_pair();
    let rs = replay_state_over(vec![
        noop(),
        environment_start,
        environment_end,
        start_now(),
        end_for(4, 42),
        invocation_finished(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(4));
    let resolution = tokio::time::timeout(Duration::from_secs(1), rs.await_resolution(handle))
        .await
        .expect("retaining the omitted direct call must not block the subsequent clock read")
        .unwrap();
    assert!(matches!(
        resolution,
        Resolution::Completed { end_idx, delivery_marker: None, .. }
            if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(rs.has_unclaimed_retained_starts());
    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
#[test_r::timeout("10s")]
async fn stalled_replay_late_concurrent_owner_releases_delivery_marker() {
    let rs = replay_with_delivered_timer().await;
    let clock_claim = rs.claim_concurrent_start(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
    );
    tokio::pin!(clock_claim);
    assert!(futures::poll!(clock_claim.as_mut()).is_pending());

    // The later-polled timer task is legitimate concurrent work, not a divergent omission.
    // Releasing its delivery must wake the same pending clock claim without resetting replay.
    let timer = rs
        .claim_concurrent_start(
            &HostFunctionName::P3MonotonicClockWaitFor,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(timer.start_idx(), OplogIndex::from_u64(2));
    let resolution = rs.await_resolution(timer).await.unwrap();
    assert!(matches!(
        resolution,
        Resolution::Completed { end_idx, delivery_marker: Some(marker), .. }
            if end_idx == OplogIndex::from_u64(3) && marker == OplogIndex::from_u64(4)
    ));
    assert!(futures::poll!(clock_claim.as_mut()).is_pending());
    rs.await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4))
        .await
        .unwrap()
        .acknowledge();

    let clock = tokio::time::timeout(Duration::from_secs(1), clock_claim)
        .await
        .expect("the timer's delivery must unblock the pending clock claim")
        .unwrap();
    assert_eq!(clock.start_idx(), OplogIndex::from_u64(5));
    assert!(matches!(
        rs.await_resolution(clock).await.unwrap(),
        Resolution::Completed { end_idx, delivery_marker: None, .. }
            if end_idx == OplogIndex::from_u64(6)
    ));
    assert!(matches!(
        read_invocation_finished(&rs).await.unwrap(),
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
#[ignore = "regression expectation: a quiescent replay must report an unclaimed delivery instead of waiting forever"]
#[test_r::timeout("10s")]
async fn stalled_replay_omitted_delivered_call_reports_divergence() {
    let rs = replay_with_delivered_timer().await;
    // This is the only replay reader. There is deliberately no timer owner, live effect, pending
    // network IO, or unresolved historical call: both calls have durable terminal records.
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        rs.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    )
    .await;
    let error = match outcome {
        Err(_) => panic!(
            "replay stalled: the only reader cannot claim Start(5) beyond the unclaimed \
             wait-for Start(2) and CompletionDelivered(4); cursor={}, target={}",
            rs.last_replayed_index(),
            rs.replay_target(),
        ),
        Ok(Ok(_)) => panic!("replay must not skip a recorded guest-delivery boundary"),
        Ok(Err(error)) => error,
    };
    match error {
        WorkerExecutorError::UnexpectedOplogEntry { got, .. } => {
            assert!(
                got.contains("Start at 2")
                    || got.contains("Start(2)")
                    || got.contains("Start 2")
                    || got.contains("start_index: 2"),
                "diagnostic must identify the unclaimed Start: {got}"
            );
        }
        other => panic!("expected replay divergence, got {other}"),
    }
}
