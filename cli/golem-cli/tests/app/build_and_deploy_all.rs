use crate::app::{cmd, flag, replace_string_in_file, TestContext};
use crate::Tracing;
use assert2::assert;
use assert2::let_assert;
use golem_templates::model::GuestLanguage;
use heck::ToKebabCase;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, tag, test};

inherit_test_dep!(Tracing);

#[test]
#[tag(group2)]
async fn build_and_deploy_all_templates_default() {
    build_and_deploy_all_templates(None).await;
}

async fn build_and_deploy_all_templates(group: Option<&str>) {
    let mut ctx = TestContext::new();
    if let Some(group) = group {
        ctx.use_template_group(group);
    }
    let app_name = "all-templates-app";

    let outputs = ctx.cli([cmd::COMPONENT, cmd::TEMPLATES]).await;
    assert!(outputs.success_or_dump());

    let template_prefix = "  - ";

    let templates = outputs
        .stdout()
        .filter(|line| line.starts_with(template_prefix) && line.contains(':'))
        .map(|line| {
            let template_with_desc = line.strip_prefix(template_prefix);
            let_assert!(Some(template_with_desc) = template_with_desc, "{}", line);
            let separator_index = template_with_desc.find(":");
            let_assert!(
                Some(separator_index) = separator_index,
                "{}",
                template_with_desc
            );

            template_with_desc[..separator_index].to_string()
        })
        .collect::<Vec<_>>();

    println!("{templates:#?}");

    let outputs = ctx.cli([cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success_or_dump());

    ctx.cd(app_name);

    for template in &templates {
        let component_name = format!("app:{}", template.to_kebab_case(),);
        let outputs = ctx
            .cli([cmd::COMPONENT, cmd::NEW, template, &component_name])
            .await;
        assert!(outputs.success_or_dump());
    }

    let agent_metas = agent_metas();

    // NOTE: renaming conflicting agent names, prefix all agents with Rust/Ts
    //       to avoid conflicts between language implementations
    for agent_meta in &agent_metas {
        replace_string_in_file(
            ctx.cwd_path_join(agent_meta.src_path),
            agent_meta.agent_template_name,
            agent_meta.agent_test_name,
        )
        .unwrap();
        for (path, from, to) in &agent_meta.extra_replaces {
            replace_string_in_file(ctx.cwd_path_join(path), from, to).unwrap()
        }
    }

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    // Checking bridge SDKs for all agents and languages, one by one
    let mut failed_bridge_sdks = vec![];
    for language in GuestLanguage::iter() {
        for agent_meta in &agent_metas {
            let output = ctx
                .cli([
                    cmd::GENERATE_BRIDGE,
                    flag::LANGUAGE,
                    language.id(),
                    flag::AGENT_TYPE_NAME,
                    agent_meta.agent_test_name,
                ])
                .await;
            if !output.success() {
                failed_bridge_sdks.push((
                    agent_meta.agent_test_name.to_string(),
                    language.id(),
                    output.stderr().map(|s| s.to_string()).collect::<Vec<_>>(),
                ));
            }
        }
    }

    assert!(
        failed_bridge_sdks.is_empty(),
        "Failed SDKs:\n{:#?}",
        failed_bridge_sdks
    );
}

fn agent_metas() -> Vec<AgentMeta> {
    vec![
        // Rust agents: prefix with Rust
        AgentMeta::new(
            "components-rust/app-rust/src/lib.rs",
            "CounterAgent",
            "RustCounterAgent",
            vec![(
                "components-rust/app-rust/golem.yaml",
                "counter-agent",
                "rust-counter-agent",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-human-in-the-loop/src/lib.rs",
            "ApprovalWorkflow",
            "RustApprovalWorkflow",
            vec![(
                "components-rust/app-rust-human-in-the-loop/golem.yaml",
                "approval-workflow",
                "rust-approval-workflow",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-human-in-the-loop/src/lib.rs",
            "HumanAgent",
            "RustHumanAgent",
            vec![(
                "components-rust/app-rust-human-in-the-loop/golem.yaml",
                "human-agent",
                "rust-human-agent",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-json/src/lib.rs",
            "Tasks",
            "RustTasks",
            vec![(
                "components-rust/app-rust-json/golem.yaml",
                "tasks(name)",
                "rust-tasks(name)",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-llm-session/src/lib.rs",
            "ChatAgent",
            "RustChatAgent",
            vec![(
                "components-rust/app-rust-llm-session/golem.yaml",
                "chat-agent",
                "rust-chat-agent",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-llm-websearch-summary-example/src/lib.rs",
            "ResearchAgent",
            "RustResearchAgent",
            vec![(
                "components-rust/app-rust-llm-websearch-summary-example/golem.yaml",
                "research-agent",
                "rust-research-agent",
            )],
        ),
        AgentMeta::new(
            "components-rust/app-rust-snapshotting/src/lib.rs",
            "CounterAgent",
            "RustCounterAgentSnapshotting",
            vec![(
                "components-rust/app-rust-snapshotting/golem.yaml",
                "counter-agent",
                "rust-counter-agent-snapshotting",
            )],
        ),
        // TypeScript agents: prefix with Ts
        AgentMeta::new(
            "components-ts/app-ts/src/main.ts",
            "CounterAgent",
            "TsCounterAgent",
            vec![
                (
                    "components-ts/app-ts/src/main.ts",
                    "/counters",
                    "/ts-counters",
                ),
                (
                    "components-ts/app-ts/golem.yaml",
                    "counter-agent",
                    "ts-counter-agent",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-human-in-the-loop/src/main.ts",
            "WorkflowAgent",
            "TsWorkflowAgent",
            vec![
                (
                    "components-ts/app-ts-human-in-the-loop/src/main.ts",
                    "/workflows",
                    "/ts-workflows",
                ),
                (
                    "components-ts/app-ts-human-in-the-loop/golem.yaml",
                    "workflow-agent",
                    "ts-workflow-agent",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-human-in-the-loop/src/main.ts",
            "HumanAgent",
            "TsHumanAgent",
            vec![
                (
                    "components-ts/app-ts-human-in-the-loop/src/main.ts",
                    "/humans",
                    "/ts-humans",
                ),
                (
                    "components-ts/app-ts-human-in-the-loop/golem.yaml",
                    "human-agent",
                    "ts-human-agent",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-json/src/main.ts",
            "TaskAgent",
            "TsTaskAgent",
            vec![
                (
                    "components-ts/app-ts-json/src/main.ts",
                    "/task-agents",
                    "/ts-task-agents",
                ),
                (
                    "components-ts/app-ts-json/golem.yaml",
                    "task-agent",
                    "ts-task-agent",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-llm-session/src/main.ts",
            "ChatAgent",
            "TsChatAgent",
            vec![
                (
                    "components-ts/app-ts-llm-session/src/main.ts",
                    "/chats",
                    "/ts-chats",
                ),
                (
                    "components-ts/app-ts-llm-session/golem.yaml",
                    "chat-agent",
                    "ts-chat-agent",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-snapshotting/src/main.ts",
            "CounterAgent",
            "TsCounterAgentSnapshotting",
            vec![
                (
                    "components-ts/app-ts-snapshotting/src/main.ts",
                    "/counters",
                    "/ts-agent-snapshotting-counters",
                ),
                (
                    "components-ts/app-ts-snapshotting/golem.yaml",
                    "counter-agent",
                    "ts-counter-agent-snapshotting",
                ),
            ],
        ),
        AgentMeta::new(
            "components-ts/app-ts-websearch-summary-example/src/main.ts",
            "ResearchAgent",
            "TsResearchAgent",
            vec![
                (
                    "components-ts/app-ts-websearch-summary-example/src/main.ts",
                    "/research",
                    "/ts-research",
                ),
                (
                    "components-ts/app-ts-websearch-summary-example/golem.yaml",
                    "research-agent",
                    "ts-research-agent",
                ),
            ],
        ),
    ]
}

struct AgentMeta {
    src_path: &'static str,
    agent_template_name: &'static str,
    agent_test_name: &'static str,
    extra_replaces: Vec<(&'static str, &'static str, &'static str)>,
}

impl AgentMeta {
    pub fn new(
        src_path: &'static str,
        agent_template_name: &'static str,
        agent_test_name: &'static str,
        extra_replaces: Vec<(&'static str, &'static str, &'static str)>,
    ) -> Self {
        Self {
            src_path,
            agent_template_name,
            agent_test_name,
            extra_replaces,
        }
    }
}
