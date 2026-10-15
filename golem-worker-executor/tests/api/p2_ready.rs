use super::*;
use anyhow::Context as _;
use pretty_assertions::assert_eq;
use test_r::test;
use tokio::io::AsyncReadExt;

#[test]
#[timeout("2m")]
async fn durable_p2_ready_is_nonblocking_on_silent_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    silent_tcp_ready(last_unique_id, deps, host_api_tests, true).await
}

#[test]
#[timeout("2m")]
async fn ephemeral_p2_ready_is_nonblocking_on_silent_tcp(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    silent_tcp_ready(last_unique_id, deps, host_api_tests, false).await
}

async fn silent_tcp_ready(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    host_api_tests: &PrecompiledComponent,
    durable: bool,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let executor = start(deps, &context).await?;
    let component = executor
        .component_dep(&context.default_environment_id, host_api_tests)
        .store()
        .await?;
    let name = if durable {
        agent_id!("Networking", "p2-ready-silent-peer")
    } else {
        agent_id!("EphemeralNetworking", "p2-ready-silent-peer")
    };
    let key = IdempotencyKey::fresh();
    let physical = if durable {
        name.clone()
    } else {
        name.clone()
            .with_ephemeral_invocation_phantom(&key)
            .map_err(|error| anyhow!(error))?
    };
    let id = AgentId::from_agent_id(component.id, &physical).map_err(|error| anyhow!(error))?;
    let owned = OwnedAgentId::new(context.default_environment_id, &id);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let mut invocation = tokio::spawn({
        let executor = executor.clone();
        async move {
            executor
                .invoke_and_await_agent_with_key(
                    &component,
                    &name,
                    &key,
                    "tcp_input_ready_p2",
                    data_value!(port),
                )
                .await
        }
    });
    let result = async {
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(30), listener.accept())
            .await
            .context("guest connection")??;
        // The peer remains open and writes nothing until the guest has returned.
        let result = tokio::time::timeout(Duration::from_secs(10), &mut invocation)
            .await
            .context("ready must return without peer readiness")???
            .into_typed::<Result<bool, String>>()?;
        assert_eq!(result, Ok(false));
        let mut byte = [0];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), peer.read(&mut byte)).await??,
            0,
            "guest closes the socket without sending data"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while executor.concurrent_agent_permit_is_held(&owned).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .context("normal permit release")?;
        executor.commit_oplog(&id).await?;
        let entries = executor.get_oplog(&id, OplogIndex::INITIAL).await?;
        let ready_starts: Vec<_> = entries
            .iter()
            .filter_map(|entry| match &entry.entry {
                PublicOplogEntry::Start(start)
                    if start.function_name == "io::poll::pollable::ready" =>
                {
                    Some(entry.oplog_index)
                }
                _ => None,
            })
            .collect();
        assert_eq!(ready_starts.len(), 1, "{entries:#?}");
        let ready_start = ready_starts[0];
        assert_eq!(
            entries.iter().filter(|entry| matches!(
                &entry.entry, PublicOplogEntry::End(end) if end.start_index == ready_start
            )).count(),
            1
        );
        assert!(entries.iter().all(|entry| !matches!(
            &entry.entry, PublicOplogEntry::Cancelled(end) if end.start_index == ready_start
        )));
        assert_eq!(
            count_agent_invocation_pair_since(&entries, OplogIndex::INITIAL),
            (2, 2),
            "constructor and method each complete once"
        );
        info!(durable, %ready_start, "P2 ready returned false and committed End while the peer sent no data");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // Dropping the peer releases an unexpectedly blocked input wait before stopping the worker.
    drop(listener);
    if result.is_err() {
        let _ = tokio::time::timeout(Duration::from_secs(10), executor.interrupt(&id)).await;
        if !invocation.is_finished() {
            let _ = tokio::time::timeout(Duration::from_secs(10), &mut invocation).await;
        }
    }
    result
}
