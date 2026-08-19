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

test_r::enable!();

#[test_r::sequential]
mod tests {
    use golem_client::api::RegistryServiceClient;
    use golem_common::agent_id;
    use golem_common::model::account_usage::BYTE_SECONDS_PER_GB_MONTH;
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_test_framework::config::{
        EnvBasedTestDependencies, EnvBasedTestDependenciesConfig, TestDependencies,
    };
    use golem_test_framework::dsl::{TestDsl, TestDslExtended};
    use golem_test_framework::model::IFSEntry;
    use std::path::PathBuf;
    use std::sync::Once;
    use std::time::Duration;
    use test_r::{test, timeout};

    static TRACING_INIT: Once = Once::new();

    fn init_tracing() {
        TRACING_INIT.call_once(|| {
            init_tracing_with_default_debug_env_filter(
                &TracingConfig::test_pretty_without_time("storage-billing").with_env_overrides(),
            );
        });
    }

    async fn create_deps() -> EnvBasedTestDependencies {
        init_tracing();
        let deps = EnvBasedTestDependencies::new(EnvBasedTestDependenciesConfig {
            worker_executor_cluster_size: 1,
            ..EnvBasedTestDependenciesConfig::new()
        })
        .await
        .expect("Failed constructing storage-billing test dependencies");

        let cluster = deps.worker_executor_cluster();
        cluster.kill_all().await;
        cluster
            .restart_all_with_extra_env_vars(vec![
                (
                    "GOLEM__RESOURCE_LIMITS__CONFIG__BATCH_UPDATE_INTERVAL".to_string(),
                    "200ms".to_string(),
                ),
                (
                    "GOLEM__RESOURCE_LIMITS__CONFIG__LIMIT_REFRESH_INTERVAL".to_string(),
                    "1s".to_string(),
                ),
            ])
            .await;

        deps
    }

    async fn durable_byte_seconds(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
    ) -> anyhow::Result<f64> {
        let usage = deps
            .registry_service()
            .client(&user.token)
            .await
            .get_account_storage_usage(&user.account_id.0, None)
            .await?;
        Ok(usage.usage.durable_storage_gb_month * BYTE_SECONDS_PER_GB_MONTH)
    }

    async fn wait_for_durable_billing_to_settle(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
    ) -> anyhow::Result<f64> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut last = durable_byte_seconds(deps, user).await?;
        let mut unchanged_since = tokio::time::Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let current = durable_byte_seconds(deps, user).await?;
            if current != last {
                last = current;
                unchanged_since = tokio::time::Instant::now();
            }
            if unchanged_since.elapsed() >= Duration::from_secs(1) {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("durable storage billing did not settle before timeout");
            }
        }
    }

    #[test]
    #[timeout("2m")]
    async fn unmanaged_filesystem_storage_is_unmetered() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "it_initial_file_system_release")
            .name("golem-it:initial-file-system")
            .unique()
            .with_files(
                "FileReadWrite",
                &[IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                }],
            )
            .store()
            .await?;

        let agent = agent_id!("FileReadWrite", "provisioned-storage-billing");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        let before = durable_byte_seconds(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        user.delete_worker(&worker).await?;
        let after = wait_for_durable_billing_to_settle(&deps, &user).await?;

        assert_eq!(after, before);
        Ok(())
    }
}
