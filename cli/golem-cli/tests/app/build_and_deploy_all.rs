use crate::app::{cmd, flag, replace_string_in_file, TestContext};
use crate::Tracing;

use golem_cli::fs;
use golem_cli::model::GuestLanguage;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, tag, test};

inherit_test_dep!(Tracing);

#[test]
#[tag(group2)]
async fn build_and_deploy_all_templates() {
    let mut ctx = TestContext::new();

    let app_name = "all-templates-app";

    let outputs = ctx.cli([cmd::TEMPLATES]).await;
    assert!(outputs.success_or_dump());

    let template_prefix = "  - ";

    let templates = outputs
        .stdout()
        .filter(|line| line.starts_with(template_prefix) && line.contains(':'))
        .map(|line| {
            let template_with_desc = line.strip_prefix(template_prefix);
            let Some(template_with_desc) = template_with_desc else {
                panic!("{}", line)
            };
            let separator_index = template_with_desc.find(":");
            let Some(separator_index) = separator_index else {
                panic!("{}", template_with_desc)
            };

            template_with_desc[..separator_index].to_string()
        })
        .collect::<Vec<_>>();

    println!("{templates:#?}");

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();

    ctx.cd(app_name);

    for template in &templates {
        let outputs = ctx
            .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, template])
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
            "app-rust/src/lib.rs",
            "CounterAgent",
            "RustCounterAgent",
            vec![
                ("app-rust/src/lib.rs", "/counters", "/rust-counters"),
                ("app-rust/golem.yaml", "counter-agent", "rust-counter-agent"),
                ("app-rust/golem.yaml", "CounterAgent", "RustCounterAgent"),
            ],
        ),
        AgentMeta::new(
            "app-rust-human-in-the-loop/src/lib.rs",
            "WorkflowAgent",
            "RustWorkflowAgent",
            vec![
                (
                    "app-rust-human-in-the-loop/src/lib.rs",
                    "/workflows",
                    "/rust-workflows",
                ),
                (
                    "app-rust-human-in-the-loop/golem.yaml",
                    "workflow-agent",
                    "rust-workflow-agent",
                ),
                (
                    "app-rust-human-in-the-loop/golem.yaml",
                    "WorkflowAgent",
                    "RustWorkflowAgent",
                ),
            ],
        ),
        AgentMeta::new(
            "app-rust-human-in-the-loop/src/lib.rs",
            "HumanAgent",
            "RustHumanAgent",
            vec![
                (
                    "app-rust-human-in-the-loop/src/lib.rs",
                    "/humans",
                    "/rust-humans",
                ),
                (
                    "app-rust-human-in-the-loop/golem.yaml",
                    "human-agent",
                    "rust-human-agent",
                ),
                (
                    "app-rust-human-in-the-loop/golem.yaml",
                    "HumanAgent",
                    "RustHumanAgent",
                ),
            ],
        ),
        AgentMeta::new(
            "app-rust-json/src/lib.rs",
            "Tasks",
            "RustTasks",
            vec![
                (
                    "app-rust-json/src/lib.rs",
                    "/task-agents",
                    "/rust-task-agents",
                ),
                (
                    "app-rust-json/golem.yaml",
                    "tasks(name)",
                    "rust-tasks(name)",
                ),
                ("app-rust-json/golem.yaml", "Tasks", "RustTasks"),
            ],
        ),
        // NOTE: These are temporarily disabled because of golem-ai depending on an older golem-rust
        // AgentMeta::new(
        //     "app-rust-llm-session/src/lib.rs",
        //     "ChatAgent",
        //     "RustChatAgent",
        //     vec![
        //         (
        //             "app-rust-llm-session/src/lib.rs",
        //             "/chats",
        //             "/rust-chats",
        //         ),
        //         (
        //             "app-rust-llm-session/golem.yaml",
        //             "chat-agent",
        //             "rust-chat-agent",
        //         ),
        //     ],
        // ),
        // AgentMeta::new(
        //     "app-rust-llm-websearch-summary-example/src/lib.rs",
        //     "ResearchAgent",
        //     "RustResearchAgent",
        //     vec![
        //         (
        //             "app-rust-llm-websearch-summary-example/src/lib.rs",
        //             "/research",
        //             "/rust-research",
        //         ),
        //         (
        //             "app-rust-llm-websearch-summary-example/golem.yaml",
        //             "research-agent",
        //             "rust-research-agent",
        //         ),
        //     ],
        // ),
        AgentMeta::new(
            "app-rust-snapshotting/src/counter_with_snapshot_agent.rs",
            "CounterWithSnapshotAgent",
            "RustCounterAgentSnapshotting",
            vec![
                (
                    "app-rust-snapshotting/src/counter_with_snapshot_agent.rs",
                    "/snapshot-counters",
                    "/rust-agent-snapshotting-counters",
                ),
                (
                    "app-rust-snapshotting/golem.yaml",
                    "counter-with-snapshot-agent",
                    "rust-counter-agent-snapshotting",
                ),
            ],
        ),
        // TypeScript agents: prefix with Ts
        AgentMeta::new(
            "app-ts/src/main.ts",
            "CounterAgent",
            "TsCounterAgent",
            vec![
                ("app-ts/src/main.ts", "/counters", "/ts-counters"),
                ("app-ts/golem.yaml", "counter-agent", "ts-counter-agent"),
                ("app-ts/golem.yaml", "CounterAgent", "TsCounterAgent"),
            ],
        ),
        AgentMeta::new(
            "app-ts-human-in-the-loop/src/main.ts",
            "WorkflowAgent",
            "TsWorkflowAgent",
            vec![
                (
                    "app-ts-human-in-the-loop/src/main.ts",
                    "/workflows",
                    "/ts-workflows",
                ),
                (
                    "app-ts-human-in-the-loop/golem.yaml",
                    "workflow-agent",
                    "ts-workflow-agent",
                ),
                (
                    "app-ts-human-in-the-loop/golem.yaml",
                    "WorkflowAgent",
                    "TsWorkflowAgent",
                ),
            ],
        ),
        AgentMeta::new(
            "app-ts-human-in-the-loop/src/main.ts",
            "HumanAgent",
            "TsHumanAgent",
            vec![
                (
                    "app-ts-human-in-the-loop/src/main.ts",
                    "/humans",
                    "/ts-humans",
                ),
                (
                    "app-ts-human-in-the-loop/golem.yaml",
                    "human-agent",
                    "ts-human-agent",
                ),
                (
                    "app-ts-human-in-the-loop/golem.yaml",
                    "HumanAgent",
                    "TsHumanAgent",
                ),
            ],
        ),
        AgentMeta::new(
            "app-ts-json/src/main.ts",
            "TaskAgent",
            "TsTaskAgent",
            vec![
                ("app-ts-json/src/main.ts", "/task-agents", "/ts-task-agents"),
                ("app-ts-json/golem.yaml", "task-agent", "ts-task-agent"),
                ("app-ts-json/golem.yaml", "TaskAgent", "TsTaskAgent"),
            ],
        ),
        AgentMeta::new(
            "app-ts-snapshotting/src/main.ts",
            "CounterWithSnapshotAgent",
            "TsCounterAgentSnapshotting",
            vec![
                (
                    "app-ts-snapshotting/src/main.ts",
                    "/snapshot-counters",
                    "/ts-agent-snapshotting-counters",
                ),
                (
                    "app-ts-snapshotting/golem.yaml",
                    "counter-with-snapshot-agent",
                    "ts-counter-agent-snapshotting",
                ),
                (
                    "app-ts-snapshotting/golem.yaml",
                    "CounterWithSnapshotAgent",
                    "TsCounterAgentSnapshotting",
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
