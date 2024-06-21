use crate::api_definition::{
    golem_def, make_golem_file, make_open_api_file, make_shopping_cart_component,
};
use crate::cli::{Cli, CliLive};
use crate::worker::make_component;
use golem_cli::model::component::ComponentView;
use golem_cli::model::Format;
use golem_client::model::{ApiDeployment, HttpApiDefinition, WorkerId};
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
            format!("text_component_add{suffix}"),
            ctx.clone(),
            text_component_add,
        ),
        Trial::test_in_context(
            format!("text_component_update{suffix}"),
            ctx.clone(),
            text_component_update,
        ),
        Trial::test_in_context(
            format!("text_component_get{suffix}"),
            ctx.clone(),
            text_component_get,
        ),
        Trial::test_in_context(
            format!("text_component_list{suffix}"),
            ctx.clone(),
            text_component_list,
        ),
        Trial::test_in_context(
            format!("text_worker_add{suffix}"),
            ctx.clone(),
            text_worker_add,
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
        Trial::test_in_context(
            format!("text_api_definition_import{suffix}"),
            ctx.clone(),
            text_api_definition_import,
        ),
        Trial::test_in_context(
            format!("text_api_definition_add{suffix}"),
            ctx.clone(),
            text_api_definition_add,
        ),
        Trial::test_in_context(
            format!("text_api_definition_update{suffix}"),
            ctx.clone(),
            text_api_definition_update,
        ),
        Trial::test_in_context(
            format!("text_api_definition_list{suffix}"),
            ctx.clone(),
            text_api_definition_list,
        ),
        Trial::test_in_context(
            format!("text_api_definition_get{suffix}"),
            ctx.clone(),
            text_api_definition_get,
        ),
        Trial::test_in_context(
            format!("text_api_deployment_deploy{suffix}"),
            ctx.clone(),
            text_api_deployment_deploy,
        ),
        Trial::test_in_context(
            format!("text_api_deployment_get{suffix}"),
            ctx.clone(),
            text_api_deployment_get,
        ),
        Trial::test_in_context(
            format!("text_api_deployment_list{suffix}"),
            ctx.clone(),
            text_api_deployment_list,
        ),
    ]
}

pub fn all(deps: Arc<dyn TestDependencies + Send + Sync + 'static>) -> Vec<Trial> {
    let mut short_args = make(
        "_short",
        "CLI_short",
        CliLive::make("text_short", deps.clone())
            .unwrap()
            .with_short_args(),
        deps.clone(),
    );

    let mut long_args = make(
        "_long",
        "CLI_long",
        CliLive::make("text_long", deps.clone())
            .unwrap()
            .with_long_args(),
        deps,
    );

    short_args.append(&mut long_args);

    short_args
}

