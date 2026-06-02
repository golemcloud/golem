// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Cleanup helpers for cloud-perf benchmarks.
//!
//! The [`CleanupClient`] trait is the narrow interface used by the cascading
//! cleanup logic, which enables unit-testing with the [`MockCleanupClient`]
//! below.

use async_trait::async_trait;
use golem_client::api::RegistryServiceClient;
use golem_common::model::environment::EnvironmentId;
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use tracing::warn;
use uuid::Uuid;

// ── Narrow trait ─────────────────────────────────────────────────────────────

/// Narrow client interface covering only the operations used by the cascading
/// cleanup helpers.  Use [`RegistryCleanupAdapter`] to wrap a real client and
/// [`MockCleanupClient`] (in tests) to inject failures.
#[async_trait]
pub trait CleanupClient: Send + Sync {
    /// Returns `(component_id, revision)` pairs for all components in the env.
    async fn list_env_components(&self, env_id: &Uuid) -> anyhow::Result<Vec<(Uuid, u64)>>;
    async fn delete_component(&self, id: &Uuid, revision: u64) -> anyhow::Result<()>;

    /// Returns domain-registration IDs for the env.
    async fn list_env_domain_registrations(&self, env_id: &Uuid) -> anyhow::Result<Vec<Uuid>>;
    async fn delete_domain_registration(&self, id: &Uuid) -> anyhow::Result<()>;

    /// Returns `(application_id, env_revision)` for the environment.
    async fn get_env_app_id_and_revision(&self, env_id: &Uuid) -> anyhow::Result<(Uuid, u64)>;
    async fn delete_environment(&self, env_id: &Uuid, revision: u64) -> anyhow::Result<()>;

    /// Returns the application's current revision.
    async fn get_application_revision(&self, app_id: &Uuid) -> anyhow::Result<u64>;
    async fn delete_application(&self, app_id: &Uuid, revision: u64) -> anyhow::Result<()>;

    /// Returns the account's current revision.
    async fn get_account_revision(&self, account_id: &Uuid) -> anyhow::Result<u64>;
    async fn delete_account(&self, account_id: &Uuid, revision: u64) -> anyhow::Result<()>;
}

// ── Real adapter ─────────────────────────────────────────────────────────────

/// Wraps any `RegistryServiceClient` implementor and bridges it to
/// [`CleanupClient`].
pub struct RegistryCleanupAdapter<C> {
    inner: C,
}

