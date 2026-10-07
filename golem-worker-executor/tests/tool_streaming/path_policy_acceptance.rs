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

use super::*;
use test_r::test;

inherit_test_dep!(WorkerExecutorTestDependencies);
inherit_test_dep!(LastUniqueId);
inherit_test_dep!(Tracing);
inherit_test_dep!(
    #[tagged_as("tool_streaming_rust_caller")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("filesystem_tools")]
    PrecompiledComponent
);
inherit_test_dep!(
    #[tagged_as("audit_middleware")]
    PrecompiledComponent
);

const PATH_POLICY_NAME: &str = "path-policy";

#[derive(Clone, Default)]
struct PathPolicyAuditSink {
    committed: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

async fn start_path_policy_audit_sink() -> (String, PathPolicyAuditSink, tokio::task::JoinHandle<()>)
{
    async fn commit(
        State(state): State<PathPolicyAuditSink>,
        axum::Json(record): axum::Json<serde_json::Value>,
    ) -> axum::http::StatusCode {
        state.committed.lock().unwrap().push(record);
        axum::http::StatusCode::NO_CONTENT
    }

    let state = PathPolicyAuditSink::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/records", listener.local_addr().unwrap());
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/records", post(commit))
                .with_state(task_state),
        )
        .await
        .expect("serve path-policy audit sink");
    });
    (url, state, task)
}

async fn extract_path_policy_metadata(
    deps: &WorkerExecutorTestDependencies,
    filesystem_tools: &PrecompiledComponent,
    audit: &PrecompiledComponent,
) -> anyhow::Result<(
    golem_common::model::agent::extraction::ExtractedComponentMetadata,
    golem_common::model::agent::extraction::ExtractedComponentMetadata,
)> {
    let filesystem_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", filesystem_tools.wasm_name)),
        false,
        true,
    )
    .await?;
    if !filesystem_metadata
        .tool_middlewares
        .iter()
        .any(|definition| definition.name == PATH_POLICY_NAME)
    {
        anyhow::bail!(
            "GOL-605 dependency missing: component '{}' does not export universal middleware '{}@0.1.0'",
            filesystem_tools.wasm_name,
            PATH_POLICY_NAME
        );
    }
    let audit_metadata = extract_component_metadata(
        &deps
            .component_directory
            .join(format!("{}.wasm", audit.wasm_name)),
        false,
        true,
    )
    .await?;
    Ok((filesystem_metadata, audit_metadata))
}

