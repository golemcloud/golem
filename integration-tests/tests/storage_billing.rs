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

    async fn create_deps(pool_bytes: u64, suspend_after: Duration) -> EnvBasedTestDependencies {
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
                    pool_bytes.to_string(),
                ),
                (
                    "GOLEM__SUSPEND__SUSPEND_AFTER".to_string(),
                    format!("{}ms", suspend_after.as_millis()),
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
            if unchanged_since.elapsed() >= Duration::from_secs(2) {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("durable storage billing did not settle before timeout");
            }
        }
    }

    async fn wait_for_durable_billing_increase_to_settle(
        deps: &EnvBasedTestDependencies,
        user: &golem_test_framework::config::dsl_impl::TestUserContext<EnvBasedTestDependencies>,
        baseline: f64,
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
            if current > baseline && unchanged_since.elapsed() >= Duration::from_secs(2) {
                return Ok(current);
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "durable storage billing did not increase and settle: baseline={baseline}, current={current}"
                );
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
    async fn provisioned_read_write_file_tracks_permit_window() -> anyhow::Result<()> {
        let deps = create_deps(1024, Duration::from_secs(5)).await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .unique()
            .with_files(
                "FileSystem",
                &[IFSEntry {
                    source_path: PathBuf::from("initial-file-system/files/baz.txt"),
                    target_path: CanonicalFilePath::from_abs_str("/bar/baz.txt").unwrap(),
                    permissions: AgentFilePermissions::ReadWrite,
                }],
            )
            .store()
            .await?;

        let agent = agent_id!("FileSystem", "provisioned-storage-billing");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        let idle_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let idle_end = wait_for_durable_billing_to_settle(&deps, &user).await?;
        assert_billing_window(
            idle_end - idle_start,
            0.0,
            1.0,
            "loaded-idle provisioned file",
        );

        user.invoke_and_await_agent(&component, &agent, "sleep_for", data_value!(2.0f64))
            .await?;
        let active_end =
            wait_for_durable_billing_increase_to_settle(&deps, &user, idle_end).await?;
        assert_billing_window(
            active_end - idle_end,
            4.0,
            12.0,
            "permit-retaining provisioned file",
        );
        user.delete_worker(&worker).await?;
        Ok(())
    }

    #[test]
    #[timeout("2m")]
    async fn evicted_agent_stops_metering_until_reload() -> anyhow::Result<()> {
        const FILE_BYTES: usize = 1024 * 1024;
        let deps = create_deps(FILE_BYTES as u64, Duration::from_secs(5)).await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .store()
            .await?;
        let agent_a = agent_id!("FileSystem", "storage-billing-eviction-a");
        let agent_b = agent_id!("FileSystem", "storage-billing-eviction-b");
        let worker_a = user.start_agent(&component.id, agent_a.clone()).await?;
        user.invoke_and_await_agent(
            &component,
            &agent_a,
            "write_file",
            data_value!("/metered.txt", "a".repeat(FILE_BYTES)),
        )
        .await?;

        let active_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        user.invoke_and_await_agent(&component, &agent_a, "sleep_for", data_value!(0.75f64))
            .await?;
        let active_end =
            wait_for_durable_billing_increase_to_settle(&deps, &user, active_start).await?;
        assert_billing_window(
            active_end - active_start,
            512.0 * 1024.0,
            1.5 * 1024.0 * 1024.0,
            "pre-eviction permit window",
        );

        let worker_b = user.start_agent(&component.id, agent_b.clone()).await?;
        user.invoke_and_await_agent(
            &component,
            &agent_b,
            "write_file",
            data_value!("/temporary.txt", "b".repeat(FILE_BYTES)),
        )
        .await?;
        user.invoke_and_await_agent(
            &component,
            &agent_b,
            "delete_file",
            data_value!("/temporary.txt"),
        )
        .await?;
        let evicted_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        let evicted_end = wait_for_durable_billing_to_settle(&deps, &user).await?;
        assert_billing_window(evicted_end - evicted_start, 0.0, 1.0, "evicted interval");

        user.invoke_and_await_agent(
            &component,
            &agent_a,
            "read_file",
            data_value!("/metered.txt"),
        )
        .await?;
        let reloaded_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        user.invoke_and_await_agent(&component, &agent_a, "sleep_for", data_value!(0.75f64))
            .await?;
        let reloaded_end =
            wait_for_durable_billing_increase_to_settle(&deps, &user, reloaded_start).await?;
        assert_billing_window(
            reloaded_end - reloaded_start,
            512.0 * 1024.0,
            1.5 * 1024.0 * 1024.0,
            "post-reload permit window",
        );

        user.delete_worker(&worker_a).await?;
        user.delete_worker(&worker_b).await?;

        Ok(())
    }

    #[test]
    #[timeout("2m")]
    async fn durable_suspension_pauses_storage_billing() -> anyhow::Result<()> {
        const FILE_BYTES: usize = 1024 * 1024;
        let deps = create_deps(2 * FILE_BYTES as u64, Duration::from_secs(1)).await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .store()
            .await?;
        let agent = agent_id!("FileSystem", "storage-billing-suspension");
        let worker = user.start_agent(&component.id, agent.clone()).await?;
        user.invoke_and_await_agent(
            &component,
            &agent,
            "write_file",
            data_value!("/metered.txt", "x".repeat(FILE_BYTES)),
        )
        .await?;

        let active_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        user.invoke_and_await_agent(&component, &agent, "sleep_for", data_value!(0.5f64))
            .await?;
        let active_end =
            wait_for_durable_billing_increase_to_settle(&deps, &user, active_start).await?;
        assert_billing_window(
            active_end - active_start,
            256.0 * 1024.0,
            1024.0 * 1024.0,
            "permit-retaining host wait",
        );

        let suspended_start = active_end;
        user.invoke_and_await_agent(&component, &agent, "sleep_for", data_value!(3.0f64))
            .await?;
        let suspended_end =
            wait_for_durable_billing_increase_to_settle(&deps, &user, suspended_start).await?;
        assert_billing_window(
            suspended_end - suspended_start,
            512.0 * 1024.0,
            1.75 * 1024.0 * 1024.0,
            "durable suspension",
        );

        user.delete_worker(&worker).await?;

        Ok(())
    }

    #[test]
    #[timeout("2m")]
    async fn p2_and_p3_mutations_produce_matching_billing() -> anyhow::Result<()> {
        const FILE_BYTES: usize = 1024 * 1024;
        let deps = create_deps(2 * FILE_BYTES as u64, Duration::from_secs(5)).await;
        let user = deps.user().await?;
        let (_, env) = user.app_and_env().await?;
        let p2_component = user
            .component(&env.id, "golem_it_host_api_tests_release")
            .store()
            .await?;
        let p3_component = user
            .component(&env.id, "it_initial_file_system_release")
            .name("golem-it:initial-file-system")
            .unique()
            .store()
            .await?;

        let p2_agent = agent_id!("FileSystem", "p2-storage-billing");
        user.invoke_and_await_agent(
            &p2_component,
            &p2_agent,
            "write_file",
            data_value!("/metered.txt", "x".repeat(FILE_BYTES)),
        )
        .await?;
        let p2_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        user.invoke_and_await_agent(&p2_component, &p2_agent, "sleep_for", data_value!(0.5f64))
            .await?;
        let p2_end = wait_for_durable_billing_increase_to_settle(&deps, &user, p2_start).await?;
        let p2_billing = p2_end - p2_start;

        let p3_agent = agent_id!("P3FileSystem", "p3-storage-billing");
        user.invoke_and_await_agent(
            &p3_component,
            &p3_agent,
            "write_bytes",
            data_value!("p3-metered.txt", FILE_BYTES as u64),
        )
        .await?;
        let p3_start = wait_for_durable_billing_to_settle(&deps, &user).await?;
        user.invoke_and_await_agent(&p3_component, &p3_agent, "sleep_for", data_value!(0.5f64))
            .await?;
        let p3_end = wait_for_durable_billing_increase_to_settle(&deps, &user, p3_start).await?;
        let p3_billing = p3_end - p3_start;

        assert_billing_window(
            p2_billing,
            256.0 * 1024.0,
            1024.0 * 1024.0,
            "P2 mutation billing",
        );
        assert_billing_window(
            p3_billing,
            256.0 * 1024.0,
            1024.0 * 1024.0,
            "P3 mutation billing",
        );
        let ratio = p2_billing / p3_billing;
        assert!(
            (0.75..=1.333_334).contains(&ratio),
            "P2/P3 billing should match for equal bytes and permit windows: P2={p2_billing}, P3={p3_billing}"
        );
        Ok(())
    }
}
