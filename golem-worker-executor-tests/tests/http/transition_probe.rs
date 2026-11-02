use super::*;
use golem_common::model::agent::{RevertToOplogIndex, RevertWorkerTarget};
use golem_worker_executor_test_utils::ReplayAdmissionStage as Stage;
use pretty_assertions::assert_eq;
use std::sync::atomic::{AtomicUsize, Ordering};
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("http_tests")]
    PrecompiledComponent
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoverySchedule {
    ClockClaimFirst,
    SendClaimFirst,
    ClockIssuedBeforeSend,
    RetainedControl,
}

async fn http_clock_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    component: &PrecompiledComponent,
    schedule: RecoverySchedule,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, component)
        .store()
        .await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let (tx, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let count = Arc::new(AtomicUsize::new(0));
    let server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn({
        let count = count.clone();
        async move {
            let route = Router::new().route(
                "/transition",
                axum::routing::any(move || {
                    let tx = tx.clone();
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        let (release, wait) = tokio::sync::oneshot::channel();
                        tx.send(release).unwrap();
                        let _ = wait.await;
                        http::StatusCode::NO_CONTENT
                    }
                }),
            );
            axum::serve(listener, route).await.unwrap();
        }
    }));
    let agent = agent_id!("HttpClient4");
    let worker = executor
        .start_agent_with(
            &component.id,
            agent.clone(),
            HashMap::from([("PORT".to_string(), port.to_string())]),
            Vec::new(),
        )
        .await?;
    let mut before_clock = executor.gate_next_replay_access_admission(
        &worker,
        "monotonic-clock::now",
        Stage::BeforeDeferredStart,
    );
    let mut issued_clock = executor.gate_next_replay_access_admission(
        &worker,
        "monotonic-clock::now",
        Stage::AfterDeferredStart,
    );
    let recording = executor.invoke_and_await_agent(
        &component,
        &agent,
        "transition_clock_probe",
        data_value!(schedule != RecoverySchedule::RetainedControl),
    );
    let coordinate = async {
        before_clock.entered().await;
        let release_http = requests.recv().await.expect("HTTP recording starts");
        before_clock.release();
        issued_clock.entered().await;
        executor.commit_oplog(&worker).await?;
        issued_clock.release();
        release_http.send(()).unwrap();
        anyhow::Ok(())
    };
    let (recorded, coordinated) = timeout(Duration::from_secs(40), async {
        tokio::join!(recording, coordinate)
    })
    .await?;
    coordinated?;
    assert_eq!(recorded?.into_typed::<u16>()?, 204);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let entries = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let clock = entries
        .iter()
        .find_map(|entry| match &entry.entry {
            PublicOplogEntry::Start(start)
                if start.function_name == "clocks::monotonic-clock::now" =>
            {
                Some(entry.oplog_index)
            }
            _ => None,
        })
        .expect("recorded P3 clock Start");
    executor
        .revert(
            &worker,
            RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                last_oplog_index: clock,
            }),
        )
        .await?;
    drop(before_clock);
    drop(issued_clock);
    drop(executor);

    let executor = start(deps, &context).await?;
    let mut before_clock = executor.gate_next_replay_access_admission(
        &worker,
        "monotonic-clock::now",
        Stage::BeforeDeferredStart,
    );
    let mut issued_clock = executor.gate_next_replay_access_admission(
        &worker,
        "monotonic-clock::now",
        Stage::AfterDeferredStart,
    );
    let mut before_send =
        executor.gate_next_replay_access_admission(&worker, "client::send", Stage::AfterScope);
    let mut issued_send =
        executor.gate_next_replay_access_admission(&worker, "client::send", Stage::AfterStart);
    let mut before_scope = (schedule == RecoverySchedule::ClockIssuedBeforeSend).then(|| {
        executor.gate_next_replay_access_admission(&worker, "client::send", Stage::BeforeScope)
    });
    let reconstruction =
        executor.invoke_and_await_agent(&component, &agent, "stored_send_error", data_value!());
    let coordinate = async {
        before_clock.entered().await;
        if let Some(before_scope) = &mut before_scope {
            before_scope.entered().await;
            before_clock.release();
            issued_clock.entered().await;
            before_scope.release();
        }
        before_send.entered().await;
        if schedule == RecoverySchedule::ClockClaimFirst {
            before_clock.release();
            issued_clock.entered().await;
        }
        before_send.release();
        issued_send.entered().await;
        if matches!(
            schedule,
            RecoverySchedule::SendClaimFirst | RecoverySchedule::RetainedControl
        ) {
            before_clock.release();
            issued_clock.entered().await;
        }
        issued_send.release();
        issued_clock.release();
        let release = requests.recv().await.expect("HTTP recovery starts");
        release.send(()).unwrap();
        anyhow::Ok(())
    };
    let (recovered, coordinated) = timeout(Duration::from_secs(40), async {
        tokio::join!(reconstruction, coordinate)
    })
    .await?;
    coordinated?;
    assert_eq!(recovered?.into_typed::<String>()?, "none");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    executor.commit_oplog(&worker).await?;
    let repaired = executor.get_oplog(&worker, OplogIndex::INITIAL).await?;
    let dropped_regions: Vec<_> = repaired
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Revert(params) => Some(params.dropped_region.clone()),
            _ => None,
        })
        .collect();
    let repaired: Vec<_> = repaired
        .into_iter()
        .filter(|entry| {
            !dropped_regions
                .iter()
                .any(|region| region.contains(entry.oplog_index))
        })
        .collect();
    let jumps: Vec<_> = repaired
        .iter()
        .filter_map(|entry| match &entry.entry {
            PublicOplogEntry::Jump(params) => Some((entry.oplog_index, &params.jump)),
            _ => None,
        })
        .collect();
    assert!(
        jumps.is_empty(),
        "call-local HTTP recovery must preserve all interleaved history: {jumps:?}"
    );
    let sends = partition_starts(&repaired, "http::client::send");
    assert_eq!(sends.counts(), (0, 1, 0));
    assert_eq!(
        sends.ended,
        partition_starts(&entries, "http::client::send").ended,
        "recovery completes the original send Start in place"
    );
    let retained_clock = repaired.iter().find_map(|entry| match &entry.entry {
        PublicOplogEntry::Start(start) if start.function_name == "clocks::monotonic-clock::now" => {
            Some(entry.oplog_index)
        }
        _ => None,
    });
    drop(before_clock);
    drop(issued_clock);
    drop(before_send);
    drop(issued_send);
    drop(before_scope);
    drop(executor);

    let executor = start(deps, &context).await?;
    let before = count.load(Ordering::SeqCst);
    let second = timeout(
        Duration::from_secs(30),
        executor.invoke_and_await_agent(&component, &agent, "stored_send_error", data_value!()),
    )
    .await;
    server.abort();
    let retained_clock = retained_clock.expect("a replayable clock Start survives recovery");
    assert_eq!(retained_clock, clock, "recovery adopts the old clock Start");
    assert!(repaired.iter().any(|entry| matches!(
        &entry.entry,
        PublicOplogEntry::End(params) if params.start_index == retained_clock
    )));
    assert!(!repaired.iter().any(|entry| match &entry.entry {
        PublicOplogEntry::CompletionDelivered(params) => params.start_index == retained_clock,
        PublicOplogEntry::CompletionDiscarded(params) => params.start_index == retained_clock,
        _ => false,
    }));
    assert_eq!(second??.into_typed::<String>()?, "none");
    assert_eq!(
        before,
        count.load(Ordering::SeqCst),
        "completed replay must not repeat HTTP"
    );
    Ok(())
}

#[test]
#[test_r::timeout("3m")]
async fn http_clock_claim_before_send_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    http_clock_recovery(
        last_unique_id,
        deps,
        component,
        RecoverySchedule::ClockClaimFirst,
    )
    .await
}

#[test]
#[test_r::timeout("3m")]
async fn http_clock_send_recovery_before_claim(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    http_clock_recovery(
        last_unique_id,
        deps,
        component,
        RecoverySchedule::SendClaimFirst,
    )
    .await
}

#[test]
#[test_r::timeout("3m")]
async fn http_clock_issued_before_send_recovery(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    http_clock_recovery(
        last_unique_id,
        deps,
        component,
        RecoverySchedule::ClockIssuedBeforeSend,
    )
    .await
}

#[test]
#[test_r::timeout("3m")]
async fn http_clock_retained_start_without_jump(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("http_tests")] component: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    http_clock_recovery(
        last_unique_id,
        deps,
        component,
        RecoverySchedule::RetainedControl,
    )
    .await
}
