use crate::app::{cmd, flag, replace_strings_in_file, TestContext};
use crate::Tracing;
use assert2::assert;
use assert2::let_assert;
use heck::ToKebabCase;
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
    assert!(outputs.success());

    let template_prefix = "  - ";

    let templates = outputs
        .stdout
        .iter()
        .filter(|line| line.starts_with(template_prefix) && line.contains(':'))
        .map(|line| {
            let template_with_desc = line.as_str().strip_prefix(template_prefix);
            let_assert!(Some(template_with_desc) = template_with_desc, "{}", line);
            let separator_index = template_with_desc.find(":");
            let_assert!(
                Some(separator_index) = separator_index,
                "{}",
                template_with_desc
            );

            &template_with_desc[..separator_index]
        })
        .collect::<Vec<_>>();

    println!("{templates:#?}");

    let outputs = ctx.cli([cmd::APP, cmd::NEW, app_name, "rust"]).await;
    assert!(outputs.success());

    ctx.cd(app_name);

    for template in templates {
        let component_name = format!("app:{}", template.to_kebab_case(),);
        let outputs = ctx
            .cli([cmd::COMPONENT, cmd::NEW, template, &component_name])
            .await;
        assert!(outputs.success());
    }

    // NOTE: renaming conflicting agent names
    // Prefix all agents with Rust/Ts to avoid conflicts between language implementations
    for (path, from, to) in [
        // Rust agents: prefix with Rust
        (
            "components-rust/app-rust/src/lib.rs",
            "CounterAgent",
            "RustCounterAgent",
        ),
        (
            "components-rust/app-rust/golem.yaml",
            "counter-agent",
            "rust-counter-agent",
        ),
        (
            "components-rust/app-rust-human-in-the-loop/src/lib.rs",
            "ApprovalWorkflow",
            "RustApprovalWorkflow",
        ),
        (
            "components-rust/app-rust-human-in-the-loop/golem.yaml",
            "approval-workflow",
            "rust-approval-workflow",
        ),
        (
            "components-rust/app-rust-human-in-the-loop/src/lib.rs",
            "HumanAgent",
            "RustHumanAgent",
        ),
        (
            "components-rust/app-rust-human-in-the-loop/golem.yaml",
            "human-agent",
            "rust-human-agent",
        ),
        (
            "components-rust/app-rust-json/src/lib.rs",
            "Tasks",
            "RustTasks",
        ),
        (
            "components-rust/app-rust-json/golem.yaml",
            "tasks(name)",
            "rust-tasks(name)",
        ),
        (
            "components-rust/app-rust-llm-session/src/lib.rs",
            "ChatAgent",
            "RustChatAgent",
        ),
        (
            "components-rust/app-rust-llm-session/golem.yaml",
            "chat-agent",
            "rust-chat-agent",
        ),
        (
            "components-rust/app-rust-llm-websearch-summary-example/src/lib.rs",
            "ResearchAgent",
            "RustResearchAgent",
        ),
        (
            "components-rust/app-rust-llm-websearch-summary-example/golem.yaml",
            "research-agent",
            "rust-research-agent",
        ),
        (
            "components-rust/app-rust-snapshotting/src/lib.rs",
            "CounterAgent",
            "RustCounterAgentSnapshotting",
        ),
        (
            "components-rust/app-rust-snapshotting/golem.yaml",
            "counter-agent",
            "rust-counter-agent-snapshotting",
        ),
        // TypeScript agents: prefix with Ts
        (
            "components-ts/app-ts/src/main.ts",
            "CounterAgent",
            "TsCounterAgent",
        ),
        (
            "components-ts/app-ts/golem.yaml",
            "counter-agent",
            "ts-counter-agent",
        ),
        (
            "components-ts/app-ts-human-in-the-loop/src/main.ts",
            "ApprovalWorkflow",
            "TsApprovalWorkflow",
        ),
        (
            "components-ts/app-ts-human-in-the-loop/golem.yaml",
            "approval-workflow",
            "ts-approval-workflow",
        ),
        (
            "components-ts/app-ts-human-in-the-loop/src/main.ts",
            "HumanAgent",
            "TsHumanAgent",
        ),
        (
            "components-ts/app-ts-human-in-the-loop/golem.yaml",
            "human-agent",
            "ts-human-agent",
        ),
        (
            "components-ts/app-ts-json/src/main.ts",
            "TaskAgent",
            "TsTaskAgent",
        ),
        (
            "components-ts/app-ts-json/golem.yaml",
            "task-agent",
            "ts-task-agent",
        ),
        (
            "components-ts/app-ts-llm-session/src/main.ts",
            "ChatAgent",
            "TsChatAgent",
        ),
        (
            "components-ts/app-ts-llm-session/golem.yaml",
            "chat-agent",
            "ts-chat-agent",
        ),
        (
            "components-ts/app-ts-snapshotting/src/main.ts",
            "CounterAgent",
            "TsCounterAgentSnapshotting",
        ),
        (
            "components-ts/app-ts-snapshotting/golem.yaml",
            "counter-agent",
            "ts-counter-agent-snapshotting",
        ),
        (
            "components-ts/app-ts-websearch-summary-example/src/main.ts",
            "ResearchAgent",
            "TsResearchAgent",
        ),
        (
            "components-ts/app-ts-websearch-summary-example/golem.yaml",
            "research-agent",
            "ts-research-agent",
        ),
    ] {
        replace_strings_in_file(ctx.cwd_path_join(path), &[(from, to)]).unwrap()
    }

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert!(outputs.success());

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success());
}
