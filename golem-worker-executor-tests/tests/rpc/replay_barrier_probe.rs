use super::*;
use pretty_assertions::assert_eq;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("agent_rpc_rust")]
    PrecompiledComponent
);

/// The target agent of the `wasm-rpc::invoke_and_await` call recorded by the `Start` at `start`.
/// Calls in flight together can record their `Start`s in either order, so the tests identify a
/// call by its target, not by its position.
fn rpc_target(oplog: &[PublicOplogEntryWithIndex], start: OplogIndex) -> AgentId {
    oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::Start(s) if e.oplog_index == start => Some(
                HostRequestGolemRpcInvoke::from_value(s.request.as_ref().unwrap().value())
                    .unwrap()
                    .remote_agent_id,
            ),
            _ => None,
        })
        .unwrap()
}

#[test]
#[timeout("3 minutes")]
async fn real_rpc_retained_start_reconstruction_probe(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorded = attempts.clone();
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.rpc_suspend_after = Duration::from_secs(120);
        })),
        wrap_rpc: Some(Arc::new(move |rpc| {
            Arc::new(RecordingRpc::new(
                rpc,
                "inc_after_promise",
                recorded.clone(),
            ))
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let a = agent_key!("RpcBlockingCounter", "barrier-a");
    let b = agent_key!("RpcBlockingCounter", "barrier-b");
    let counter = agent_key!("RpcCounter", "barrier-counter");
    let caller_id = agent_key!("CancelTester", "barrier-caller");
    let target_a = executor.start_agent(&component.id, a.clone()).await?;
    let target_b = executor.start_agent(&component.id, b.clone()).await?;
    executor.start_agent(&component.id, counter.clone()).await?;
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    let gate_a = executor
        .invoke_and_await_agent(&component, &a, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let gate_b = executor
        .invoke_and_await_agent(&component, &b, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let gate_a = gate_a.clone();
        let gate_b = gate_b.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent(
                    &component,
                    &caller_id,
                    "replay_barrier_probe",
                    data_value!(
                        "barrier-a",
                        gate_a,
                        "barrier-b",
                        gate_b,
                        "barrier-counter",
                        8u32
                    ),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while attempts.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    eprintln!("Both blocking RPCs dispatched; releasing A");
    executor
        .complete_promise(&executor.context.default_environment_id, &gate_a, vec![])
        .await?;
    // B remains pending until A's guest delivery and the release of its client are recorded.
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut last_len = 0;
        loop {
            executor.commit_oplog(&caller.agent_id).await?;
            let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
            if oplog.len() != last_len {
                last_len = oplog.len();
                print_rpc_memory_oplog(&oplog);
            }
            let first_rpc = oplog.iter().find_map(|e| match &e.entry {
                PublicOplogEntry::Start(s)
                    if s.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
                {
                    Some(e.oplog_index)
                }
                _ => None,
            });
            let delivered_a = oplog.iter().find_map(|e| match &e.entry {
                PublicOplogEntry::CompletionDelivered(d) if Some(d.start_index) == first_rpc => {
                    Some(e.oplog_index)
                }
                _ => None,
            });
            if let Some(delivered_a) = delivered_a
                && oplog.iter().any(|e| {
                    e.oplog_index > delivered_a
                        && matches!(&e.entry, PublicOplogEntry::SpanClosed(_))
                })
            {
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    eprintln!("A delivered while B is pending; releasing B");
    executor
        .complete_promise(&executor.context.default_environment_id, &gate_b, vec![])
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(30), invocation).await???;
    assert_eq!(result.into_typed::<u64>()?, 14);
    let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    print_rpc_memory_oplog(&oplog);
    assert!(
        !oplog
            .iter()
            .any(|e| matches!(&e.entry, PublicOplogEntry::Snapshot(_)))
    );
    let mut starts: Vec<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            PublicOplogEntry::Start(s)
                if s.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
            {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 18);
    let end = |start| {
        oplog
            .iter()
            .find_map(|e| {
                matches!(&e.entry, PublicOplogEntry::End(v) if v.start_index == start)
                    .then_some(e.oplog_index)
            })
            .unwrap()
    };
    // A and the first inner call are in flight together, so either can record its `Start` first.
    let a_position = starts[..2]
        .iter()
        .position(|start| rpc_target(&oplog, *start) == target_a.agent_id)
        .expect("A's call records one of the first two RPC Starts");
    let a_start = starts.remove(a_position);
    starts.insert(0, a_start);
    let delivered = |start| {
        oplog
            .iter()
            .find_map(|e| match &e.entry {
                PublicOplogEntry::CompletionDelivered(v) if v.start_index == start => {
                    Some(e.oplog_index)
                }
                _ => None,
            })
            .unwrap()
    };
    let delivered_a = delivered(starts[0]);
    assert!(end(starts[8]) < starts[9]);
    assert!(starts[9] < end(starts[0]));
    assert!(end(starts[0]) < delivered_a);
    assert!(delivered_a < end(starts[9]));
    let delivered_b = delivered(starts[9]);
    // A's client is released after its delivery and before B completes. The release is a
    // hint, not a durable call: it closes the connection span opened by A's `wasm-rpc::upsert`.
    let new_starts: Vec<_> = oplog
        .iter()
        .filter_map(|e| match &e.entry {
            PublicOplogEntry::Start(s) if s.function_name == "golem::rpc::wasm-rpc::upsert" => {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .collect();
    let span_closed = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::SpanClosed(closed)
                if e.oplog_index > delivered_a && e.oplog_index < delivered_b =>
            {
                Some((e.oplog_index, closed.resource_start_index))
            }
            _ => None,
        })
        .expect("the release of A's client must be recorded between A and B deliveries");
    assert!(
        span_closed
            .1
            .is_some_and(|start| new_starts.contains(&start) && start < starts[0]),
        "the closed span belongs to the connection of A"
    );
    assert!(span_closed.0 < end(starts[9]));
    assert!(end(starts[9]) < delivered_b);
    assert!(
        !oplog.iter().any(|e| matches!(&e.entry,
            PublicOplogEntry::Start(s) if s.function_name == "golem::rpc::wasm-rpc::drop")),
        "releasing a client records no durable call"
    );
    // Each awaited inner call is delivered to the guest before it makes the next call.
    for pair in starts[1..10].windows(2) {
        assert!(end(pair[0]) < delivered(pair[0]));
        assert!(delivered(pair[0]) < pair[1]);
    }
    eprintln!(
        "Recorded overlap verified: A={}, last inner={}, open B={}, A delivery={delivered_a}",
        starts[0], starts[8], starts[9]
    );
    let attempts_before = attempts.lock().unwrap().clone();
    assert_eq!(attempts_before.len(), 2);
    assert!(attempts_before.iter().all(Option::is_some));
    assert_ne!(attempts_before[0], attempts_before[1]);
    drop(executor);
    eprintln!("Executor dropped; starting fresh executor and invocation");
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&component, &caller_id, "name", data_value!()),
    )
    .await??;
    assert_eq!(result.into_typed::<String>()?, "barrier-caller");
    assert_eq!(
        *attempts.lock().unwrap(),
        attempts_before,
        "completed replay must not redispatch RPCs"
    );
    for (id, expected) in [(a, 7u64), (b, 7), (counter, 64)] {
        let value = executor
            .invoke_and_await_agent(&component, &id, "get_value", data_value!())
            .await?
            .into_typed::<u64>()?;
        assert_eq!(value, expected);
    }
    for (target, key) in [
        (&target_a, &attempts_before[0]),
        (&target_b, &attempts_before[1]),
    ] {
        let key = executor
            .finalized_invocation_key(
                &target.agent_id,
                &InvocationRequestKey::Internal(key.clone().unwrap()),
                "inc_after_promise",
            )
            .await?;
        let history = executor.get_oplog(target, OplogIndex::INITIAL).await?;
        assert_eq!(history.iter().filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "inc_after_promise" && method.invocation_id == key)
        )).count(), 1, "one target invocation must execute for each recorded RPC key");
        assert_eq!(
            history
                .iter()
                .filter(|entry| matches!(&entry.entry,
                    PublicOplogEntry::AgentInvocationFinished(finished)
                        if finished.method_name.as_deref() == Some("inc_after_promise")
                ))
                .count(),
            1
        );
    }
    Ok(())
}

/// Two awaited calls of one caller invocation complete in the opposite order from their start,
/// live and across a real executor restart. The later call completes first and is delivered
/// before the restart; after recovery only the earlier call is dispatched again (with its
/// original key), and each target runs its invocation once.
#[test]
#[timeout("3 minutes")]
async fn two_awaited_calls_complete_in_reverse_order_across_restart(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("agent_rpc_rust")] fixture: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let recorded = attempts.clone();
    let overrides = TestExecutorOverrides {
        configure: Some(Arc::new(|config| {
            config.suspend.rpc_suspend_after = Duration::from_secs(120);
        })),
        wrap_rpc: Some(Arc::new(move |rpc| {
            Arc::new(RecordingRpc::new(
                rpc,
                "inc_after_promise",
                recorded.clone(),
            ))
        })),
        ..Default::default()
    };
    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let component = executor
        .component_dep(&context.default_environment_id, fixture)
        .store()
        .await?;
    let a = agent_key!("RpcBlockingCounter", "reverse-a");
    let b = agent_key!("RpcBlockingCounter", "reverse-b");
    let caller_id = agent_key!("CancelTester", "reverse-caller");
    let target_a = executor.start_agent(&component.id, a.clone()).await?;
    let target_b = executor.start_agent(&component.id, b.clone()).await?;
    let caller = executor
        .start_agent(&component.id, caller_id.clone())
        .await?;
    executor
        .invoke_and_await_agent(&component, &b, "inc_by", data_value!(10u64))
        .await?;
    let gate_a = executor
        .invoke_and_await_agent(&component, &a, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let gate_b = executor
        .invoke_and_await_agent(&component, &b, "create_promise", data_value!())
        .await?
        .into_typed::<PromiseId>()?;
    let key = IdempotencyKey::fresh();
    let input = data_value!("reverse-a", gate_a.clone(), "reverse-b", gate_b.clone());
    let invocation = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let key = key.clone();
        let input = input.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &caller_id,
                    &key,
                    "await_two_gated",
                    input,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while attempts.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let rpc_starts = |oplog: &[PublicOplogEntryWithIndex]| {
        oplog
            .iter()
            .filter_map(|e| match &e.entry {
                PublicOplogEntry::Start(s)
                    if s.function_name == "golem::rpc::wasm-rpc::invoke_and_await" =>
                {
                    Some(e.oplog_index)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let has_end = |oplog: &[PublicOplogEntryWithIndex], start: OplogIndex| {
        oplog
            .iter()
            .any(|e| matches!(&e.entry, PublicOplogEntry::End(end) if end.start_index == start))
    };
    let delivered = |oplog: &[PublicOplogEntryWithIndex], start: OplogIndex| {
        oplog.iter().any(|e| {
            matches!(&e.entry, PublicOplogEntry::CompletionDelivered(d) if d.start_index == start)
        })
    };

    // The two calls can record their `Start`s in either order; the call that started later is
    // released first.
    let (earlier_start, later_start, later_is_b) =
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                executor.commit_oplog(&caller.agent_id).await?;
                let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
                if let [earlier, later] = rpc_starts(&oplog)[..] {
                    let later_is_b = rpc_target(&oplog, later) == target_b.agent_id;
                    return Ok::<_, anyhow::Error>((earlier, later, later_is_b));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
    let (earlier_gate, later_gate, earlier_target, later_target) = if later_is_b {
        (&gate_a, &gate_b, &target_a, &target_b)
    } else {
        (&gate_b, &gate_a, &target_b, &target_a)
    };
    executor
        .complete_promise(&executor.context.default_environment_id, later_gate, vec![])
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            executor.commit_oplog(&caller.agent_id).await?;
            let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
            if delivered(&oplog, later_start) {
                assert!(
                    !has_end(&oplog, earlier_start),
                    "the earlier call must still be open when the later one is delivered"
                );
                return Ok::<_, anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    let attempts_before = attempts.lock().unwrap().clone();
    assert_eq!(attempts_before.len(), 2);
    assert!(attempts_before.iter().all(Option::is_some));
    assert_ne!(attempts_before[0], attempts_before[1]);

    invocation.abort();
    let _ = invocation.await;
    drop(executor);

    let executor = start_with_overrides(deps, &context, overrides.clone()).await?;
    let resumed = {
        let executor = executor.clone();
        let component = component.clone();
        let caller_id = caller_id.clone();
        let key = key.clone();
        let input = input.clone();
        tokio::spawn(async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &caller_id,
                    &key,
                    "await_two_gated",
                    input,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        while attempts.lock().unwrap().len() < 3 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    executor
        .complete_promise(
            &executor.context.default_environment_id,
            earlier_gate,
            vec![],
        )
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(30), resumed).await???;
    assert_eq!(result.into_typed::<(u64, u64)>()?, (7, 17));
    let attempts_after = attempts.lock().unwrap().clone();
    assert_eq!(
        attempts_after.len(),
        3,
        "recovery repairs only the open call"
    );
    assert_eq!(attempts_after[..2], attempts_before[..]);
    // The two first dispatches race, so the repaired key identifies the earlier call; the target
    // histories below prove that it belongs to the earlier call's target.
    let earlier_key = attempts_after[2].clone();
    assert!(
        attempts_before.contains(&earlier_key),
        "recovery dispatches the open call with its original key"
    );
    let later_key = attempts_before
        .iter()
        .find(|key| **key != earlier_key)
        .cloned()
        .unwrap();
    let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    assert_eq!(rpc_starts(&oplog), vec![earlier_start, later_start]);
    assert!(delivered(&oplog, earlier_start) && delivered(&oplog, later_start));

    drop(executor);
    let executor = start_with_overrides(deps, &context, overrides).await?;
    let replayed = tokio::time::timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent_with_key(
            &component,
            &caller_id,
            &key,
            "await_two_gated",
            input,
        ),
    )
    .await??;
    assert_eq!(replayed.into_typed::<(u64, u64)>()?, (7, 17));
    assert_eq!(
        executor
            .invoke_and_await_agent(&component, &caller_id, "name", data_value!())
            .await?
            .into_typed::<String>()?,
        "reverse-caller"
    );
    assert_eq!(
        *attempts.lock().unwrap(),
        attempts_after,
        "completed replay must not redispatch RPCs"
    );
    for (id, expected) in [(a, 7u64), (b, 17)] {
        let value = executor
            .invoke_and_await_agent(&component, &id, "get_value", data_value!())
            .await?
            .into_typed::<u64>()?;
        assert_eq!(value, expected);
    }
    for (target, key) in [(earlier_target, &earlier_key), (later_target, &later_key)] {
        let key = executor
            .finalized_invocation_key(
                &target.agent_id,
                &InvocationRequestKey::Internal(key.clone().unwrap()),
                "inc_after_promise",
            )
            .await?;
        let history = executor.get_oplog(target, OplogIndex::INITIAL).await?;
        assert_eq!(history.iter().filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "inc_after_promise" && method.invocation_id == key)
        )).count(), 1, "one target invocation must execute for each recorded RPC key");
    }
    Ok(())
}
