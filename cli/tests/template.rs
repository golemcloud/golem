use crate::cli::{Cli, CliLive};
use crate::context::ContextInfo;
use golem_cli::clients::template::TemplateView;
use libtest_mimic::{Failed, Trial};
use std::sync::Arc;

fn make(suffix: &str, name: &str, cli: CliLive, context: Arc<ContextInfo>) -> Vec<Trial> {
    let ctx = (context.clone(), name.to_string(), cli);
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

pub fn all(context: Arc<ContextInfo>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI short",
        CliLive::make(&context.golem_service)
            .unwrap()
            .with_short_args(),
        context.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI long",
        CliLive::make(&context.golem_service)
            .unwrap()
            .with_long_args(),
        context.clone(),
    );

    short_args.append(&mut long_args);

    short_args
}

fn template_add_and_find_all(
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_name = format!("{name} template add and find all");
    let env_service = context.env.wasm_root.join("environment-service.wasm");
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_name_other = format!("{name} template add and find by name other");
    let template_name = format!("{name} template add and find by name");
    let env_service = context.env.wasm_root.join("environment-service.wasm");
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
    (context, name, cli): (Arc<ContextInfo>, String, CliLive),
) -> Result<(), Failed> {
    let template_name = format!("{name} template update");
    let env_service = context.env.wasm_root.join("environment-service.wasm");
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
