use crate::cli::{Cli, CliLive};
use crate::components::TestDependencies;
use crate::config::CloudEnvBasedTestDependencies;
use crate::Tracing;
use assert2::assert;
use golem_cli::model::component::ComponentView;
use golem_common::uri::oss::url::ComponentUrl;
use test_r::core::{DynamicTestRegistration, TestProperties, TestType};
use test_r::{add_test, inherit_test_dep, test_dep, test_gen};

inherit_test_dep!(CloudEnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[test_dep]
fn cli(deps: &CloudEnvBasedTestDependencies) -> CliLive {
    CliLive::make("component", deps).unwrap()
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, "_short", "CLI_short", true);
    make(r, "_long", "CLI_long", false);
}

fn make(r: &mut DynamicTestRegistration, suffix: &'static str, name: &'static str, short: bool) {
    add_test!(
        r,
        format!("component_add_and_find_all{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_find_all((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_find_by_name{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_find_by_name((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get_urn{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get_urn((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_add_and_get_url{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_add_and_get_url((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update_urn{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update_urn((deps, name.to_string(), cli.with_args(short)))
        }
    );
    add_test!(
        r,
        format!("component_update_url{suffix}"),
        TestProperties {
            test_type: TestType::IntegrationTest,
            ..TestProperties::default()
        },
        move |deps: &CloudEnvBasedTestDependencies, cli: &CliLive, _tracing: &Tracing| {
            component_update_url((deps, name.to_string(), cli.with_args(short)))
        }
    );
}

fn component_add_and_find_all(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component add and find all");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run_trimmed(&["component", "list"])?;
    assert!(res.contains(&component), "{res:?}.contains({component:?})");
    Ok(())
}

fn component_add_and_find_by_name(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name_other = format!("{name} component add and find by name other");
    let component_name = format!("{name} component add and find by name");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name_other,
        env_service.to_str().unwrap(),
    ])?;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: Vec<ComponentView> = cli.run_trimmed(&[
        "component",
        "list",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    assert!(res.contains(&component), "{res:?}.contains({component:?})");
    assert_eq!(res.len(), 1, "{res:?}.len() == 1");
    Ok(())
}

fn component_update(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component update");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('c', "component-name"),
        &component.component_name,
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_update_urn(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component update urn");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component.component_urn.to_string(),
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_update_url(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component update url");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let component_url = ComponentUrl {
        name: component.component_name.to_string(),
    };

    let _: ComponentView = cli.run_trimmed(&[
        "component",
        "update",
        &cfg.arg('C', "component"),
        &component_url.to_string(),
        env_service.to_str().unwrap(),
    ])?;
    Ok(())
}

fn component_add_and_get(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component add and get");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;
    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('c', "component-name"),
        &component_name,
    ])?;
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_urn(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component add and get urn");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('C', "component"),
        &component.component_urn.to_string(),
    ])?;
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}

fn component_add_and_get_url(
    (deps, name, cli): (
        &(impl TestDependencies + Send + Sync + 'static),
        String,
        CliLive,
    ),
) -> Result<(), anyhow::Error> {
    let component_name = format!("{name} component add and get url");
    let env_service = deps.component_directory().join("environment-service.wasm");
    let cfg = &cli.config;
    let component: ComponentView = cli.run_trimmed(&[
        "component",
        "add",
        &cfg.arg('c', "component-name"),
        &component_name,
        env_service.to_str().unwrap(),
    ])?;

    let component_url = ComponentUrl {
        name: component.component_name.to_string(),
    };

    let res: ComponentView = cli.run_trimmed(&[
        "component",
        "get",
        &cfg.arg('C', "component"),
        &component_url.to_string(),
    ])?;
    assert_eq!(res, component, "{res:?} = ({component:?})");
    Ok(())
}