fn text_component_add(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} text component add");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component_res = cli.with_format(Format::Text).run_string(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let lines = component_res.lines().collect::<Vec<_>>();

    let regex_header =
        Regex::new("New component created with ID ([^ ]+), version 0, and size of ([0-9]+) bytes.")
            .unwrap();
    assert!(regex_header.is_match(lines.first().unwrap()));

    assert_eq!(
        *lines.get(1).unwrap(),
        format!("Component name: {component_name}.")
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

fn text_component_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} text component update");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let update_res = cli.with_format(Format::Text).run_string(&[
        "component",
        "update",
        &cfg.arg('C', "component-id"),
        &component.component_id,
        env_service.to_str().unwrap(),
    ])?;

    let lines = update_res.lines().collect::<Vec<_>>();

    assert_eq!(
        *lines.first().unwrap(),
        format!(
            "Updated component with ID {}. New version: 1. Component size is 72129 bytes.",
            component.component_id
        )
    );
    assert_eq!(
        *lines.get(1).unwrap(),
        format!("Component name: {component_name}.")
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

fn text_component_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name} text component get");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let get_res = cli.with_format(Format::Text).run_string(&[
        "component",
        "get",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;

    let lines = get_res.lines().collect::<Vec<_>>();

    assert_eq!(
        *lines.first().unwrap(),
        format!(
            "Component with ID {}. Version: 0. Component size is 72129 bytes.",
            component.component_id
        )
    );
    assert_eq!(
        *lines.get(1).unwrap(),
        format!("Component name: {component_name}.")
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

fn text_component_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("{name: <9} text component list");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let list_res = cli.with_format(Format::Text).run_string(&[
        "component",
        "list",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;

    let expected = formatdoc!(
        "
        +--------------------------------------+-------------------------------+---------+-------+---------------+
        | ID                                   | Name                          | Version | Size  | Exports count |
        +--------------------------------------+-------------------------------+---------+-------+---------------+
        | {} | {} |       0 | 72129 |             2 |
        +--------------------------------------+-------------------------------+---------+-------+---------------+
        ",
        component.component_id,
        component_name,
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
    let component_id = make_component(deps, &format!("{name} text worker add"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_add");
    let cfg = &cli.config;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    let regex_res =
        Regex::new("New worker created for component ([^ ]+), with name ([^ ]+).\n").unwrap();

    let matched = regex_res.captures(&res);

    assert!(matched.is_some());

    assert_eq!(
        matched.as_ref().unwrap().get(1).unwrap().as_str(),
        component_id
    );
    assert_eq!(
        matched.as_ref().unwrap().get(2).unwrap().as_str(),
        worker_name
    );

    Ok(())
}

fn text_worker_invoke_and_await(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_id =
        make_component(deps, &format!("{name} text worker_invoke_and_await"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('f', "function"),
        "golem:it/api.{get-arguments}",
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
    let component_id = make_component(deps, &format!("{name} text worker get"), &cli)?.component_id;
    let worker_name = format!("{name}_worker_get");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "get",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    let expected = formatdoc!(
        r#"
            Worker "{worker_name}" of component {component_id} with component version 0.
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
    let component_id =
        make_component(deps, &format!("{name} text worker list"), &cli)?.component_id;
    let worker_name = format!("{name:_<9}_worker_list");
    let cfg = &cli.config;
    let _: WorkerId = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component-id"),
        &component_id,
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "list",
        &cfg.arg('C', "component-id"),
        &component_id,
        &cfg.arg('f', "filter"),
        &format!("name = {worker_name}"),
        &cfg.arg('p', "precise"),
        "true",
    ])?;

    let expected =
        formatdoc!(
            "
            +--------------------------------------+-----------------------+--------+-------------------+
            | Component                            | Name                  | Status | Component version |
            +--------------------------------------+-----------------------+--------+-------------------+
            | {component_id} | {worker_name} |   Idle |                 0 |
            +--------------------------------------+-----------------------+--------+-------------------+
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

fn text_api_definition_import(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("text_api_definition_import{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let path = make_open_api_file(&component_name, &component.component_id)?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "import",
        path.to_str().unwrap(),
    ])?;

    let component_end = &component.component_id[component.component_id.len() - 7..];

    let expected =
        formatdoc!(
            "
            API Definition imported with ID {component_name} and version 0.1.0.
            Routes:
            +--------+------------------------------+-------------+--------------------------------+
            | Method | Path                         | ComponentId | WorkerName                     |
            +--------+------------------------------+-------------+--------------------------------+
            | Get    | /{{user-id}}/get-cart-contents |    *{component_end} | worker-${{request.path.user-id}} |
            +--------+------------------------------+-------------+--------------------------------+
            "
        );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_api_definition_add(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("text_api_definition_add{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "add",
        path.to_str().unwrap(),
    ])?;

    let component_end = &component.component_id[component.component_id.len() - 7..];

    let expected =
        formatdoc!(
            "
            API Definition created with ID {component_name} and version 0.1.0.
            Routes:
            +--------+------------------------------+-------------+--------------------------------+
            | Method | Path                         | ComponentId | WorkerName                     |
            +--------+------------------------------+-------------+--------------------------------+
            | Get    | /{{user-id}}/get-cart-contents |    *{component_end} | worker-${{request.path.user-id}} |
            +--------+------------------------------+-------------+--------------------------------+
            "
        );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_api_definition_update(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("text_api_definition_update{name}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "update",
        path.to_str().unwrap(),
    ])?;

    let component_end = &component.component_id[component.component_id.len() - 7..];

    let expected =
        formatdoc!(
            "
            API Definition updated with ID {component_name} and version 0.1.0.
            Routes:
            +--------+------------------------------+-------------+--------------------------------+
            | Method | Path                         | ComponentId | WorkerName                     |
            +--------+------------------------------+-------------+--------------------------------+
            | Get    | /{{user-id}}/get-cart-contents |    *{component_end} | worker-${{request.path.user-id}} |
            +--------+------------------------------+-------------+--------------------------------+
            "
        );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_api_definition_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("text_api_definition_list{name:_>9}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;
    let cfg = &cli.config;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "list",
        &cfg.arg('i', "id"),
        &component_name,
    ])?;

    let expected = formatdoc!(
        "
            +-----------------------------------+---------+--------------+
            | ID                                | Version | Routes count |
            +-----------------------------------+---------+--------------+
            | {component_name} | 0.1.0   |            1 |
            +-----------------------------------+---------+--------------+
            "
    );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_api_definition_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_name = format!("text_api_definition_get{name:_>9}");
    let component = make_shopping_cart_component(deps, &component_name, &cli)?;
    let def = golem_def(&component_name, &component.component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinition = cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let cfg = &cli.config;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "get",
        &cfg.arg('i', "id"),
        &component_name,
        &cfg.arg('V', "version"),
        "0.1.0",
    ])?;

    let component_end = &component.component_id[component.component_id.len() - 7..];

    let expected =
        formatdoc!(
            "
            API Definition with ID {component_name} and version 0.1.0.
            Routes:
            +--------+------------------------------+-------------+--------------------------------+
            | Method | Path                         | ComponentId | WorkerName                     |
            +--------+------------------------------+-------------+--------------------------------+
            | Get    | /{{user-id}}/get-cart-contents |    *{component_end} | worker-${{request.path.user-id}} |
            +--------+------------------------------+-------------+--------------------------------+
            "
        );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}

fn text_api_deployment_deploy(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = crate::api_deployment::make_definition(
        deps,
        &cli,
        &format!("text_api_deployment_deploy{name}"),
    )?;
    let host = format!("text-deploy-host{name}");
    let cfg = &cli.config;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-deployment",
        "deploy",
        &cfg.arg('d', "definition"),
        &format!("{}/{}", &definition.id, &definition.version),
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let expected = formatdoc!(
        "
            API deployment on sdomain.{host} with definition {}/{}
            ",
        definition.id,
        definition.version,
    );

    assert_eq!(res, expected);

    Ok(())
}

fn text_api_deployment_get(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = crate::api_deployment::make_definition(
        deps,
        &cli,
        &format!("text_api_deployment_get{name}"),
    )?;
    let host = format!("text-get-host{name}");
    let cfg = &cli.config;

    let _: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('d', "definition"),
        &format!("{}/{}", &definition.id, &definition.version),
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-deployment",
        "get",
        &format!("sdomain.{host}"),
    ])?;

    let expected = formatdoc!(
        "
            API deployment on sdomain.{host} with definition {}/{}
            ",
        definition.id,
        definition.version,
    );

    assert_eq!(res, expected);

    Ok(())
}

fn text_api_deployment_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let definition = crate::api_deployment::make_definition(
        deps,
        &cli,
        &format!("text_api_deployment_list{name:_>9}"),
    )?;
    let host = format!("text-list-host{name:->9}");
    let cfg = &cli.config;

    let _: ApiDeployment = cli.run(&[
        "api-deployment",
        "deploy",
        &cfg.arg('d', "definition"),
        &format!("{}/{}", &definition.id, &definition.version),
        &cfg.arg('H', "host"),
        &host,
        &cfg.arg('s', "subdomain"),
        "sdomain",
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-deployment",
        "list",
        &cfg.arg('i', "id"),
        &definition.id,
    ])?;

    let expected = formatdoc!(
        "
        +---------------------------------+-----------------------------------+---------+
        | Site                            | Definition ID                     | Version |
        +---------------------------------+-----------------------------------+---------+
        | sdomain.{host} | {} | {}   |
        +---------------------------------+-----------------------------------+---------+
        ",
        definition.id,
        definition.version,
    );

    assert_eq!(strip_ansi_escapes::strip_str(res), expected);

    Ok(())
}
