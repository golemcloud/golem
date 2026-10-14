use super::*;
use test_r::{test, timeout};

#[derive(Debug, FromSchema)]
struct RustSdkTerminalObservation {
    success_bytes: Vec<u8>,
    success_result: bool,
    declared_bytes: Vec<u8>,
    declared_error: bool,
    failed_bytes: Vec<u8>,
    failed_terminal: String,
    failed_result: bool,
    cancellation_result: String,
    cancellation_closed_stdin: bool,
    abandoned_writer_bytes: Vec<u8>,
    abandoned_writer_result: bool,
    finish_failure_bytes: Vec<u8>,
    finish_failure_observed: bool,
    cancelled_reader_resumed_bytes: Vec<u8>,
    cancelled_reader_result: bool,
    dropped_observer_bytes: Vec<u8>,
}

#[test]
#[tracing::instrument]
#[timeout("3m")]
async fn gol40_rust_sdk_proxy_reflection_and_terminal_rows_use_real_components(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_provider")] provider: &PrecompiledComponent,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let executor = start_with_overrides(
        deps,
        &context,
        TestExecutorOverrides {
            environment_state_service: Some(environment_state.clone()),
            ..Default::default()
        },
    )
    .await?;
    let provider_component = executor
        .component_dep(&context.default_environment_id, provider)
        .store()
        .await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let provider_path = deps
        .component_directory
        .join(format!("{}.wasm", provider.wasm_name));
    let metadata = extract_component_metadata(&provider_path, false, true).await?;
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment_state(
            context.account_id,
            provider_component.id,
            provider_component.revision,
            "golem-it:tool-streaming-rust-provider",
            "ToolStreamingCaller",
            metadata.tools,
        )),
    );

    let agent_id = agent_id!("ToolStreamingCaller", "gol40-rust-sdk-conformance");
    let worker_id = executor
        .start_agent(&caller_component.id, agent_id.clone())
        .await?;
    let principal = Principal::Oidc(OidcPrincipal {
        sub: "gol40-rust".to_string(),
        issuer: "https://gol40.test".to_string(),
        email: None,
        name: None,
        email_verified: None,
        given_name: None,
        family_name: None,
        picture: None,
        preferred_username: None,
        claims: "{}".to_string(),
    });

    let proxy: Vec<String> = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            principal.clone(),
            "rust_proxy_generated_parity",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        proxy,
        [
            "rust",
            "artifact/inspect",
            "MATRIX.SAMPLE",
            "108",
            "south|east|north",
            "oidc:gol40-rust",
            "request.source",
            "unsupported source",
            "false",
        ]
    );

    let capable_path = "/gol40-rust-reflection-capability";
    let reflection: Vec<String> = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            principal.clone(),
            "rust_reflected_generated_parity",
            data_value!(capable_path),
        )
        .await?
        .into_typed()?;
    assert_eq!(reflection[0], "no-stream:reflection-parity");
    assert_eq!(reflection[1], reflection[0]);
    assert!(reflection[2].contains("Declared"), "{reflection:?}");
    assert!(reflection[3].contains("declared"), "{reflection:?}");
    assert_eq!(&reflection[4..], ["oidc", "oidc", "0", "0"]);
    assert_eq!(
        executor.get_file_contents(&worker_id, capable_path).await?,
        Vec::<u8>::new(),
        "generated and reflected clients must receive the named filesystem capability"
    );

    let terminals: RustSdkTerminalObservation = executor
        .invoke_and_await_agent_as_principal(
            &caller_component,
            &agent_id,
            principal,
            "gol40_rust_sdk_terminal_observation",
            data_value!(),
        )
        .await?
        .into_typed()?;
    assert_eq!(terminals.success_bytes, b"marker:");
    assert!(terminals.success_result);
    assert_eq!(terminals.declared_bytes, b"marker:");
    assert!(terminals.declared_error);
    assert_eq!(terminals.failed_bytes, b"marker:");
    assert_eq!(terminals.failed_terminal, "stream producer failed");
    assert!(terminals.failed_result);
    assert_eq!(
        terminals.cancellation_result,
        "Err(ToolRpcError::Cancelled)"
    );
    assert!(terminals.cancellation_closed_stdin);
    assert_eq!(terminals.abandoned_writer_bytes, b"marker:");
    assert!(terminals.abandoned_writer_result);
    assert_eq!(terminals.finish_failure_bytes, b"marker:");
    assert!(terminals.finish_failure_observed);
    assert_eq!(terminals.cancelled_reader_resumed_bytes, b"resumed");
    assert!(terminals.cancelled_reader_result);
    assert_eq!(terminals.dropped_observer_bytes, b"marker:detached");

    if let Some(active) = executor
        .active_entity_metadata(&OwnedAgentId::new(
            context.default_environment_id,
            &worker_id,
        ))
        .await
    {
        assert!(active.tool_operations.operations.is_empty());
        assert!(active.slots.iter().all(|slot| slot.invocations.is_empty()));
    }

    let (
        provider_checkpoint_port,
        provider_checkpoint_gate_port,
        provider_checkpoint_server,
        mut provider_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let (
        caller_checkpoint_port,
        caller_checkpoint_gate_port,
        caller_checkpoint_server,
        mut caller_checkpoint_arrivals,
    ) = start_crash_checkpoint_server().await;
    let exception_agent_id = agent_id!("ToolStreamingCaller", "gol40-rust-sdk-exception");
    let exception_worker_id = executor
        .start_agent_with(
            &caller_component.id,
            exception_agent_id.clone(),
            HashMap::from([
                (
                    "PROVIDER_CRASH_CHECKPOINT_PORT".to_string(),
                    provider_checkpoint_port.to_string(),
                ),
                (
                    "PROVIDER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    provider_checkpoint_gate_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_PORT".to_string(),
                    caller_checkpoint_port.to_string(),
                ),
                (
                    "CALLER_CRASH_CHECKPOINT_GATE_PORT".to_string(),
                    caller_checkpoint_gate_port.to_string(),
                ),
            ]),
            Vec::new(),
        )
        .await?;
    let exception_call = executor.invoke_and_await_agent(
        &caller_component,
        &exception_agent_id,
        "gol40_rust_sdk_exception",
        data_value!(),
    );
    let exception_gate = async {
        let provider_arrival = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            provider_checkpoint_arrivals.recv(),
        )
        .await
        .expect("provider clean-output checkpoint timed out")
        .expect("provider checkpoint server stopped");
        assert_eq!(provider_arrival.name, "provider-clean-stdout-before-trap");
        let caller_arrival = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            caller_checkpoint_arrivals.recv(),
        )
        .await
        .expect("exception byte observation timed out")
        .expect("caller checkpoint server stopped");
        assert_eq!(
            caller_arrival.name,
            "gol40-rust-sdk-exception-bytes-observed"
        );
        caller_arrival
            .release
            .send(())
            .expect("release exception byte observation");
        provider_arrival
            .release
            .send(())
            .expect("release provider exception");
    };
    let (exception, ()) = tokio::join!(exception_call, exception_gate);
    let exception = exception.expect_err("provider exception must fail the real caller invocation");
    assert!(
        format!("{exception:#}").contains("deterministic streaming tool trap"),
        "{exception:#}"
    );

    executor.delete_worker(&worker_id).await?;
    executor.delete_worker(&exception_worker_id).await?;
    provider_checkpoint_server.abort();
    caller_checkpoint_server.abort();
    Ok(())
}
