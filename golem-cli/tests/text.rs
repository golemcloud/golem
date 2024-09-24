use crate::api_definition::{
    golem_def, make_golem_file, make_open_api_file, make_shopping_cart_component,
};
use crate::cli::{Cli, CliLive};
use crate::worker::add_environment_service_component;
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_cli::model::Format;
use golem_client::model::{ApiDeployment, HttpApiDefinitionWithTypeInfo};
use golem_common::model::TargetWorkerId;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
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
    let res = cli.with_format(Format::Text).run_string(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!("(?m)^Added new component {component_name}$")
    ));
    assert_component_fields(&res, None, &component_name, Some(0));

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

    let res = cli.with_format(Format::Text).run_string(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component.component_urn.to_string(),
        env_service.to_str().unwrap(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!("(?m)^Updated component {component_name} to version 1$")
    ));
    assert_component_fields(
        &res,
        Some(component.component_urn),
        &component_name,
        Some(1),
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

    let res = cli.with_format(Format::Text).run_string(&[
        "component",
        "get",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;

    assert!(regex_contains(
        &res,
        &format!("(?m)^Got metadata for component {component_name}$")
    ));
    assert_component_fields(
        &res,
        Some(component.component_urn),
        &component_name,
        Some(0),
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
        +----------------------------------------------------+-------------------------------+---------+-------+---------------+
        | URN                                                | Name                          | Version | Size  | Exports count |
        +----------------------------------------------------+-------------------------------+---------+-------+---------------+
        | {} | {} |       0 | 71228 |             2 |
        +----------------------------------------------------+-------------------------------+---------+-------+---------------+
        ",
        component.component_urn,
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
    let component_urn =
        add_environment_service_component(deps, &format!("{name} text worker add"), &cli)?
            .component_urn;
    let worker_name = format!("{name}_worker_add");
    let cfg = &cli.config;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!("(?m)^Added worker {}$", worker_name),
    ));

    assert!(regex_contains(
        &res,
        &format!(
            "(?m)^Worker URN:.+{}$",
            WorkerUrn {
                id: TargetWorkerId {
                    component_id: component_urn.id.clone(),
                    worker_name: Some(worker_name.clone())
                }
            }
        ),
    ));
    assert!(regex_contains(
        &res,
        &format!("(?m)^Component URN:.+{}$", component_urn),
    ));
    assert!(regex_contains(
        &res,
        &format!("(?m)^Worker name:.+{}$", worker_name),
    ));

    Ok(())
}

fn text_worker_invoke_and_await(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_urn = add_environment_service_component(
        deps,
        &format!("{name} text worker_invoke_and_await"),
        &cli,
    )?
    .component_urn;
    let worker_name = format!("{name}_worker_invoke_and_await");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
        &cfg.arg('e', "env"),
        "TEST_ENV=test-value",
        "test-arg",
    ])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "invoke-and-await",
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
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
    let component_urn =
        add_environment_service_component(deps, &format!("{name} text worker get"), &cli)?
            .component_urn;
    let worker_name = format!("{name}_worker_get");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "get",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!("(?m)^Got metadata for worker {}$", worker_name),
    ));

    assert!(regex_contains(
        &res,
        &format!(
            "(?m)^Worker URN:.+{}$",
            WorkerUrn {
                id: TargetWorkerId {
                    component_id: component_urn.id.clone(),
                    worker_name: Some(worker_name.clone())
                }
            }
        ),
    ));
    assert!(regex_contains(
        &res,
        &format!("(?m)^Component URN:.+{}$", component_urn),
    ));
    assert!(regex_contains(
        &res,
        &format!("(?m)^Worker name:.+{}$", worker_name),
    ));
    assert!(regex_contains(&res, "(?m)^Component version:.+0$"));
    assert!(regex_contains(&res, "(?m)^Created at:.+$"));
    assert!(regex_contains(&res, "(?m)^Component size:.+[0-9]+.*$"));
    assert!(regex_contains(
        &res,
        "(?m)^Total linear memory size:.+[0-9]+.*$"
    ));
    assert!(regex_contains(&res, "(?m)^Status:.+Idle$"));
    assert!(regex_contains(&res, "(?m)^Retry count:.+0$"));

    Ok(())
}

