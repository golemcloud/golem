mod tool_lookup_tests {
    use super::*;
    use golem_common::model::tool::ToolSource;
    use golem_common::model::tool_middleware::CompiledToolMiddlewareChain;
    use test_r::test;

    #[test]
    async fn standalone_startup_failure_is_readable_by_public_id_and_tool_key() {
        use crate::api::invocation::{InvocationOutcome, InvocationState};
        use golem_api_grpc::proto::golem::workerexecutor::v1::{
            InvocationKind, InvocationLifecycleState,
        };
        use golem_common::model::{InvocationOwner, OwnedInvocationId, StandaloneToolInvocation};

        let mut harness = RestHarness::new(AgentMode::Durable);
        let component = harness
            .worker_service
            .component_service
            .get_revision(harness.component_id, harness.component_revision)
            .await
            .unwrap();
        let state = crate::service::component::tests::tool_deployment_state(
            &component,
            "weather",
            weather_tool_definition(),
            [ToolBindingOwner::ComponentBaseline {
                component_id: component.id,
            }],
        );
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
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let auth = scoped_invocation_auth();
        let key = RestHarness::tool_key();
        let tool = ToolName::try_from("weather").unwrap();
        let unique = StandaloneToolInvocation::unique_part(
            harness.environment_id,
            &tool,
            auth.idempotency_scope(),
            "current",
            Some(&key),
        );
        let id = OwnedInvocationId::new(InvocationOwner::Tool(tool.clone()), unique);
        let owner = AgentId::standalone_tool_invocation(
            harness.environment_id,
            TypeId::try_from(1u32).unwrap(),
            unique,
        );
        *harness.worker_client.invocation_summary.lock().unwrap() = Some(invocation_summary_proto(
            unique,
            InvocationLifecycleState::Failed,
            InvocationKind::Tool,
            Some("tool:weather/current".into()),
        ));
        *harness.worker_client.invocation_error.lock().unwrap() =
            Some(WorkerExecutorError::failed_to_resume_worker(
                owner,
                WorkerExecutorError::runtime("primary instantiation refused"),
            ));

        let got = harness
            .worker_service
            .get_invocation(harness.environment_id, &id, auth.clone())
            .await
            .unwrap();
        assert_eq!(got.summary.invocation_id, id.to_string());
        assert_eq!(got.summary.state, InvocationState::Failed);
        let Some(InvocationOutcome::Failure(failure)) = got.result else {
            panic!("the public response must contain the recorded startup failure");
        };
        assert!(
            failure.error.contains("primary instantiation refused"),
            "{}",
            failure.error
        );
        for command in [None, Some("current")] {
            let found = harness
                .worker_service
                .lookup_invocations(
                    harness.environment_id,
                    None,
                    &key,
                    None,
                    Some(&tool),
                    command,
                    auth.clone(),
                )
                .await
                .unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].invocation_id, id.to_string());
            assert_eq!(found[0].state, InvocationState::Failed);
        }
    }

    #[test]
    async fn removed_tool_invocations_use_retained_names() {
        let mut harness = RestHarness::new(AgentMode::Durable);
        let component = harness
            .worker_service
            .component_service
            .get_revision(harness.component_id, harness.component_revision)
            .await
            .unwrap();
        harness.worker_service.agent_types = agent_type_resolver(Arc::new(TestRegistryService {
            removed: true,
            provisional_tool_state: None,
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
                current_deployment_revision: None,
            },
        }));
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let auth = scoped_invocation_auth();
        let key = RestHarness::tool_key();
        let tool = ToolName::try_from("weather").unwrap();
        let unique = golem_common::model::StandaloneToolInvocation::unique_part(
            harness.environment_id,
            &tool,
            auth.idempotency_scope(),
            "current",
            Some(&key),
        );
        let id = golem_common::model::OwnedInvocationId::new(
            golem_common::model::InvocationOwner::Tool(tool.clone()),
            unique,
        );
        *harness.worker_client.invocation_summary.lock().unwrap() = Some(invocation_summary_proto(
            unique,
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState::Pending,
            golem_api_grpc::proto::golem::workerexecutor::v1::InvocationKind::Tool,
            Some("tool:weather/current".into()),
        ));
        let direct = harness
            .worker_service
            .get_invocation(harness.environment_id, &id, auth.clone())
            .await
            .unwrap();
        assert_eq!(direct.summary.invocation_id, id.to_string());
        harness
            .worker_service
            .cancel_invocation_by_id(harness.environment_id, &id, auth.clone())
            .await
            .unwrap();
        assert!(
            harness
                .worker_client
                .effects
                .lock()
                .unwrap()
                .contains(&"cancel-invocation")
        );
        let found = harness
            .worker_service
            .lookup_invocations(
                harness.environment_id,
                None,
                &key,
                None,
                Some(&tool),
                Some("current"),
                auth.clone(),
            )
            .await
            .unwrap();
        assert_eq!(found[0].invocation_id, id.to_string());
        let hosted = harness
            .worker_service
            .lookup_invocations(
                harness.environment_id,
                Some(&harness.keyed_agent_id()),
                &key,
                None,
                Some(&tool),
                Some("current"),
                auth.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            hosted[0].invocation_id,
            format!("{}.{}", harness.keyed_agent_id(), unique)
        );
        for agent in [None, Some(harness.keyed_agent_id())] {
            let error = harness
                .worker_service
                .lookup_invocations(
                    harness.environment_id,
                    agent.as_ref(),
                    &key,
                    None,
                    Some(&tool),
                    None,
                    auth.clone(),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, WorkerServiceError::TypeChecker(_)));
            assert!(error.to_string().contains("--command"));
        }
        let old_address = harness.worker_client.invocation_reads.lock().unwrap()[0].0;
        assert_eq!(
            old_address,
            AgentId::standalone_tool_invocation(
                harness.environment_id,
                1u32.try_into().unwrap(),
                unique
            )
        );
        let renamed = golem_common::model::OwnedInvocationId::new(
            golem_common::model::InvocationOwner::Tool(
                ToolName::try_from("renamed-weather").unwrap(),
            ),
            unique,
        );
        harness
            .worker_service
            .get_invocation(harness.environment_id, &renamed, auth.clone())
            .await
            .unwrap();
        assert_ne!(
            harness
                .worker_client
                .invocation_reads
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .0,
            old_address
        );
        let never = golem_common::model::OwnedInvocationId::new(
            golem_common::model::InvocationOwner::Tool(
                ToolName::try_from("never-existed").unwrap(),
            ),
            unique,
        );
        assert!(matches!(
            harness
                .worker_service
                .get_invocation(harness.environment_id, &never, auth)
                .await,
            Err(WorkerServiceError::InvocationNotFound(_))
        ));
    }

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
            [
                owner.clone(),
                ToolBindingOwner::AgentType {
                    agent_type_name: harness.agent_type_name.clone(),
                },
            ],
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
        presented.commands.nodes[1].aliases = vec!["today".into()];
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
        let agent_owner = ToolBindingOwner::AgentType {
            agent_type_name: harness.agent_type_name.clone(),
        };
        let mut agent_chain =
            state.tool_middleware_chains.values().next().unwrap()[&tool_name].clone();
        agent_chain.owner = agent_owner.clone();
        state.tool_middleware_chains.insert(
            agent_owner,
            BTreeMap::from([(tool_name.clone(), agent_chain)]),
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
        harness.worker_service.component_service = Arc::new(StaticComponentService {
            components: vec![component],
            tool_state: None,
            tool_queries: Mutex::new(vec![]),
            revision_gate: Mutex::new(None),
        });
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
        for command in [Some("forecast"), Some("today"), None] {
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
            let hosted = harness
                .worker_service
                .lookup_invocations(
                    harness.environment_id,
                    Some(&harness.keyed_agent_id()),
                    &key,
                    None,
                    Some(&tool_name),
                    command,
                    auth.clone(),
                )
                .await
                .unwrap();
            assert_eq!(hosted.len(), 1);
            assert_eq!(
                hosted[0].operation.as_deref(),
                Some("tool:weather/forecast")
            );
        }
    }
}
