use crate::cli::{Cli, CliLive};
use crate::worker::make_template;
use golem_cli::model::template::TemplateView;
use golem_cli::model::Format;
use golem_client::model::WorkerId;
use golem_test_framework::config::TestDependencies;
use indoc::formatdoc;
use libtest_mimic::{Failed, Trial};
use regex::Regex;
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
            format!("text_template_add{suffix}"),
            ctx.clone(),
            text_template_add,
        ),
        Trial::test_in_context(
            format!("text_template_update{suffix}"),
            ctx.clone(),
            text_template_update,
        ),
        Trial::test_in_context(
            format!("text_template_list{suffix}"),
            ctx.clone(),
            text_template_list,
        ),
        Trial::test_in_context(
            format!("text_worker_add{suffix}"),
            ctx.clone(),
            text_worker_add,
        ),
        Trial::test_in_context(
            format!("text_worker_get_invocation_key{suffix}"),
            ctx.clone(),
            text_worker_get_invocation_key,
        ),
        Trial::test_in_context(
            format!("text_worker_invoke_and_await{suffix}"),
            ctx.clone(),
            text_worker_invoke_and_await,
        ),
        Trial::test_in_context(
            format!("text_worker_get{suffix}"),
            ctx.clone(),
            text_worker_get,
        ),
        Trial::test_in_context(
            format!("text_worker_list{suffix}"),
            ctx.clone(),
            text_worker_list,
        ),
        Trial::test_in_context(
            format!("text_example_list{suffix}"),
            ctx.clone(),
            text_example_list,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make(deps.clone()).unwrap().with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make(deps.clone()).unwrap().with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn text_template_add(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name = format!("{name} text template add");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let template_res = cli.with_format(Format::Text).run_string(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;

    let lines = template_res.lines().collect::<Vec<_>>();

    let regex_header =
        Regex::new("New template created with ID ([^ ]+), version 0, and size of ([0-9]+) bytes.")
            .unwrap();
    assert!(regex_header.is_match(lines.first().unwrap()));

    assert_eq!(
        *lines.get(1).unwrap(),
        format!("Template name: {template_name}.")
    );
    assert_eq!(*lines.get(2).unwrap(), "Exports:");
    assert_eq!(
        *lines.get(3).unwrap(),
        "\tgolem:it/api/get-environment() -> result<list<tuple<string, string>>, string>"
    );
    assert_eq!(
        *lines.get(4).unwrap(),
        "\tgolem:it/api/get-arguments() -> result<list<string>, string>"
    );

    Ok(())
}

fn text_template_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name = format!("{name} text template update");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;

    let update_res = cli.with_format(Format::Text).run_string(&[
        "template",
        "update",
        &cfg.arg('T', "template-id"),
        &template.template_id,
        env_service.to_str().unwrap(),
    ])?;

    let lines = update_res.lines().collect::<Vec<_>>();

    assert_eq!(
        *lines.first().unwrap(),
        format!(
            "Updated template with ID {}. New version: 1. Template size is 72305 bytes.",
            template.template_id
        )
    );
    assert_eq!(
        *lines.get(1).unwrap(),
        format!("Template name: {template_name}.")
    );
    assert_eq!(*lines.get(2).unwrap(), "Exports:");
    assert_eq!(
        *lines.get(3).unwrap(),
        "\tgolem:it/api/get-environment() -> result<list<tuple<string, string>>, string>"
    );
    assert_eq!(
        *lines.get(4).unwrap(),
        "\tgolem:it/api/get-arguments() -> result<list<string>, string>"
    );

    Ok(())
}

fn text_template_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_name = format!("{name: <9} text template list");
    let env_service = deps.template_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let template: TemplateView = cli.run(&[
        "template",
        "add",
        &cfg.arg('t', "template-name"),
        &template_name,
        env_service.to_str().unwrap(),
    ])?;

    let list_res = cli.with_format(Format::Text).run_string(&[
        "template",
        "list",
        &cfg.arg('t', "template-name"),
        &template_name,
    ])?;

    let expected = formatdoc!(
        "
        +--------------------------------------+------------------------------+---------+-------+---------------+
        | ID                                   | Name                         | Version | Size  | Exports count |
        +--------------------------------------+------------------------------+---------+-------+---------------+
        | {} | {} |       0 | 72305 |             2 |
        +--------------------------------------+------------------------------+---------+-------+---------------+
        ",
        template.template_id,
        template_name,
    );

    assert_eq!(strip_ansi_escapes::strip_str(list_res), expected);

    Ok(())
}

