use super::*;
use golem_worker_executor::services::resource_limits::AtomicResourceEntry;
use pretty_assertions::assert_eq;

pub(crate) struct BatchControl {
    captured: tokio::sync::watch::Sender<u64>,
    replies: Semaphore,
    drive: tokio::sync::Mutex<()>,
    shutdown: CancellationToken,
}

impl BatchControl {
    fn new(shutdown: CancellationToken) -> Self {
        Self {
            captured: tokio::sync::watch::channel(0).0,
            replies: Semaphore::new(0),
            drive: tokio::sync::Mutex::new(()),
            shutdown,
        }
    }

    pub(crate) async fn receive_batch(&self) {
        self.captured.send_modify(|captured| *captured += 1);
        tokio::select! {
            permit = self.replies.acquire() => permit.unwrap().forget(),
            _ = self.shutdown.cancelled() => {}
        }
    }
}

/// Drives the real periodic service through the Registry dependency, never through a runtime hook.
pub(in crate::api) struct PeriodicResourceLimits {
    service: Arc<golem_worker_executor::services::resource_limits::ResourceLimitsGrpc>,
    control: Option<Arc<BatchControl>>,
    accounts: Mutex<HashMap<AccountId, Arc<AtomicResourceEntry>>>,
}

impl PeriodicResourceLimits {
    pub(super) fn new(
        registry: Arc<MutableResourceLimitsRegistry>,
        batch_update_interval: Duration,
        limit_refresh_interval: Duration,
        metering: ResourceUsageMeteringConfig,
        shutdown: CancellationToken,
    ) -> Arc<Self> {
        let controlled = batch_update_interval >= Duration::from_secs(3600);
        let control = controlled.then(|| Arc::new(BatchControl::new(shutdown.clone())));
        *registry.batch_control.lock().unwrap() = control.clone();
        let service = golem_worker_executor::services::resource_limits::ResourceLimitsGrpc::new(
            registry,
            if controlled {
                Duration::from_millis(20)
            } else {
                batch_update_interval
            },
            if controlled {
                Duration::ZERO
            } else {
                limit_refresh_interval
            },
            metering,
            shutdown,
        );
        Arc::new(Self {
            service,
            control,
            accounts: Mutex::default(),
        })
    }

    pub(super) async fn run_batch_for_test(&self) {
        tokio::time::timeout(Duration::from_secs(30), async {
            let control = self.control.as_ref().expect("controlled periodic service");
            let _drive = control.drive.lock().await;
            let mut captured = control.captured.subscribe();
            let previous = *captured.borrow_and_update();
            // Retire any batch captured before the caller's action. The following batch
            // must capture after that action, even when the first timer tick ran earlier.
            if previous > 0 {
                control.replies.add_permits(1);
                while *captured.borrow_and_update() <= previous {
                    captured.changed().await.unwrap();
                }
            } else {
                while *captured.borrow_and_update() == 0 {
                    captured.changed().await.unwrap();
                }
            }
            let completing = *captured.borrow_and_update();
            control.replies.add_permits(1);
            // The next Registry call proves the serial periodic loop has applied this reply,
            // including configurations where monthly capacity monitoring is disabled.
            while *captured.borrow_and_update() <= completing {
                captured.changed().await.unwrap();
            }
        })
        .await
        .expect("periodic Registry batch did not complete");
    }

    /// Applies one Registry reply without also draining the following usage capture.
    pub(super) async fn release_registry_reply(&self) {
        tokio::time::timeout(Duration::from_secs(30), async {
            let control = self.control.as_ref().expect("controlled periodic service");
            let _drive = control.drive.lock().await;
            let mut captured = control.captured.subscribe();
            while *captured.borrow_and_update() == 0 {
                captured.changed().await.unwrap();
            }
            let completing = *captured.borrow_and_update();
            control.replies.add_permits(1);
            while *captured.borrow_and_update() <= completing {
                captured.changed().await.unwrap();
            }
        })
        .await
        .expect("periodic Registry reply did not complete");
    }

    pub(super) async fn initialized_policy_revision_for_test(
        &self,
        account: AccountId,
    ) -> Option<u64> {
        self.accounts.lock().unwrap().get(&account).map(|entry| {
            entry.with_monthly_capacity(
                golem_common::model::agent::AgentMode::Durable,
                None,
                |capacity| capacity.policy_revision,
            )
        })
    }

    pub(super) async fn subscribe_capacity_updates_for_test(
        &self,
        account: AccountId,
    ) -> Result<tokio::sync::watch::Receiver<u64>, WorkerExecutorError> {
        Ok(self
            .initialize_account(account)
            .await?
            .subscribe_capacity_updates())
    }
}

#[async_trait]
impl ResourceLimits for PeriodicResourceLimits {
    async fn initialize_account(
        &self,
        account: AccountId,
    ) -> Result<Arc<AtomicResourceEntry>, WorkerExecutorError> {
        let entry = self.service.initialize_account(account).await?;
        self.accounts.lock().unwrap().insert(account, entry.clone());
        Ok(entry)
    }
}

#[test_r::test]
#[test_r::timeout("2m")]
async fn periodic_registry_choreography_preserves_usage_and_capacity(_tracing: &Tracing) {
    use golem_common::model::agent::AgentMode;

    for enabled in [true, false] {
        let policy = MonthlyResourcePolicy {
            period: AccountUsagePeriod::current(),
            mode: MonthlyUsageMode::HardLimit,
            available_fuel: 1_003,
            available_memory_gb_seconds: u64::MAX,
            available_memory_byte_nanoseconds_remainder: 0,
            available_durable_storage_byte_seconds: u64::MAX,
            available_durable_storage_byte_nanoseconds_remainder: 0,
            available_ephemeral_storage_byte_seconds: u64::MAX,
            available_ephemeral_storage_byte_nanoseconds_remainder: 0,
        };
        let registry = Arc::new(MutableResourceLimitsRegistry::new(policy.clone()));
        let shutdown = CancellationToken::new();
        let limits = PeriodicResourceLimits::new(
            registry.clone(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            ResourceUsageMeteringConfig {
                compute: enabled,
                memory: enabled,
                filesystem: enabled,
            },
            shutdown.clone(),
        );
        let account = AccountId::new();
        let entry = limits.initialize_account(account).await.unwrap();
        // Let the loop capture before the mutation, so the driver must also drain a stale batch.
        let mut captured = limits.control.as_ref().unwrap().captured.subscribe();
        while *captured.borrow_and_update() == 0 {
            captured.changed().await.unwrap();
        }
        assert!(entry.borrow_fuel(37));
        entry.return_fuel(11);
        let mut exhausted = policy;
        exhausted.available_memory_gb_seconds = 0;
        registry.set_policy(exhausted);
        limits.run_batch_for_test().await;
        assert_eq!(
            limits.initialized_policy_revision_for_test(account).await,
            Some(1)
        );
        assert_eq!(
            entry.with_monthly_capacity(AgentMode::Durable, None, |capacity| capacity
                .exhaustion
                .is_some()),
            enabled
        );
        assert_eq!(
            registry
                .applied_updates()
                .iter()
                .map(|update| update.fuel_delta)
                .sum::<i64>(),
            if enabled { 26 } else { 0 }
        );
        limits.run_batch_for_test().await;
        assert_eq!(
            registry
                .applied_updates()
                .iter()
                .map(|update| update.fuel_delta)
                .sum::<i64>(),
            if enabled { 26 } else { 0 }
        );
        shutdown.cancel();
    }
}