fn text_worker_list(
    (deps, name, cli): (
        Arc<dyn TestDependencies + Send + Sync + 'static>,
        String,
        CliLive,
    ),
) -> Result<(), Failed> {
    let component_urn =
        add_environment_service_component(deps, &format!("{name} text worker list"), &cli)?
            .component_urn;
    let worker_name = format!("{name:_<9}_worker_list");
    let cfg = &cli.config;
    let _: WorkerUrn = cli.run(&[
        "worker",
        "add",
        &cfg.arg('w', "worker-name"),
        &worker_name,
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
    ])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "worker",
        "list",
        &cfg.arg('C', "component"),
        &component_urn.to_string(),
        &cfg.arg('f', "filter"),
        &format!("name = {worker_name}"),
        "--precise",
        "true",
    ])?;

    assert!(regex_contains(&res, r"\|[^|]+Component[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Name[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+version[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Status[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Create at[^|]+|"));
    assert!(regex_contains(
        &res,
        &format!(r"\|\W+{}\W+|", component_urn)
    ));
    assert!(regex_contains(&res, &format!(r"\|\W+{}\W+|", worker_name)));
    assert!(regex_contains(&res, r"\|\W+Idle\W+|"));

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
        "zig",
    ])?;

    assert!(regex_contains(&res, r"\|[^|]+Name[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Language[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Tier[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+Description[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+zig-default[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+zig[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]+zig-default-minimal[^|]+|"));

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
    let component_id = component.component_urn.id.0.to_string();
    let component_version = component.component_version;
    let path = make_open_api_file(&component_name, &component_id, component_version)?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "import",
        path.to_str().unwrap(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!(
            r"(?m)^Imported API definition {} with version {}",
            component_name, "0.1.0"
        )
    ));
    assert_api_definition_fields(&res, &component_name);

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
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(&component_name, &component_id);
    let path = make_golem_file(&def)?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "add",
        path.to_str().unwrap(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!(
            r"(?m)^Added API definition {} with version {}",
            component_name, "0.1.0"
        )
    ));
    assert_api_definition_fields(&res, &component_name);

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
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(&component_name, &component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinitionWithTypeInfo =
        cli.run(&["api-definition", "add", path.to_str().unwrap()])?;
    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "update",
        path.to_str().unwrap(),
    ])?;

    assert!(regex_contains(
        &res,
        &format!(
            r"(?m)^Updated API definition {} with version {}",
            component_name, "0.1.0"
        )
    ));
    assert_api_definition_fields(&res, &component_name);

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
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(&component_name, &component_id);
    let path = make_golem_file(&def)?;
    let cfg = &cli.config;

    let _: HttpApiDefinitionWithTypeInfo =
        cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "list",
        &cfg.arg('i', "id"),
        &component_name,
    ])?;

    assert!(regex_contains(&res, r"\|[^|]ID[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]Version[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]Routes count[^|]+|"));
    assert!(regex_contains(&res, r"\|[^|]0.1.0[^|]+|"));
    assert!(regex_contains(
        &res,
        &format!(r"\|[^|]{}[^|]+|", component_name)
    ));

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
    let component_id = component.component_urn.id.0.to_string();
    let def = golem_def(&component_name, &component_id);
    let path = make_golem_file(&def)?;

    let _: HttpApiDefinitionWithTypeInfo =
        cli.run(&["api-definition", "add", path.to_str().unwrap()])?;

    let cfg = &cli.config;

    let res = cli.with_format(Format::Text).run_string(&[
        "api-definition",
        "get",
        &cfg.arg('i', "id"),
        &component_name,
        &cfg.arg('V', "version"),
        "0.1.0",
    ])?;

    assert!(regex_contains(
        &res,
        &format!(
            r"(?m)^Got metadata for API definition {} version {}",
            component_name, "0.1.0"
        )
    ));
    assert_api_definition_fields(&res, &component_name);

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

fn regex_contains(s: &str, regex: &str) -> bool {
    Regex::new(regex)
        .unwrap_or_else(|err| panic!("Failed to parse regex: {}: {}", regex, err))
        .is_match(s)
}

fn assert_component_fields(
    res: &str,
    component_urn: Option<ComponentUrn>,
    component_name: &str,
    component_version: Option<u64>,
) {
    match component_urn {
        Some(component_urn) => {
            assert!(regex_contains(
                res,
                &format!("(?m)^Component URN:.+{}$", component_urn)
            ));
        }
        None => {
            assert!(regex_contains(res, "(?m)^Component URN:.+$"));
        }
    }

    assert!(regex_contains(
        res,
        &format!("(?m)^Component name:.+{}$", component_name)
    ));
    match component_version {
        Some(component_version) => {
            assert!(regex_contains(
                res,
                &format!("(?m)^Component version:.*{}$", component_version)
            ));
        }
        None => {
            assert!(regex_contains(res, "(?m)^Component version:.*[0-9]+$"));
        }
    }
    assert!(regex_contains(res, "(?m)^Component size:.*[0-9]+.+$"));
    assert!(regex_contains(res, "(?m)^Created at:.+$"));
    assert!(res.contains(
        "golem:it/api.{get-environment}() -> result<list<tuple<string, string>>, string>"
    ));
    assert!(res.contains("golem:it/api.{get-arguments}() -> result<list<string>, string>"));
}

fn assert_api_definition_fields(res: &str, id: &str) {
    assert!(regex_contains(res, &format!(r"(?m)^ID:.+{}$", id)));
    assert!(regex_contains(res, r"(?m)^Version:.+0.1.0$"));
    assert!(regex_contains(res, "(?m)^Created at:.+$"));
    assert!(regex_contains(res, "(?m)^Routes:$"));
    assert!(regex_contains(res, r"\|[^|]Method[^|]+|"));
    assert!(regex_contains(res, r"\|[^|]Path[^|]+|"));
    assert!(regex_contains(res, r"\|[^|]Component URN[^|]+|"));
}
