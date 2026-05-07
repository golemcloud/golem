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

#[test]
async fn build_and_deploy_all_templates_for_moonbit() {
    build_and_deploy_all_templates_for_lang(GuestLanguage::MoonBit).await;
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

    // MoonBit does not yet support single->multi component upgrade via
    // incremental template application, so we only apply the base template.
    let templates: Vec<_> = if language == GuestLanguage::MoonBit {
        vec![language.id().to_string()]
    } else {
        templates
    };

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

    let outputs = ctx
        .cli([cmd::AGENT_TYPE, cmd::LIST, flag::FORMAT, "json"])
        .await;
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

// Verifies that a Scala application created with a single template (flat layout)
// can be transformed into a multi-component layout by adding a second Scala
// component, and that the resulting application still builds.
#[test]
async fn scala_single_to_multi_component_upgrade_builds() {
    single_to_multi_component_upgrade_builds_for_lang(GuestLanguage::Scala).await;
}

// Verifies that a MoonBit application created with a single template (flat layout)
// can be transformed into a multi-component layout by adding a second MoonBit
// component, and that the resulting application still builds.
#[test]
async fn moonbit_single_to_multi_component_upgrade_builds() {
    single_to_multi_component_upgrade_builds_for_lang(GuestLanguage::MoonBit).await;
}

async fn single_to_multi_component_upgrade_builds_for_lang(language: GuestLanguage) {
    let mut ctx = TestContext::new();

    let app_name = format!("single-to-multi-{}", language.id());
    let second_component = format!("{}:{}-second", app_name, language.id());

    fs::create_dir_all(ctx.cwd_path_join(&app_name)).unwrap();
    ctx.cd(&app_name);

    // Step 1: create a flat single-component application from the language's
    // default template. After this call, sources should live at the app root.
    let outputs = ctx
        .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, language.id()])
        .await;
    assert!(outputs.success_or_dump());

    // Verify the layout is actually flat: the component sources are at the
    // application root, not in a nested directory.
    assert_flat_layout(ctx.cwd_path(), language);

    // Step 2: add a second component using the same default template. This
    // forces the existing component to be promoted from a flat layout into
    // its own subdirectory (multi-component layout).
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            language.id(),
            flag::COMPONENT_NAME,
            &second_component,
        ])
        .await;
    assert!(outputs.success_or_dump());

    // Verify the layout is now multi-component: sources are no longer at the
    // application root.
    assert_multi_component_layout(ctx.cwd_path(), language);

    // Step 3: verify the upgraded application still builds.
    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());
}

fn assert_flat_layout(app_root: &std::path::Path, language: GuestLanguage) {
    match language {
        GuestLanguage::Scala => {
            assert!(
                app_root.join("src").join("main").join("scala").exists(),
                "expected flat Scala layout: src/main/scala/ should exist at the app root"
            );
            assert!(
                app_root.join("build.sbt").exists(),
                "expected flat Scala layout: build.sbt should exist at the app root"
            );
        }
        GuestLanguage::MoonBit => {
            assert!(
                app_root.join("moon.pkg").exists(),
                "expected flat MoonBit layout: moon.pkg should exist at the app root"
            );
            let has_mbt_at_root = std::fs::read_dir(app_root)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("mbt"));
            assert!(
                has_mbt_at_root,
                "expected flat MoonBit layout: at least one *.mbt file should exist at the app root"
            );
        }
        _ => {}
    }
}

fn assert_multi_component_layout(app_root: &std::path::Path, language: GuestLanguage) {
    match language {
        GuestLanguage::Scala => {
            assert!(
                !app_root.join("src").join("main").join("scala").exists(),
                "expected multi-component Scala layout: src/main/scala/ should no longer be at the app root"
            );
            // build.sbt MUST stay at the app root; it declares ThisBuild settings
            // and pairs with project/build.properties + project/plugins.sbt.
            assert!(
                app_root.join("build.sbt").exists(),
                "expected multi-component Scala layout: build.sbt should remain at the app root"
            );
            assert!(
                app_root.join("project").join("build.properties").exists(),
                "expected multi-component Scala layout: project/build.properties should remain at the app root"
            );
        }
        GuestLanguage::MoonBit => {
            assert!(
                !app_root.join("moon.pkg").exists(),
                "expected multi-component MoonBit layout: moon.pkg should no longer be at the app root"
            );
            let has_mbt_at_root = std::fs::read_dir(app_root)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("mbt"));
            assert!(
                !has_mbt_at_root,
                "expected multi-component MoonBit layout: no *.mbt files should remain at the app root"
            );
            // moon.mod.json MUST stay at the app root; it is the module-level
            // descriptor and pairs with the per-package moon.pkg files.
            assert!(
                app_root.join("moon.mod.json").exists(),
                "expected multi-component MoonBit layout: moon.mod.json should remain at the app root"
            );
        }
        _ => {}
    }
}

// We only select a few non-conflicting templates from all apps.
// Scala and MoonBit defaults both define CounterAgent, so we give the
// MoonBit component an explicit name to avoid agent-type collisions.
#[test]
async fn build_mixed_language_app() {
    let mut ctx = TestContext::new();

    let app_name = "mixed-lang-templates-app";

    fs::create_dir_all(ctx.cwd_path_join(app_name)).unwrap();
    ctx.cd(app_name);

    for template in &[
        "ts/human-in-the-loop",
        "rust/json",
        "rust/snapshotting",
        "scala",
    ] {
        let outputs = ctx
            .cli([flag::YES, cmd::NEW, ".", flag::TEMPLATE, template])
            .await;
        assert!(outputs.success_or_dump());
    }

    // MoonBit default also uses CounterAgent (same as Scala), so give it an
    // explicit component name and rename the agent type to avoid the collision.
    let outputs = ctx
        .cli([
            flag::YES,
            cmd::NEW,
            ".",
            flag::TEMPLATE,
            "moonbit",
            flag::COMPONENT_NAME,
            "mixed-lang-templates-app:moonbit",
        ])
        .await;
    assert!(outputs.success_or_dump());

    for file in &["moonbit/counter.mbt", "golem.yaml"] {
        let path = ctx.cwd_path_join(file);
        let content = fs::read_to_string(&path).unwrap();
        fs::write(&path, content.replace("CounterAgent", "MoonbitCounter")).unwrap();
    }

    let outputs = ctx.cli([cmd::BUILD]).await;
    assert!(outputs.success_or_dump());

    ctx.start_server().await;

    let outputs = ctx.cli([cmd::DEPLOY, flag::YES]).await;
    assert!(outputs.success_or_dump());
}
