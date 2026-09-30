use super::*;
use golem_common::model::agent::AgentMode;
use test_r::test;

macro_rules! pending_case {
    ($name:ident, $resource:ident, $mode:ident) => {
        #[test]
        #[timeout("2m")]
        async fn $name(
            last_unique_id: &LastUniqueId,
            deps: &WorkerExecutorTestDependencies,
            #[tagged_as("host_api_tests")] host_api_tests: &PrecompiledComponent,
            _tracing: &Tracing,
        ) -> anyhow::Result<()> {
            monthly_sleep::pending_sleep(
                last_unique_id,
                deps,
                host_api_tests,
                monthly_pending::Resource::$resource,
                AgentMode::$mode,
                monthly_sleep::Abi::P2,
            )
            .await
        }
    };
}

pending_case!(durable_p2_sleep_monthly_memory, Memory, Durable);
pending_case!(ephemeral_p2_sleep_monthly_memory, Memory, Ephemeral);
pending_case!(durable_p2_sleep_monthly_compute_prepaid, Compute, Durable);
pending_case!(
    ephemeral_p2_sleep_monthly_compute_prepaid,
    Compute,
    Ephemeral
);
pending_case!(durable_p2_sleep_monthly_scripted_storage, Storage, Durable);
pending_case!(
    ephemeral_p2_sleep_monthly_scripted_storage,
    Storage,
    Ephemeral
);
