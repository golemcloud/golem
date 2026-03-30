use crate::Tracing;
use crate::app::{TestContext, cmd, flag};

use golem_cli::fs;
use golem_cli::model::GuestLanguage;
use golem_common::base_model::agent::DeployedRegisteredAgentType;
use strum::IntoEnumIterator;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
async fn build_and_deploy_all_templates_for_ts() {
    build_and_deploy_all_templates_for_lang(GuestLanguage::TypeScript).await;
}

#[test]
async fn build_and_deploy_all_templates_for_rust() {
    build_and_deploy_all_templates_for_lang(GuestLanguage::Rust).await;
}

async fn build_and_deploy_all_templates_for_lang(language: GuestLanguage) {
    let mut ctx = TestContext::new();

    let app_name = format!("all-templates-app-{}", language.id());

    let outputs = ctx.cli([cmd::TEMPLATES]).await;
    assert!(outputs.success_or_dump());

    let template_text_output_prefix = "  - ";

    let templates = outputs
        .stdout()
        .filter(|line| line.starts_with(template_text_output_prefix) && line.contains(':'))
        .map(|line| {
            let template_with_desc = line.strip_prefix(template_text_output_prefix);
            let Some(template_with_desc) = template_with_desc else {
                panic!("{}", line)
            };
            let separator_index = template_with_desc.find(":");
            let Some(separator_index) = separator_index else {
                panic!("{}", template_with_desc)
            };

            template_with_desc[..separator_index].to_string()
        })
        .filter(|template| template.starts_with(language.id()))
        .collect::<Vec<_>>();

    println!("{templates:#?}");

    fs::create_dir_all(ctx.cwd_path_join(&app_name)).unwrap();
    ctx.cd(app_name);

    for template in &templates {
        let outputs = ctx
            .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, template])
            .await;
        assert!(outputs.success_or_dump());
    }

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());

    let outputs = ctx.cli([cmd::LIST_AGENT_TYPES, flag::FORMAT, "json"]).await;
    let deployed_agent_types = outputs
        .stdout_json::<Vec<DeployedRegisteredAgentType>>()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    // Checking bridge SDK generation for all agents and languages, one by one
    let mut failed_bridge_sdks = vec![];
    for language in GuestLanguage::iter().filter(|l| l.supports_bridge_generation()) {
        for deployed_agent_type in &deployed_agent_types {
            let output = ctx
                .cli([
                    cmd::GENERATE_BRIDGE,
                    flag::LANGUAGE,
                    language.id(),
                    flag::AGENT_TYPE_NAME,
                    deployed_agent_type.agent_type.type_name.as_str(),
                ])
                .await;
            if !output.success() {
                failed_bridge_sdks.push((
                    deployed_agent_type.agent_type.type_name.as_str(),
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

// We only select a few non-conflicting templates from all apps
#[test]
async fn build_mixed_language_app() {
    let mut ctx = TestContext::new();

    let templates = GuestLanguage::iter()
        .flat_map(|language| match language {
            GuestLanguage::TypeScript => {
                vec!["ts", "ts/human-in-the-loop"]
            }
            GuestLanguage::Rust => {
                vec!["rust/json", "rust/snapshotting"]
            }
            GuestLanguage::Scala => {
                vec!["scala"]
            }
        })
        .collect::<Vec<_>>();

    let app_name = "mixed-lang-templates-app";

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();
    ctx.cd(app_name);

    for template in &templates {
        let outputs = ctx
            .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, template])
            .await;
        assert!(outputs.success_or_dump());
    }

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
}
