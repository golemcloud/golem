use super::*;
use golem_common::model::agent_config::CanonicalAgentConfigPath;
use golem_common::model::worker::AgentConfigEntryDto;
use test_r::{test, timeout};

#[test]
#[timeout("3m")]
async fn chunk_f_native_config_uses_each_callers_effective_policy_without_host_broadening(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let mut streaming = native_streaming_tool_metadata();
    streaming.commands.nodes[0].name = "native-streaming".to_string();
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            native_tool_metadata: Some(streaming.clone()),
            ..Default::default()
        },
    )
    .await?;
    let configured = |prefix: &str| {
        vec![
            AgentConfigEntryDto {
                path: vec!["allowed".to_string()],
                value: serde_json::json!(format!("{prefix}-allowed")).into(),
            },
            AgentConfigEntryDto {
                path: vec!["denied".to_string()],
                value: serde_json::json!(format!("{prefix}-denied")).into(),
            },
        ]
    };
    let narrowed_component = executor
        .component_dep(&context.default_environment_id, caller)
        .unique()
        .name("chunk-f-native-config-narrowed")
        .with_agent_config("ToolStreamingCaller", configured("narrowed"))
        .store()
        .await?;
    let empty_component = executor
        .component_dep(&context.default_environment_id, caller)
        .unique()
        .name("chunk-f-native-config-empty")
        .with_agent_config("ToolStreamingCaller", configured("empty"))
        .store()
        .await?;
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: AgentTypeName("ToolStreamingCaller".to_string()),
    };
    let tool_name = ToolName::try_from("native-streaming").unwrap();
    for (component, scope) in [
        (
            &narrowed_component,
            ConfigKeyScope::Keys(BTreeSet::from([CanonicalAgentConfigPath(vec![
                "allowed".to_string(),
            ])])),
        ),
        (&empty_component, ConfigKeyScope::Keys(BTreeSet::new())),
    ] {
        let mut deployment = native_deployment_state(
            context.account_id,
            "ToolStreamingCaller",
            streaming.clone(),
            native_test_tool_metadata(),
        );
        deployment
            .tool_bindings
            .get_mut(&owner)
            .unwrap()
            .get_mut(&tool_name)
            .unwrap()
            .config_keys_readable = scope;
        environment_state.set_tool_deployment(
            context.default_environment_id,
            component.id,
            component.revision,
            Some(deployment),
        );
    }

    let narrowed = agent_id!("ToolStreamingCaller", "chunk-f-native-config-narrowed");
    executor
        .start_agent_with(
            &narrowed_component.id,
            narrowed.clone(),
            HashMap::new(),
            configured("override"),
        )
        .await?;
    for (key, expected) in [("allowed", "override-allowed"), ("denied", "denied")] {
        assert_eq!(
            executor
                .invoke_and_await_agent(
                    &narrowed_component,
                    &narrowed,
                    "native_config",
                    data_value!(key),
                )
                .await?
                .into_typed::<String>()?,
            expected
        );
    }

    let empty = agent_id!("ToolStreamingCaller", "chunk-f-native-config-empty");
    executor
        .start_agent_with(
            &empty_component.id,
            empty.clone(),
            HashMap::new(),
            configured("override"),
        )
        .await?;
    for key in ["allowed", "denied"] {
        assert_eq!(
            executor
                .invoke_and_await_agent(
                    &empty_component,
                    &empty,
                    "native_config",
                    data_value!(key),
                )
                .await?
                .into_typed::<String>()?,
            "denied"
        );
    }
    Ok(())
}
