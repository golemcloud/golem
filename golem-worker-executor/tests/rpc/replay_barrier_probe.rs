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
    let a = agent_id!("RpcBlockingCounter", "barrier-a");
    let b = agent_id!("RpcBlockingCounter", "barrier-b");
    let counter = agent_id!("RpcCounter", "barrier-counter");
    let caller_id = agent_id!("CancelTester", "barrier-caller");
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
    executor.complete_promise(&gate_a, vec![]).await?;
    // B remains pending until A's guest delivery and its synchronous client drop are recorded.
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut last_len = 0;
        loop {
            executor.commit_oplog(&caller).await?;
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
            if let Some(delivered_a) = delivered_a {
                let drop_start = oplog.iter().find_map(|e| match &e.entry {
                    PublicOplogEntry::Start(s)
                        if s.function_name == "golem::rpc::wasm-rpc::drop"
                            && e.oplog_index > delivered_a =>
                    {
                        Some(e.oplog_index)
                    }
                    _ => None,
                });
                if oplog.iter().any(|e| match &e.entry {
                    PublicOplogEntry::End(end) => Some(end.start_index) == drop_start,
                    _ => false,
                }) {
                    return Ok::<_, anyhow::Error>(());
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    eprintln!("A delivered while B is pending; releasing B");
    executor.complete_promise(&gate_b, vec![]).await?;
    let result = tokio::time::timeout(Duration::from_secs(30), invocation).await???;
    assert_eq!(result.into_typed::<u64>()?, 14);
    let oplog = executor.get_oplog(&caller, OplogIndex::INITIAL).await?;
    print_rpc_memory_oplog(&oplog);
    assert!(
        !oplog
            .iter()
            .any(|e| matches!(&e.entry, PublicOplogEntry::Snapshot(_)))
    );
    let starts: Vec<_> = oplog
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
    assert!(starts[0] < starts[1]);
    assert!(end(starts[8]) < starts[9]);
    assert!(starts[9] < end(starts[0]));
    assert!(end(starts[0]) < delivered_a);
    assert!(delivered_a < end(starts[9]));
    let delivered_b = delivered(starts[9]);
    let drop_start = oplog
        .iter()
        .find_map(|e| match &e.entry {
            PublicOplogEntry::Start(s)
                if s.function_name == "golem::rpc::wasm-rpc::drop"
                    && e.oplog_index > delivered_a
                    && e.oplog_index < delivered_b =>
            {
                Some(e.oplog_index)
            }
            _ => None,
        })
        .expect("the synchronous client drop must start between A and B deliveries");
    assert!(delivered_a < drop_start);
    assert!(drop_start < end(drop_start));
    assert!(end(drop_start) < end(starts[9]));
    assert!(end(starts[9]) < delivered_b);
    for start in &starts[1..9] {
        assert!(!oplog.iter().any(|e| match &e.entry {
            PublicOplogEntry::CompletionDelivered(d) => d.start_index == *start,
            _ => false,
        }));
    }
    eprintln!(
        "Recorded overlap verified: A={}, last sync={}, open B={}, A delivery={delivered_a}",
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
        let key = key.as_ref().unwrap();
        let history = executor.get_oplog(target, OplogIndex::INITIAL).await?;
        assert_eq!(history.iter().filter(|entry| matches!(&entry.entry,
            PublicOplogEntry::AgentInvocationStarted(started)
                if matches!(&started.invocation, PublicAgentInvocation::AgentMethodInvocation(method)
                    if method.method_name == "inc_after_promise" && &method.idempotency_key == key)
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
