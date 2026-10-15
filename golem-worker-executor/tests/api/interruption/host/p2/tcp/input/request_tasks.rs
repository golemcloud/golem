use super::*;

pub(super) type RequestTask<T> = Option<AbortOnDropHandle<T>>;

pub(super) fn request_is_finished<T>(task: &RequestTask<T>) -> bool {
    task.as_ref().is_none_or(|task| task.is_finished())
}

pub(super) async fn join_request<T>(task: &mut RequestTask<T>, label: &str) -> anyhow::Result<T> {
    let joined = tokio::time::timeout(
        Duration::from_secs(10),
        task.as_mut().context("request task already joined")?,
    )
    .await
    .with_context(|| format!("{label} join timed out"))?;
    task.take();
    joined.with_context(|| format!("{label} task failed"))
}

pub(super) async fn start_refresh(
    limits: &Arc<ResourceLimitsGrpc>,
    registry: &MutableResourceLimitsRegistry,
    account: AccountId,
    task: &mut RequestTask<()>,
) -> anyhow::Result<()> {
    ensure!(task.is_none(), "previous refresh was not joined");
    let revision = registry.current_limits().monthly_policy_revision;
    *task = Some(AbortOnDropHandle::new(tokio::spawn({
        let limits = limits.clone();
        async move { limits.run_batch_for_test().await }
    })));
    tokio::time::timeout(Duration::from_secs(30), async {
        while limits.initialized_policy_revision_for_test(account).await != Some(revision) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .context("input-test policy refresh was not applied")?;
    Ok(())
}

async fn contain_request<T>(
    task: &mut RequestTask<T>,
    label: &str,
    errors: &mut Vec<anyhow::Error>,
) {
    if task.is_none() {
        return;
    }
    if let Err(error) = join_request(task, label).await {
        errors.push(error);
    }
    if task.is_some() {
        task.as_ref().unwrap().abort();
        match join_request(task, label).await {
            Ok(_) => {}
            Err(error)
                if error
                    .downcast_ref::<tokio::task::JoinError>()
                    .is_some_and(|error| error.is_cancelled()) => {}
            Err(error) => errors.push(error),
        }
    }
}

// These handles wrap caller observation and the test's Registry batch request.
// Worker Store settlement, accepted-stop drivers and unload work remain Worker-owned.
pub(super) async fn finish_input_test<T>(
    result: anyhow::Result<()>,
    executor: &TestWorkerExecutor,
    id: &AgentId,
    worker: &Worker<impl golem_worker_executor::workerctx::WorkerCtx>,
    invocation: &mut RequestTask<T>,
    refresh: &mut RequestTask<()>,
) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    if result.is_err() {
        match tokio::time::timeout(Duration::from_secs(10), executor.interrupt(id)).await {
            Ok(Ok(())) => {}
            other => errors.push(anyhow!("cleanup interrupt: {other:?}")),
        }
        match tokio::time::timeout(
            Duration::from_secs(10),
            worker.join_accepted_stops_for_test(),
        )
        .await
        {
            Ok(Ok(())) => {}
            other => errors.push(anyhow!("cleanup accepted stops: {other:?}")),
        }
        match tokio::time::timeout(Duration::from_secs(10), worker.retained_cleanup_for_test())
            .await
        {
            Ok(Ok(())) => {}
            other => errors.push(anyhow!("cleanup retained work: {other:?}")),
        }
    }
    contain_request(invocation, "invocation observer", &mut errors).await;
    contain_request(refresh, "Registry refresh request", &mut errors).await;
    match (result, errors.is_empty()) {
        (result, true) => result,
        (Ok(()), false) => Err(anyhow!("cleanup failed: {errors:#?}")),
        (Err(primary), false) => Err(primary.context(format!("cleanup failed: {errors:#?}"))),
    }
}

#[test_r::test]
async fn joined_request_is_consumed_before_result_assertions() -> anyhow::Result<()> {
    let mut task = Some(AbortOnDropHandle::new(tokio::spawn(async {
        Err::<(), _>(anyhow!("request failed"))
    })));
    ensure!(join_request(&mut task, "request").await?.is_err());
    ensure!(task.is_none());
    let mut errors = Vec::new();
    contain_request(&mut task, "request", &mut errors).await;
    ensure!(errors.is_empty());
    Ok(())
}

#[test_r::test]
#[timeout("30s")]
async fn request_observation_timeout_is_aborted_and_joined() -> anyhow::Result<()> {
    let (dropped, observed_drop) = tokio::sync::oneshot::channel::<()>();
    let mut task = Some(AbortOnDropHandle::new(tokio::spawn(async move {
        let _drop_receipt = dropped;
        std::future::pending::<()>().await;
    })));
    let mut errors = Vec::new();
    contain_request(&mut task, "request", &mut errors).await;
    ensure!(task.is_none(), "aborted request must be joined");
    ensure!(errors.len() == 1 && errors[0].to_string().contains("join timed out"));
    ensure!(
        observed_drop.await.is_err(),
        "request future must be dropped"
    );
    Ok(())
}
