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
    use golem_common::model::account_usage::BYTE_SECONDS_PER_GB_MONTH;
    use golem_common::model::component::{AgentFilePermissions, CanonicalFilePath};
    use golem_common::tracing::{TracingConfig, init_tracing_with_default_debug_env_filter};
    use golem_common::{agent_id, data_value};
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
                (
                    "GOLEM__FILESYSTEM_STORAGE__TOTAL_WORKER_FILESYSTEM_STORAGE_BYTES".to_string(),
                    // The eviction test writes 8-byte files. An 8-byte pool forces the first
                    // agent out before the second agent's filesystem can be loaded.
                    "8".to_string(),
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
        must_exceed: Option<f64>,
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
            if unchanged_since.elapsed() >= Duration::from_secs(1)
                && must_exceed.is_none_or(|minimum| current > minimum)
            {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("durable storage billing did not settle before timeout");
            }
        }
    }

    fn assert_billing_window(actual: f64, min: f64, max: f64, phase: &str) {
        assert!(
            (min..=max).contains(&actual),
            "unexpected storage billing during {phase}: expected {min}..={max} byte-seconds, got {actual}"
        );
    }

    #[test]
    #[timeout("2m")]
    async fn provisioned_read_write_file_is_metered() -> anyhow::Result<()> {
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
        let after = wait_for_durable_billing_to_settle(&deps, &user, Some(before)).await?;

        // Deleting the idle worker flushes its meter before the final snapshot. Four bytes over
        // two seconds should produce about eight byte-seconds without permitting overbilling.
        assert_billing_window(after - before, 4.0, 12.0, "provisioned-file metering");
        Ok(())
    }

    #[test]
    #[timeout("2m")]
    async fn evicted_agent_stops_metering_until_reload() -> anyhow::Result<()> {
        let deps = create_deps().await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .store()
            .await?;
        let agent_a = agent_id!("FileSystem", "storage-billing-a");
        let agent_b = agent_id!("FileSystem", "storage-billing-b");

        let worker_a = user.start_agent(&component.id, agent_a.clone()).await?;
        user.invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/metered.txt", "12345678"),
        )
        .await?;
        let active_start = durable_byte_seconds(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let active_end = durable_byte_seconds(&deps, &user).await?;

        let worker_b = user.start_agent(&component.id, agent_b.clone()).await?;
        user.invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/temporary.txt", "abcdefgh"),
        )
        .await?;
        user.invoke_and_await_agent(
            &component,
            &agent_b,
            "delete_file",
            data_value!("/temporary.txt"),
        )
        .await?;

        // The successful 8-byte write proves the full pool forced agent A out. Wait until any
        // final meter/drop batch arrives before opening the interval that must remain quiescent.
        let evicted_start = wait_for_durable_billing_to_settle(&deps, &user, None).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let evicted_end = durable_byte_seconds(&deps, &user).await?;

        user.invoke_and_await_agent(
            &component,
            &agent_a,
            "read_file",
            data_value!("/metered.txt"),
        )
        .await?;
        let reloaded_start = durable_byte_seconds(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        user.delete_worker(&worker_a).await?;
        let reloaded_end =
            wait_for_durable_billing_to_settle(&deps, &user, Some(reloaded_start)).await?;
        user.delete_worker(&worker_b).await?;

        // Eight bytes over two seconds should produce about sixteen byte-seconds. The window
        // covers asynchronous flush and batching latency while still rejecting overbilling.
        assert_billing_window(
            active_end - active_start,
            10.0,
            24.0,
            "pre-eviction metering",
        );
        // An evicted agent must stop accruing. The bound is 6.0 rather than something
        // tighter because eviction settling is asynchronous: at 8 bytes, 2.0 byte-seconds
        // is only a quarter-second of headroom, which a loaded CI runner can exceed
        // without anything being wrong. 6.0 still sits clearly below the 10.0 floor the
        // active windows assert, so an agent that kept billing after eviction fails.
        assert_billing_window(evicted_end - evicted_start, 0.0, 6.0, "evicted interval");
        assert_billing_window(
            reloaded_end - reloaded_start,
            10.0,
            24.0,
            "post-reload metering",
        );

        Ok(())
    }
}
