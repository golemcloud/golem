use crate::cli::{Cli, CliLive};
use golem_cli::model::template::TemplateView;
use golem_test_framework::config::TestDependencies;
use libtest_mimic::{Failed, Trial};
use std::sync::Arc;

fn make(
    suffix: &str,
    name: &str,
    cli: CliLive,
    deps: Arc<dyn TestDependencies + Send + Sync + 'static>,
) -> Vec<Trial> {
    let ctx = (deps, name.to_string(), cli);
    vec![
        Trial::test_in_context(
            format!("template_add_and_find_all{suffix}"),
            ctx.clone(),
            template_add_and_find_all,
        ),
        Trial::test_in_context(
            format!("template_add_and_find_by_name{suffix}"),
            ctx.clone(),
            template_add_and_find_by_name,
        ),
        Trial::test_in_context(
            format!("template_update{suffix}"),
            ctx.clone(),
            template_update,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI short",
        CliLive::make(deps.clone()).unwrap().with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn template_add_and_find_all(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name = format!("{name} template add and find all");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<TemplateView> = cli.run(&["template", "list"])?;
    assert!(res.contains(&template), "{res:?}.contains({template:?})");
    Ok(())
}

fn template_add_and_find_by_name(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name_other = format!("{name} template add and find by name other");
    let template_name = format!("{name} template add and find by name");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let _: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name_other,
        env_service.to_str().unwrap(),
    ])?;
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<TemplateView> = cli.run(&[
        "template",
        "list",
        &cfg.arg('t', "template-name"),
        &template_name,
    ])?;
    assert!(res.contains(&template), "{res:?}.contains({template:?})");
    assert_eq!(res.len(), 1, "{res:?}.len() == 1");
    Ok(())
}

fn template_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name = format!("{name} template update");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;
    let _: TemplateView = cli.run(&[
        "template",
        "update",
        &cfg.arg('T', "template-id"),
        &template.template_id,
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}