fn path_policy_parameters(
    definition: &ToolMiddleware,
    base: &str,
    allowed_root: &str,
) -> TypedSchemaValue {
    TypedSchemaValue::new(
        definition.parameter_schema.clone(),
        SchemaValue::Record {
            fields: vec![
                SchemaValue::String(base.to_string()),
                SchemaValue::List {
                    elements: vec![SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String(allowed_root.to_string()),
                            SchemaValue::List {
                                elements: vec![
                                    SchemaValue::Enum { case: 0 },
                                    SchemaValue::Enum { case: 1 },
                                ],
                            },
                        ],
                    }],
                },
            ],
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn install_audit_path_policy_chain(
    deployment: &mut ToolDeploymentState,
    agent_type: &AgentTypeName,
    tool_name: &str,
    audit_component: &golem_common::model::component::ComponentDto,
    audit_definitions: &[ToolMiddleware],
    audit_sink_url: &str,
    filesystem_component: &golem_common::model::component::ComponentDto,
    filesystem_middlewares: &[ToolMiddleware],
    base: &str,
    allowed_root: &str,
) {
    let tool_name = ToolName::try_from(tool_name).unwrap();
    let audit = audit_definitions
        .iter()
        .find(|definition| definition.name == "audit")
        .expect("shipped audit middleware metadata");
    let path_policy = filesystem_middlewares
        .iter()
        .find(|definition| definition.name == PATH_POLICY_NAME)
        .expect("GOL-605 path-policy middleware metadata");

    install_middleware_chain(
        deployment,
        agent_type,
        &tool_name,
        audit_component.id,
        audit_component.revision,
        "golem:audit-middleware",
        audit_definitions,
        vec![(
            audit.name.as_str(),
            audit_middleware_parameters(audit, "path-policy-acceptance", audit_sink_url),
        )],
    );
    let owner = ToolBindingOwner::AgentType {
        agent_type_name: agent_type.clone(),
    };
    let audit_occurrence = deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .remove(&tool_name)
        .unwrap()
        .occurrences
        .into_iter()
        .next()
        .unwrap();

    install_middleware_chain(
        deployment,
        agent_type,
        &tool_name,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        filesystem_middlewares,
        vec![(
            path_policy.name.as_str(),
            path_policy_parameters(path_policy, base, allowed_root),
        )],
    );
    let chain = deployment
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    chain.occurrences[0].filesystem_access = ToolFilesystemAccess::Allowed;
    chain.occurrences.insert(0, audit_occurrence);
    deployment
        .tool_bindings
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .filesystem_access = ToolFilesystemAccess::Allowed;
}

fn write_input(path: &str, content: &str) -> TypedSchemaValue {
    filesystem_tool_input(vec![
        (
            "path",
            SchemaType::string(),
            SchemaValue::String(path.to_string()),
        ),
        (
            "content",
            SchemaType::string(),
            SchemaValue::String(content.to_string()),
        ),
        (
            "create-parent-directories",
            SchemaType::bool(),
            SchemaValue::Bool(true),
        ),
    ])
}

fn read_input(path: &str) -> TypedSchemaValue {
    filesystem_tool_input(vec![
        (
            "path",
            SchemaType::string(),
            SchemaValue::String(path.to_string()),
        ),
        {
            let (schema, value) = filesystem_optional_line(None);
            ("start-line", schema, value)
        },
        {
            let (schema, value) = filesystem_optional_line(None);
            ("end-line", schema, value)
        },
        {
            let (schema, value) = filesystem_cursor_none();
            ("cursor", schema, value)
        },
    ])
}

fn assert_path_policy_denied(
    result: Result<Option<SchemaValue>, SerializableToolRpcError>,
    expected_operation_case: u32,
    expected_supplied_path: &str,
) -> anyhow::Result<()> {
    let Err(SerializableToolRpcError::RemoteToolError(error)) = result else {
        anyhow::bail!("expected path-policy denial, got {result:?}");
    };
    let SerializableToolError::CustomError(error) = error.as_ref() else {
        anyhow::bail!("expected path-policy custom error, got {error:?}");
    };
    assert_eq!(error.name, "path-policy-denied");
    let SchemaValue::Record { fields } = error.payload.value() else {
        anyhow::bail!("path-policy-denied payload must be a record");
    };
    assert_eq!(
        fields[0],
        SchemaValue::Enum {
            case: expected_operation_case
        }
    );
    assert_eq!(fields[1], SchemaValue::String("path".to_string()));
    assert_eq!(
        fields[2],
        SchemaValue::String(expected_supplied_path.to_string())
    );
    assert!(matches!(fields[3], SchemaValue::Option { inner: Some(_) }));
    assert!(matches!(fields[4], SchemaValue::List { .. }));
    assert!(matches!(fields[5], SchemaValue::String(_)));
    Ok(())
}

fn filesystem_definitions(
    metadata: &golem_common::model::agent::extraction::ExtractedComponentMetadata,
) -> BTreeMap<ToolName, golem_common::schema::tool::Tool> {
    metadata
        .tools
        .iter()
        .map(|definition| {
            let name = definition
                .name()
                .expect("filesystem tool has a root command");
            (ToolName::try_from(name).unwrap(), definition.clone())
        })
        .collect()
}

fn assert_read_content(value: SchemaValue, expected: &str) {
    let SchemaValue::Record { fields } = value else {
        panic!("read-file result must be a record");
    };
    assert_eq!(fields[0], SchemaValue::String(expected.to_string()));
}

#[test]
#[timeout("5m")]
async fn named_fork_replays_guest_tool_calls_with_path_policy(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_named_fork_tool_replay(
        last_unique_id,
        deps,
        caller,
        filesystem_tools,
        audit,
        false,
        false,
    )
    .await
}

#[test]
#[timeout("5m")]
async fn named_fork_replays_guest_tool_calls_with_path_policy_and_audit(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_named_fork_tool_replay(
        last_unique_id,
        deps,
        caller,
        filesystem_tools,
        audit,
        true,
        false,
    )
    .await
}

#[test]
#[timeout("5m")]
async fn named_fork_replays_guest_tool_calls_with_path_policy_at_incomplete_start(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    exercise_named_fork_tool_replay(
        last_unique_id,
        deps,
        caller,
        filesystem_tools,
        audit,
        false,
        true,
    )
    .await
}

async fn exercise_named_fork_tool_replay(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    caller: &PrecompiledComponent,
    filesystem_tools: &PrecompiledComponent,
    audit: &PrecompiledComponent,
    audited: bool,
    incomplete: bool,
) -> anyhow::Result<()> {
    use crate::fork::start_with_local_resume_and_overrides;
    use golem_common::model::AgentId;
    use golem_worker_executor::services::golem_config::SnapshotPolicy;

    let (filesystem_metadata, audit_metadata) =
        extract_path_policy_metadata(deps, filesystem_tools, audit).await?;
    let (audit_sink_url, audit_sink, audit_sink_task) = start_path_policy_audit_sink().await;
    let context = TestContext::new(last_unique_id);
    let environment_state = Arc::new(TestEnvironmentStateService::default());
    let overrides = || TestExecutorOverrides {
        environment_state_service: Some(environment_state.clone()),
        configure: Some(Arc::new(move |config| {
            config.oplog.default_snapshotting = SnapshotPolicy::Disabled;
            if !audited {
                config.oplog.max_payload_size = 1;
            }
        })),
        ..Default::default()
    };
    let executor = start_with_local_resume_and_overrides(deps, &context, overrides()).await?;
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem_tools)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let mut deployment = deployment_state(
        context.account_id,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        agent_type.0.as_str(),
        filesystem_metadata.tools.clone(),
    );
    for tool_name in ["write-file", "read-file"] {
        install_audit_path_policy_chain(
            &mut deployment,
            &agent_type,
            tool_name,
            &audit_component,
            &audit_metadata.tool_middlewares,
            &audit_sink_url,
            &filesystem_component,
            &filesystem_metadata.tool_middlewares,
            "/workspace",
            "allowed",
        );
        if !audited {
            deployment
                .tool_middleware_chains
                .get_mut(&ToolBindingOwner::AgentType {
                    agent_type_name: agent_type.clone(),
                })
                .unwrap()
                .get_mut(&ToolName::try_from(tool_name).unwrap())
                .unwrap()
                .occurrences
                .remove(0);
        }
    }
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );
    let parent_name = agent_id!("ToolStreamingCaller", "fork-parent");
    let parent = executor
        .start_agent(&caller_component.id, parent_name.clone())
        .await?;
    let content = "parent checkpoint bytes";
    let evidence: Result<Vec<String>, String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &parent_name,
            "filesystem_policy_write_read",
            data_value!("allowed/state.txt".to_string(), content.to_string()),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        evidence.map_err(anyhow::Error::msg)?,
        vec!["created", "23", content, "1", "1", "none"]
    );
    let audit_calls_per_tool = usize::from(audited);
    assert_eq!(
        audit_sink.committed.lock().unwrap().len(),
        2 * audit_calls_per_tool
    );
    let original_audit_records = audit_sink.committed.lock().unwrap().clone();
    let history = executor.get_oplog(&parent, OplogIndex::INITIAL).await?;
    let cut = if incomplete {
        history.iter().find(|entry| matches!(&entry.entry, PublicOplogEntry::Start(start) if start.function_name == "golem::entity::invoke"))
            .expect("first entity Start").oplog_index
    } else {
        history
            .last()
            .expect("completed parent history")
            .oplog_index
    };
    assert!(
        history
            .iter()
            .any(|entry| matches!(entry.entry, PublicOplogEntry::Start(_)))
    );

    let child_name = agent_id!("ToolStreamingCaller", "fork-child");
    let child = AgentId {
        component_id: caller_component.id,
        agent_id: child_name.to_string(),
    };
    executor.fork_worker(&parent, &child.agent_id, cut).await?;
    let copied = executor.get_oplog(&child, OplogIndex::INITIAL).await?;
    for entry in history.iter().filter(|entry| entry.oplog_index <= cut) {
        if let PublicOplogEntry::Start(start) = &entry.entry {
            let copy = copied
                .iter()
                .find(|copy| copy.oplog_index == entry.oplog_index)
                .unwrap();
            let PublicOplogEntry::Start(copied_start) = &copy.entry else {
                panic!("fork changed a Start's entry kind");
            };
            assert_eq!(copied_start.function_name, start.function_name);
            assert_eq!(copied_start.parent_start_index, start.parent_start_index);
            if start.function_name == "golem::entity::invoke" {
                assert_eq!(copied_start.request, start.request);
            }
        }
    }
    let restored: Result<String, String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &child_name,
            "filesystem_policy_read",
            data_value!("allowed/state.txt".to_string()),
        )
        .await?
        .into_typed()?;
    assert_eq!(restored.map_err(anyhow::Error::msg)?, content);
    assert_eq!(
        executor
            .get_file_contents(&child, "/workspace/allowed/state.txt")
            .await?,
        content.as_bytes()
    );
    assert_eq!(
        audit_sink.committed.lock().unwrap().len(),
        3 * audit_calls_per_tool,
        "completed replay must not repeat audit HTTP effects"
    );

    let edited: Result<Vec<String>, String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &child_name,
            "filesystem_policy_write_read",
            data_value!(
                "allowed/state.txt".to_string(),
                "child-only edit".to_string()
            ),
        )
        .await?
        .into_typed()?;
    assert_eq!(
        edited.map_err(anyhow::Error::msg)?,
        vec!["replaced", "15", "child-only edit", "1", "1", "none"]
    );
    assert_eq!(
        executor
            .get_file_contents(&parent, "/workspace/allowed/state.txt")
            .await?,
        content.as_bytes()
    );
    assert_eq!(
        audit_sink.committed.lock().unwrap().len(),
        5 * audit_calls_per_tool
    );
    drop(executor);

    let executor = start_with_local_resume_and_overrides(deps, &context, overrides()).await?;
    let recovered: Result<String, String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &child_name,
            "filesystem_policy_read",
            data_value!("allowed/state.txt".to_string()),
        )
        .await?
        .into_typed()?;
    assert_eq!(recovered.map_err(anyhow::Error::msg)?, "child-only edit");
    assert_eq!(
        audit_sink.committed.lock().unwrap().len(),
        6 * audit_calls_per_tool,
        "restart must not repeat recorded audit effects"
    );
    let child_history = executor.get_oplog(&child, OplogIndex::INITIAL).await?;
    let grandchild_name = agent_id!("ToolStreamingCaller", "fork-grandchild");
    let grandchild = AgentId {
        component_id: caller_component.id,
        agent_id: grandchild_name.to_string(),
    };
    executor
        .fork_worker(
            &child,
            &grandchild.agent_id,
            child_history.last().unwrap().oplog_index,
        )
        .await?;
    let inherited: Result<String, String> = executor
        .invoke_and_await_agent(
            &caller_component,
            &grandchild_name,
            "filesystem_policy_read",
            data_value!("allowed/state.txt".to_string()),
        )
        .await?
        .into_typed()?;
    assert_eq!(inherited.map_err(anyhow::Error::msg)?, "child-only edit");
    let records = audit_sink.committed.lock().unwrap();
    assert_eq!(records.len(), 7 * audit_calls_per_tool);
    assert_eq!(
        &records[..original_audit_records.len()],
        original_audit_records.as_slice()
    );
    if audited {
        for record in &records[..2] {
            assert_eq!(record["owner"]["agentId"], parent.agent_id);
        }
        for record in &records[2..6] {
            assert_eq!(record["owner"]["agentId"], child.agent_id);
        }
        assert_eq!(records[6]["owner"]["agentId"], grandchild.agent_id);
    }
    drop(records);
    audit_sink_task.abort();
    Ok(())
}

