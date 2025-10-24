use crate::app::{cmd, flag, TestContext};
use crate::Tracing;
use assert2::let_assert;
use heck::ToKebabCase;
use nanoid::nanoid;
use test_r::{inherit_test_dep, test};

inherit_test_dep!(Tracing);

#[test]
async fn build_and_deploy_all_templates() {
    let mut ctx = TestContext::new();
    let app_name = "all-templates-app";

    let outputs = ctx.cli([cmd::COMPONENT, cmd::TEMPLATES]).await;
    assert2::assert!(outputs.success());

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
    assert2::assert!(outputs.success());

    ctx.cd(app_name);

    let alphabet: [char; 8] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'];
    for template in templates {
        for _ in 0..2 {
            let component_name = format!(
                "app:{}-{}",
                template.to_kebab_case(),
                nanoid!(10, &alphabet),
            );
            let outputs = ctx
                .cli([cmd::COMPONENT, cmd::NEW, template, &component_name])
                .await;
            assert2::assert!(outputs.success());
        }
    }

    let outputs = ctx.cli([cmd::APP, cmd::BUILD]).await;
    assert2::assert!(outputs.success());

    ctx.start_server();

    let outputs = ctx.cli([cmd::APP, cmd::DEPLOY, flag::YES]).await;
    assert2::assert!(outputs.success());
}
