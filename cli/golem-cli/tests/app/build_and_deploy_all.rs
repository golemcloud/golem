use crate::app::{cmd, flag, replace_strings_in_file, TestContext};
use crate::Tracing;
use assert2::assert;
use assert2::let_assert;
use heck::ToKebabCase;
use nanoid::nanoid;
use test_r::{inherit_test_dep, tag, test};

inherit_test_dep!(Tracing);

#[test]
#[tag(group2)]
async fn build_and_deploy_all_templates_default() {
    build_and_deploy_all_templates(None).await;
}

#[test]
#[tag(group3)]
async fn build_and_deploy_all_templates_generic() {
    build_and_deploy_all_templates(Some("generic")).await;
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
    for (path, from, to) in [
        (
            "components-rust/app-rust/src/lib.rs",
            "CounterAgent",
            "RustCounterAgent",
        ),
        (
            "components-ts/app-ts/src/main.ts",
            "CounterAgent",
            "TsCounterAgent",
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