async fn unload_path_policy_owner(
    executor: &TestWorkerExecutor,
    environment_id: golem_common::model::environment::EnvironmentId,
    worker_id: &golem_common::model::AgentId,
) -> anyhow::Result<()> {
    executor
        .wait_for_status(
            worker_id,
            AgentStatus::Idle,
            std::time::Duration::from_secs(30),
        )
        .await?;
    let owner = OwnedAgentId::new(environment_id, worker_id);
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while executor.worker_eviction_class(&owner).await
            != Some(golem_worker_executor::worker::EvictionClass::LoadedIdle)
        {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("path-policy owner did not become loaded-idle"))?;
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            if executor.stop_worker_if_idle(&owner).await? {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("path-policy owner sidecars did not become disposable"))??;
    executor.retire_unloaded_worker(&owner).await?;
    assert!(!executor.worker_is_cached(&owner).await);
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn path_1_audit_path_policy_real_file_tool_allows_and_denies(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (filesystem_metadata, audit_metadata) =
        extract_path_policy_metadata(deps, filesystem_tools, audit).await?;
    let (audit_sink_url, audit_sink, audit_sink_task) = start_path_policy_audit_sink().await;
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
    let caller_component = executor
        .component_dep(&context.default_environment_id, caller)
        .store()
        .await?;
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem_tools)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let mut deployment = deployment_state(
        context.account_id,
        filesystem_component.id,
        filesystem_component.revision,
        "golem:filesystem-tools",
        agent_type.0.as_str(),
        filesystem_metadata.tools.clone(),
    );
    install_audit_path_policy_chain(
        &mut deployment,
        &agent_type,
        "write-file",
        &audit_component,
        &audit_metadata.tool_middlewares,
        &audit_sink_url,
        &filesystem_component,
        &filesystem_metadata.tool_middlewares,
        "/workspace",
        "allowed",
    );
    environment_state.set_tool_deployment(
        context.default_environment_id,
        caller_component.id,
        caller_component.revision,
        Some(deployment),
    );

    let worker_id = executor
        .start_agent(
            &caller_component.id,
            agent_id!("ToolStreamingCaller", "path-1-audit-policy"),
        )
        .await?;
    let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
    let principal = Principal::GolemUser(GolemUserPrincipal {
        account_id: context.account_id,
    });
    let definitions = filesystem_definitions(&filesystem_metadata);
    let allowed_content = "allowed-content-must-not-enter-audit";
    let allowed = invoke_filesystem_tool_success(
        &executor,
        &worker_id,
        fingerprint,
        principal.clone(),
        &definitions,
        "write-file",
        write_input("allowed/result.txt", allowed_content),
    )
    .await?;
    assert_eq!(
        allowed,
        SchemaValue::Record {
            fields: vec![
                SchemaValue::Enum { case: 0 },
                SchemaValue::U64(allowed_content.len() as u64)
            ]
        }
    );
    assert_eq!(
        executor
            .get_file_contents(&worker_id, "/workspace/allowed/result.txt")
            .await?,
        allowed_content.as_bytes()
    );

    let denied_path = "denied/result.txt";
    let denied = invoke_filesystem_tool(
        &executor,
        &worker_id,
        fingerprint,
        principal,
        &definitions,
        "write-file",
        write_input(denied_path, "denied-content-must-not-be-written"),
    )
    .await?;
    assert_path_policy_denied(denied, 1, denied_path)?;
    assert!(
        executor
            .get_file_contents(&worker_id, "/workspace/denied/result.txt")
            .await
            .is_err(),
        "denied invocation must produce zero leaf filesystem effects"
    );

    let records = audit_sink.committed.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record["outcome"]["kind"] == "success")
    );
    assert!(records.iter().any(|record| {
        record["outcome"]["kind"] == "error"
            && record["outcome"]["customName"] == "path-policy-denied"
    }));
    let serialized = serde_json::to_string(&*records)?;
    assert!(!serialized.contains(allowed_content));
    assert!(!serialized.contains("denied-content-must-not-be-written"));
    drop(records);
    audit_sink_task.abort();
    Ok(())
}

