mod tool_lookup_tests {
    use super::*;
    use golem_common::model::tool::ToolSource;
    use golem_common::model::tool_middleware::CompiledToolMiddlewareChain;
    use test_r::test;

    #[test]
    async fn lookup_accepts_the_command_exposed_by_component_middleware() {
        let mut harness = RestHarness::new(AgentMode::Durable);
        let component = harness
            .worker_service
            .component_service
            .get_revision(harness.component_id, harness.component_revision)
            .await
            .unwrap();
        let owner = ToolBindingOwner::ComponentBaseline {
            component_id: component.id,
        };
        let tool_name = ToolName::try_from("weather").unwrap();
        let mut state = crate::service::component::tests::tool_deployment_state(
            &component,
            "weather",
            weather_tool_definition(),
            [owner.clone()],
        );
        let provider_id = ComponentId::new();
        let ToolSource::Component { component_id, .. } = &mut state
            .registered_tools
            .get_mut(&tool_name)
            .unwrap()
            .registration
            .source
        else {
            unreachable!()
        };
        *component_id = provider_id;
        assert_ne!(provider_id, component.id);
        let mut presented = weather_tool_definition();
        presented.commands.nodes[1].name = "forecast".into();
        presented.commands.nodes[1].aliases.clear();
        state.tool_middleware_chains.insert(
            owner.clone(),
            BTreeMap::from([(
                tool_name.clone(),
                CompiledToolMiddlewareChain {
                    deployment_revision: state.deployment_revision,
                    owner,
                    tool_name: tool_name.clone(),
                    effective_definition: presented,
                    occurrences: vec![],
                },
            )]),
        );
        harness.worker_service.component_service = Arc::new(StaticComponentService {
            components: vec![component.clone()],
            tool_state: Some(state.clone()),
            tool_queries: Mutex::new(vec![]),
            revision_gate: Mutex::new(None),
        });
        harness.worker_service.agent_types = agent_type_resolver(Arc::new(TestRegistryService {
            removed: false,
            provisional_tool_state: Some(state),
            resolved: ResolvedAgentType {
                registered_agent_type: RegisteredAgentType {
                    type_id: harness.type_id,
                    agent_type: component.metadata.agent_types()[0].clone(),
                    implemented_by: RegisteredAgentTypeImplementer {
                        component_id: component.id,
                        component_revision: component.revision,
                        component_name: component.component_name.0.clone(),
                        account_id: component.account_id,
                        account_email: component.account_email.clone(),
                    },
                },
                environment_id: harness.environment_id,
                deployment_revision: DeploymentRevision::INITIAL,
                current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
            },
        }));
        let auth = scoped_invocation_auth();
        let key = RestHarness::tool_key();
        let mut request = harness.tool_request();
        request.command_path = vec!["forecast".into()];
        request.mode = NativeToolInvocationMode::Schedule;
        let accepted = harness
            .worker_service
            .invoke_tool_rest(
                harness.environment_id,
                request,
                Some(key.clone()),
                auth.clone(),
            )
            .await
            .expect("the outward middleware command is callable");
        let id = golem_common::model::OwnedInvocationId::new(
            golem_common::model::InvocationOwner::Tool(tool_name.clone()),
            accepted.idempotency_key,
        );
        *harness.worker_client.invocation_summary.lock().unwrap() = Some(invocation_summary_proto(
            id.id(),
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState::Pending,
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationKind::Tool,
            Some("tool:weather/forecast".into()),
        ));
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let direct = harness
            .worker_service
            .get_invocation(harness.environment_id, &id, auth.clone())
            .await
            .expect("the invocation exists and is readable");
        assert_eq!(direct.summary.invocation_id, id.to_string());
        for command in [Some("forecast"), None] {
            let found = harness
                .worker_service
                .lookup_invocations(
                    harness.environment_id,
                    None,
                    &key,
                    None,
                    Some(&tool_name),
                    command,
                    auth.clone(),
                )
                .await
                .expect("lookup must accept the currently callable outward command");
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].invocation_id, id.to_string());
        }
    }
}