fn text_worker_add(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id = make_template(deps, &format!("{name} text worker add"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_add");
    let cfg = &cli.config;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    let regex_res =
        Regex::new("New worker created for template ([^ ]+), with name ([^ ]+).\n").unwrap();

    let matched = regex_res.captures(&res);

    assert!(matched.is_some());

    assert_eq!(
        matched.as_ref().unwrap().get(1).unwrap().as_str(),
        template_id
    );
    assert_eq!(
        matched.as_ref().unwrap().get(2).unwrap().as_str(),
        worker_name
    );

    Ok(())
}

fn text_worker_get_invocation_key(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} text worker invocation key"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invocation_key");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invocation-key",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
    ])?;

    let lines = res.lines().collect::<Vec<_>>();

    assert!(lines.first().unwrap().starts_with("Invocation key: "));
    assert_eq!(
        *lines.get(1).unwrap(),
        "You can use it in invoke-and-await command this way:"
    );
    assert!(lines
        .get(2)
        .unwrap()
        .starts_with("invoke-and-await --invocation-key "));

    Ok(())
}

fn text_worker_invoke_and_await(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id =
        make_template(deps, &format!("{name} text worker_invoke_and_await"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api/get-arguments",
    ])?;

    let lines = res.lines().collect::<Vec<_>>();

    assert_eq!(
        *lines.first().unwrap(),
        "Invocation results in WAVE format:"
    );
    assert_eq!(*lines.get(1).unwrap(), r#"- ok(["test-arg"])"#);

    Ok(())
}

fn text_worker_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id = make_template(deps, &format!("{name} text worker get"), &cli)?.template_id;
    let worker_name = format!("{name}_worker_get");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "get",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    let expected = formatdoc!(
        r#"
            Worker "{worker_name}" of template {template_id} with template version 0.
            Status: Idle.
            Startup arguments: .
            Environment variables: .
            Retry count: 0.
            "#
    );

    assert_eq!(res, expected);

    Ok(())
}

fn text_worker_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let template_id = make_template(deps, &format!("{name} text worker list"), &cli)?.template_id;
    let worker_name = format!("{name:_<9}_worker_list");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('T', "template-id"),
        &template_id,
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "list",
        &cfg.arg('T', "template-id"),
        &template_id,
        &cfg.arg('f', "filter"),
        &format!("name = {worker_name}"),
        &cfg.arg('p', "precise"),
        "true",
    ])?;

    let expected =
        formatdoc!(
            "
            +--------------------------------------+-----------------------+--------+------------------+
            | Template                             | Name                  | Status | Template version |
            +--------------------------------------+-----------------------+--------+------------------+
            | {template_id} | {worker_name} |   Idle |                0 |
            +--------------------------------------+-----------------------+--------+------------------+
            "
        );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_example_list(
    (_, _, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let cfg = &cli.config;
    let res = cli.with_format(Format::Text).run_string(&[
        "list-examples",
        &cfg.arg('l', "language"),
        "C#",
    ])?;

    let expected = formatdoc!(
        "
            +------------+----------+-------+--------------------+
            | Name       | Language | Tier  | Description        |
            +------------+----------+-------+--------------------+
            | cs-minimal | C#       | tier3 | A simple stateless |
            |            |          |       | Golem function     |
            |            |          |       | written in C# with |
            |            |          |       | no dependencies on |
            |            |          |       | external services  |
            +------------+----------+-------+--------------------+
            "
    );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}