#[test]
#[tracing::instrument]
#[timeout("5m")]
async fn path_2_two_owner_roots_remain_isolated_after_reconstruction(
    last_unique_id: &LastUniqueId,
    deps: &WorkerExecutorTestDependencies,
    #[tagged_as("tool_streaming_rust_caller")] caller: &PrecompiledComponent,
    #[tagged_as("filesystem_tools")] filesystem_tools: &PrecompiledComponent,
    #[tagged_as("audit_middleware")] audit: &PrecompiledComponent,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    let (filesystem_metadata, audit_metadata) =
        extract_path_policy_metadata(deps, filesystem_tools, audit).await?;
    let (audit_sink_url, audit_sink, audit_sink_task) = start_path_policy_audit_sink().await;
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
    let definitions = filesystem_definitions(&filesystem_metadata);
    let agent_type = AgentTypeName("ToolStreamingCaller".to_string());
    let filesystem_component = executor
        .component_dep(&context.default_environment_id, filesystem_tools)
        .store()
        .await?;
    let audit_component = executor
        .component_dep(&context.default_environment_id, audit)
        .store()
        .await?;
    let mut owners = Vec::new();
    for (owner_name, base, content) in [
        ("path-owner-a", "/workspace/owner-a", "owner-a-content"),
        ("path-owner-b", "/workspace/owner-b", "owner-b-content"),
    ] {
        let caller_component_name = format!("golem-it:{owner_name}");
        let caller_component = executor
            .component_dep(&context.default_environment_id, caller)
            .name(&caller_component_name)
            .unique()
            .store()
            .await?;
        let mut deployment = deployment_state(
            context.account_id,
            filesystem_component.id,
            filesystem_component.revision,
            "golem:filesystem-tools",
            agent_type.0.as_str(),
            filesystem_metadata.tools.clone(),
        );
        for tool_name in ["write-file", "read-file"] {
            install_audit_path_policy_chain(
                &mut deployment,
                &agent_type,
                tool_name,
                &audit_component,
                &audit_metadata.tool_middlewares,
                &audit_sink_url,
                &filesystem_component,
                &filesystem_metadata.tool_middlewares,
                base,
                "durable",
            );
        }
        environment_state.set_tool_deployment(
            context.default_environment_id,
            caller_component.id,
            caller_component.revision,
            Some(deployment),
        );
        let worker_id = executor
            .start_agent(
                &caller_component.id,
                agent_id!("ToolStreamingCaller", owner_name),
            )
            .await?;
        let fingerprint = executor.get_worker_metadata(&worker_id).await?.fingerprint;
        let principal = Principal::GolemUser(GolemUserPrincipal {
            account_id: context.account_id,
        });
        invoke_filesystem_tool_success(
            &executor,
            &worker_id,
            fingerprint,
            principal.clone(),
            &definitions,
            "write-file",
            write_input("durable/state.txt", content),
        )
        .await?;
        owners.push((
            context.default_environment_id,
            worker_id,
            fingerprint,
            principal,
            owner_name.to_string(),
            base.to_string(),
            content.to_string(),
        ));
    }

    for reconstruction_pass in [false, true] {
        for index in [1_usize, 0_usize] {
            let (_environment_id, worker_id, fingerprint, principal, _owner_name, base, content) =
                &owners[index];
            let own = invoke_filesystem_tool_success(
                &executor,
                worker_id,
                *fingerprint,
                principal.clone(),
                &definitions,
                "read-file",
                read_input("durable/state.txt"),
            )
            .await?;
            assert_read_content(own, content);

            let other_base = &owners[1 - index].5;
            let other_path = format!("{other_base}/durable/state.txt");
            let denied = invoke_filesystem_tool(
                &executor,
                worker_id,
                *fingerprint,
                principal.clone(),
                &definitions,
                "read-file",
                read_input(&other_path),
            )
            .await?;
            assert_path_policy_denied(denied, 0, &other_path)?;
            assert_eq!(
                executor
                    .get_file_contents(worker_id, &format!("{base}/durable/state.txt"))
                    .await?,
                content.as_bytes()
            );
        }
        if !reconstruction_pass {
            for (environment_id, worker_id, ..) in &owners {
                unload_path_policy_owner(&executor, *environment_id, worker_id).await?;
            }
        }
    }

    for (environment_id, worker_id, fingerprint, ..) in &owners {
        let metadata = executor.get_worker_metadata(worker_id).await?;
        assert_eq!(metadata.environment_id, *environment_id);
        assert_eq!(metadata.fingerprint, *fingerprint);
        let oplog = executor.get_oplog(worker_id, OplogIndex::INITIAL).await?;
        assert!(
            oplog.iter().any(|entry| {
                matches!(entry.entry, PublicOplogEntry::AgentInvocationFinished(_))
            })
        );
    }
    let records = audit_sink.committed.lock().unwrap();
    assert_eq!(records.len(), 10);
    for (_, _, _, _, owner_name, base, content) in &owners {
        assert_eq!(
            records
                .iter()
                .filter(|record| {
                    record["owner"]["agentId"]
                        .as_str()
                        .is_some_and(|agent_id| agent_id.contains(owner_name))
                })
                .count(),
            5,
            "Audit must attribute every allowed and denied outcome to its durable owner"
        );
        let serialized = serde_json::to_string(&*records)?;
        assert!(!serialized.contains(base));
        assert!(!serialized.contains(content));
    }
    drop(records);
    audit_sink_task.abort();
    Ok(())
}