impl<C: RegistryServiceClient + Send + Sync> RegistryCleanupAdapter<C> {
    pub fn new(inner: C) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<C: RegistryServiceClient + Send + Sync> CleanupClient for RegistryCleanupAdapter<C> {
    async fn list_env_components(&self, env_id: &Uuid) -> anyhow::Result<Vec<(Uuid, u64)>> {
        let page = self
            .inner
            .list_environment_components(env_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(page
            .values
            .into_iter()
            .map(|c| (c.id.0, c.revision.into()))
            .collect())
    }

    async fn delete_component(&self, id: &Uuid, revision: u64) -> anyhow::Result<()> {
        self.inner
            .delete_component(id, revision)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    async fn list_env_domain_registrations(&self, env_id: &Uuid) -> anyhow::Result<Vec<Uuid>> {
        let page = self
            .inner
            .list_environment_domain_registrations(env_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(page.values.into_iter().map(|dr| dr.id.0).collect())
    }

    async fn delete_domain_registration(&self, id: &Uuid) -> anyhow::Result<()> {
        self.inner
            .delete_domain_registration(id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    async fn get_env_app_id_and_revision(&self, env_id: &Uuid) -> anyhow::Result<(Uuid, u64)> {
        let env = self
            .inner
            .get_environment(env_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok((env.application_id.0, env.revision.into()))
    }

    async fn delete_environment(&self, env_id: &Uuid, revision: u64) -> anyhow::Result<()> {
        self.inner
            .delete_environment(env_id, revision)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    async fn get_application_revision(&self, app_id: &Uuid) -> anyhow::Result<u64> {
        let app = self
            .inner
            .get_application(app_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(app.revision.into())
    }

    async fn delete_application(&self, app_id: &Uuid, revision: u64) -> anyhow::Result<()> {
        self.inner
            .delete_application(app_id, revision)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    async fn get_account_revision(&self, account_id: &Uuid) -> anyhow::Result<u64> {
        let account = self
            .inner
            .get_account(account_id)
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(account.revision.into())
    }

    async fn delete_account(&self, account_id: &Uuid, revision: u64) -> anyhow::Result<()> {
        self.inner
            .delete_account(account_id, revision)
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }
}

// ── Core cleanup logic (testable via CleanupClient) ───────────────────────────

/// Steps 1–4 of the cascading cleanup: components → domain registrations →
/// environment → application.  Does **not** delete the account.
///
/// Every step is best-effort: failures are warned and cleanup continues.
///
/// **Note:** Server-side cascading delete is incomplete (golemcloud/golem#3291).
pub async fn cleanup_env_and_app_with(client: &dyn CleanupClient, env_id: &Uuid) {
    // Step 1: components
    match client.list_env_components(env_id).await {
        Ok(components) => {
            for (cid, rev) in components {
                if let Err(e) = client.delete_component(&cid, rev).await {
                    warn!("cleanup: delete component {cid} failed (best-effort): {e:?}");
                }
            }
        }
        Err(e) => warn!("cleanup: list components for env {env_id} failed (best-effort): {e:?}"),
    }

    // Step 2: domain registrations
    match client.list_env_domain_registrations(env_id).await {
        Ok(ids) => {
            for id in ids {
                if let Err(e) = client.delete_domain_registration(&id).await {
                    warn!("cleanup: delete domain registration {id} failed (best-effort): {e:?}");
                }
            }
        }
        Err(e) => {
            warn!(
                "cleanup: list domain registrations for env {env_id} failed \
                 (best-effort): {e:?}"
            )
        }
    }

    // Step 3: environment (also captures app_id for step 4)
    let app_id = match client.get_env_app_id_and_revision(env_id).await {
        Ok((app_id, rev)) => {
            if let Err(e) = client.delete_environment(env_id, rev).await {
                warn!("cleanup: delete environment {env_id} failed (best-effort): {e:?}");
            }
            Some(app_id)
        }
        Err(e) => {
            warn!("cleanup: get environment {env_id} failed (best-effort): {e:?}");
            None
        }
    };

    // Step 4: application (only when app_id is known from step 3)
    if let Some(app_id) = app_id {
        match client.get_application_revision(&app_id).await {
            Ok(rev) => {
                if let Err(e) = client.delete_application(&app_id, rev).await {
                    warn!("cleanup: delete application {app_id} failed (best-effort): {e:?}");
                }
            }
            Err(e) => {
                warn!("cleanup: get application {app_id} failed (best-effort): {e:?}")
            }
        }
    }
}

/// Step 5 of the cascading cleanup: deletes the user account.
pub async fn cleanup_account_with(client: &dyn CleanupClient, account_id: &Uuid) {
    match client.get_account_revision(account_id).await {
        Ok(rev) => {
            if let Err(e) = client.delete_account(account_id, rev).await {
                warn!("cleanup: delete account {account_id} failed (best-effort): {e:?}");
            }
        }
        Err(e) => {
            warn!("cleanup: get account {account_id} failed (best-effort): {e:?}")
        }
    }
}

// ── High-level wrappers (take a TestUserContext) ──────────────────────────────

/// Steps 1–4: components, domain registrations, environment, application.
///
/// For benchmarks whose iterations create one user with multiple envs/apps
/// (e.g. cold-start-unknown), call this once per env then call
/// [`cleanup_account`] once at the end.
pub async fn cleanup_env_and_app(
    user: &TestUserContext<BenchmarkTestDependencies>,
    env_id: &EnvironmentId,
) {
    let client = user.deps.registry_service().client(&user.token).await;
    let adapter = RegistryCleanupAdapter::new(client);
    cleanup_env_and_app_with(&adapter, &env_id.0).await;
}

/// Step 5: deletes the user account.
pub async fn cleanup_account(user: &TestUserContext<BenchmarkTestDependencies>) {
    let client = user.deps.registry_service().client(&user.token).await;
    let adapter = RegistryCleanupAdapter::new(client);
    cleanup_account_with(&adapter, &user.account_id.0).await;
}

/// Convenience wrapper for the common single-env-per-user case:
/// [`cleanup_env_and_app`] followed by [`cleanup_account`].
pub async fn cleanup_user_state(
    user: &TestUserContext<BenchmarkTestDependencies>,
    env_id: &EnvironmentId,
) {
    cleanup_env_and_app(user, env_id).await;
    cleanup_account(user).await;
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use test_r::test;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    /// In-process mock that records every operation attempted and fails the
    /// operations listed in `fail_ops`.
    pub struct MockCleanupClient {
        fail_ops: HashSet<&'static str>,
        /// Ordered log of every operation attempted.
        pub calls: Arc<Mutex<Vec<&'static str>>>,
        /// The `application_id` returned by `get_env_app_id_and_revision`
        /// (used to verify step-4 precondition propagation in tests).
        pub app_id: Uuid,
    }

    impl MockCleanupClient {
        pub fn new(fail_ops: &[&'static str]) -> (Self, Arc<Mutex<Vec<&'static str>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let mock = Self {
                fail_ops: fail_ops.iter().copied().collect(),
                calls: calls.clone(),
                app_id: Uuid::new_v4(),
            };
            (mock, calls)
        }

        fn record(&self, name: &'static str) {
            self.calls.lock().unwrap().push(name);
        }

        fn result(&self, name: &'static str) -> anyhow::Result<()> {
            self.record(name);
            if self.fail_ops.contains(name) {
                Err(anyhow::anyhow!("simulated failure in {name}"))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl CleanupClient for MockCleanupClient {
        async fn list_env_components(&self, _: &Uuid) -> anyhow::Result<Vec<(Uuid, u64)>> {
            self.record("list_env_components");
            if self.fail_ops.contains("list_env_components") {
                Err(anyhow::anyhow!("simulated failure"))
            } else {
                Ok(vec![(Uuid::new_v4(), 0)])
            }
        }

        async fn delete_component(&self, _: &Uuid, _: u64) -> anyhow::Result<()> {
            self.result("delete_component")
        }

        async fn list_env_domain_registrations(&self, _: &Uuid) -> anyhow::Result<Vec<Uuid>> {
            self.record("list_env_domain_registrations");
            if self.fail_ops.contains("list_env_domain_registrations") {
                Err(anyhow::anyhow!("simulated failure"))
            } else {
                Ok(vec![Uuid::new_v4()])
            }
        }

        async fn delete_domain_registration(&self, _: &Uuid) -> anyhow::Result<()> {
            self.result("delete_domain_registration")
        }

        async fn get_env_app_id_and_revision(&self, _: &Uuid) -> anyhow::Result<(Uuid, u64)> {
            self.record("get_env_app_id_and_revision");
            if self.fail_ops.contains("get_env_app_id_and_revision") {
                Err(anyhow::anyhow!("simulated failure"))
            } else {
                Ok((self.app_id, 1))
            }
        }

        async fn delete_environment(&self, _: &Uuid, _: u64) -> anyhow::Result<()> {
            self.result("delete_environment")
        }

        async fn get_application_revision(&self, _: &Uuid) -> anyhow::Result<u64> {
            self.record("get_application_revision");
            if self.fail_ops.contains("get_application_revision") {
                Err(anyhow::anyhow!("simulated failure"))
            } else {
                Ok(1)
            }
        }

        async fn delete_application(&self, _: &Uuid, _: u64) -> anyhow::Result<()> {
            self.result("delete_application")
        }

        async fn get_account_revision(&self, _: &Uuid) -> anyhow::Result<u64> {
            self.record("get_account_revision");
            if self.fail_ops.contains("get_account_revision") {
                Err(anyhow::anyhow!("simulated failure"))
            } else {
                Ok(1)
            }
        }

        async fn delete_account(&self, _: &Uuid, _: u64) -> anyhow::Result<()> {
            self.result("delete_account")
        }
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn all_ops() -> Vec<&'static str> {
        vec![
            "list_env_components",
            "delete_component",
            "list_env_domain_registrations",
            "delete_domain_registration",
            "get_env_app_id_and_revision",
            "delete_environment",
            "get_application_revision",
            "delete_application",
            "get_account_revision",
            "delete_account",
        ]
    }

    fn run(mock: &MockCleanupClient) {
        let env_id = Uuid::new_v4();
        let account_id = Uuid::new_v4();
        block_on(async {
            cleanup_env_and_app_with(mock, &env_id).await;
            cleanup_account_with(mock, &account_id).await;
        });
    }

    fn contains(calls: &[&str], op: &str) -> bool {
        calls.contains(&op)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn all_steps_run_on_success() {
        let (mock, calls) = MockCleanupClient::new(&[]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        for op in all_ops() {
            assert!(
                contains(&calls, op),
                "expected '{op}' to be called; got: {calls:?}"
            );
        }
    }

    #[test]
    fn step1_list_failure_continues() {
        let (mock, calls) = MockCleanupClient::new(&["list_env_components"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(
            contains(&calls, "list_env_domain_registrations"),
            "{calls:?}"
        );
        assert!(contains(&calls, "get_env_app_id_and_revision"), "{calls:?}");
        assert!(contains(&calls, "get_account_revision"), "{calls:?}");
    }

    #[test]
    fn step2_list_failure_continues() {
        let (mock, calls) = MockCleanupClient::new(&["list_env_domain_registrations"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(contains(&calls, "get_env_app_id_and_revision"), "{calls:?}");
        assert!(contains(&calls, "get_account_revision"), "{calls:?}");
    }

    /// `get_env_app_id_and_revision` (step 3 get) fails → step 4 is skipped
    /// (no app_id available) but step 5 still runs.
    #[test]
    fn step3_get_failure_skips_step4_runs_step5() {
        let (mock, calls) = MockCleanupClient::new(&["get_env_app_id_and_revision"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(
            !contains(&calls, "get_application_revision"),
            "step 4 must be skipped when step 3 get fails; got: {calls:?}"
        );
        assert!(
            contains(&calls, "get_account_revision"),
            "step 5 must still run; got: {calls:?}"
        );
    }

    /// `delete_environment` fails but get succeeded, so app_id is available:
    /// step 4 and step 5 both run.
    #[test]
    fn step3_delete_failure_still_runs_step4_and_step5() {
        let (mock, calls) = MockCleanupClient::new(&["delete_environment"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(contains(&calls, "get_application_revision"), "{calls:?}");
        assert!(contains(&calls, "get_account_revision"), "{calls:?}");
    }

    #[test]
    fn step4_failure_continues_to_step5() {
        let (mock, calls) = MockCleanupClient::new(&["get_application_revision"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(
            contains(&calls, "get_account_revision"),
            "step 5 should run after step 4 failure; got: {calls:?}"
        );
    }

    /// `get_account_revision` (step 5 get) fails → function completes without
    /// panic and `delete_account` is not attempted.
    #[test]
    fn step5_get_failure_no_delete_and_completes() {
        let (mock, calls) = MockCleanupClient::new(&["get_account_revision"]);
        run(&mock);
        let calls = calls.lock().unwrap().clone();
        assert!(contains(&calls, "get_account_revision"), "{calls:?}");
        assert!(
            !contains(&calls, "delete_account"),
            "delete_account must not run when get fails; got: {calls:?}"
        );
    }

    /// All steps fail simultaneously — function completes without panic and
    /// every unconditional step is attempted.
    #[test]
    fn all_steps_fail_no_short_circuit() {
        let (mock, calls) = MockCleanupClient::new(&all_ops());
        run(&mock); // must not panic
        let calls = calls.lock().unwrap().clone();
        assert!(contains(&calls, "list_env_components"), "{calls:?}");
        assert!(
            contains(&calls, "list_env_domain_registrations"),
            "{calls:?}"
        );
        assert!(contains(&calls, "get_env_app_id_and_revision"), "{calls:?}");
        assert!(contains(&calls, "get_account_revision"), "{calls:?}");
    }
}
