// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use test_r::{add_test, inherit_test_dep, test_gen};

use crate::cli::{Cli, CliLive};
use crate::Tracing;
use assert2::assert;
use golem_cli::command::profile::{ProfileType, ProfileView};
use golem_cli::config::{ProfileConfig, ProfileName};
use golem_cli::model::Format;
use golem_test_framework::config::EnvBasedTestDependencies;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use test_r::core::{DynamicTestRegistration, TestType};
use url::Url;

inherit_test_dep!(EnvBasedTestDependencies);
inherit_test_dep!(Tracing);

#[derive(Debug, Clone, Copy)]
enum ArgsKind {
    Short,
    Long,
}

impl Display for ArgsKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgsKind::Short => write!(f, "short"),
            ArgsKind::Long => write!(f, "long"),
        }
    }
}

#[test_gen]
fn generated(r: &mut DynamicTestRegistration) {
    make(r, ArgsKind::Short);
    make(r, ArgsKind::Long);
}

fn make(r: &mut DynamicTestRegistration, args_kind: ArgsKind) {
    add_test!(
        r,
        format!("profile_add_get_list_switch_delete_{args_kind}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, _tracing: &Tracing| {
            profile_add_get_list_switch_delete((deps, args_kind))
        }
    );
    add_test!(
        r,
        format!("profile_config_{args_kind}"),
        TestType::IntegrationTest,
        move |deps: &EnvBasedTestDependencies, _tracing: &Tracing| {
            profile_config((deps, args_kind))
        }
    );
}

fn profile_add_get_list_switch_delete(
    (deps, kind): (&EnvBasedTestDependencies, ArgsKind),
) -> Result<(), anyhow::Error> {
    let name = format!("profile_add_get_list_switch_delete_{kind}");

    let cli = CliLive::make(&name, Arc::new(deps.clone()))?;
    let cli = match kind {
        ArgsKind::Short => cli.with_short_args(),
        ArgsKind::Long => cli.with_long_args(),
    };

    let cfg = &cli.config;

    cli.run_unit(&[
        "profile",
        "add",
        &cfg.arg('s', "set-active"),
        &cfg.arg('c', "component-url"),
        "http://localhost:9876",
        &cfg.arg('w', "worker-url"),
        "http://localhost:9875",
        &cfg.arg('a', "allow-insecure"),
        &cfg.arg('f', "default-format"),
        "yaml",
        "p_with_worker_url",
    ])?;

    let p_with_worker_url: ProfileView = cli.run(&["profile", "get", "p_with_worker_url"])?;

    let expected = ProfileView {
        is_active: true,
        name: ProfileName("p_with_worker_url".to_string()),
        typ: ProfileType::Golem,
        url: Some(Url::parse("http://localhost:9876")?),
        cloud_url: None,
        worker_url: Some(Url::parse("http://localhost:9875")?),
        allow_insecure: true,
        authenticated: None,
        config: ProfileConfig {
            default_format: Format::Yaml,
        },
    };

    assert_eq!(p_with_worker_url, expected);

    cli.run_unit(&[
        "profile",
        "add",
        &cfg.arg('c', "component-url"),
        "http://localhost:9874",
        "p_no_worker_url",
    ])?;

    let p_no_worker_url: ProfileView = cli.run(&["profile", "get", "p_no_worker_url"])?;

    let expected = ProfileView {
        is_active: false,
        name: ProfileName("p_no_worker_url".to_string()),
        typ: ProfileType::Golem,
        url: Some(Url::parse("http://localhost:9874")?),
        cloud_url: None,
        worker_url: None,
        allow_insecure: false,
        authenticated: None,
        config: ProfileConfig::default(),
    };

    assert_eq!(p_no_worker_url, expected);

    cli.run_unit(&[
        "profile",
        "add",
        &cfg.arg('c', "component-url"),
        "http://localhost:9873",
        "p_2",
    ])?;

    let p_2: ProfileView = cli.run(&["profile", "get", "p_2"])?;

    let expected = ProfileView {
        is_active: false,
        name: ProfileName("p_2".to_string()),
        typ: ProfileType::Golem,
        url: Some(Url::parse("http://localhost:9873")?),
        cloud_url: None,
        worker_url: None,
        allow_insecure: false,
        authenticated: None,
        config: ProfileConfig::default(),
    };

    assert_eq!(p_2, expected);

    let list: Vec<ProfileView> = cli.run(&["profile", "list"])?;

    assert!(list.iter().any(|p| &p.name.0 == "p_with_worker_url"));
    assert!(list.iter().any(|p| &p.name.0 == "p_no_worker_url"));
    assert!(list.iter().any(|p| &p.name.0 == "p_2"));

    cli.run_unit(&["profile", "delete", "p_no_worker_url"])?;

    let list: Vec<ProfileView> = cli.run(&["profile", "list"])?;
    assert!(list.iter().all(|p| &p.name.0 != "p_no_worker_url"));

    let active: ProfileView = cli.run(&["profile", "get"])?;
    assert_eq!(active.name.0, "p_with_worker_url");

    cli.run_unit(&["profile", "switch", "p_2"])?;

    let active: ProfileView = cli.run(&["profile", "get"])?;
    assert_eq!(active.name.0, "p_2");

    assert!(
        cli.run_unit(&["profile", "delete", "p_2"]).is_err(),
        "Can't delete active"
    );

    Ok(())
}

fn profile_config(
    (deps, kind): (&EnvBasedTestDependencies, ArgsKind),
) -> Result<(), anyhow::Error> {
    let name = format!("profile_config_{kind}");

    let cli = CliLive::make(&name, Arc::new(deps.clone()))?;
    let cli = match kind {
        ArgsKind::Short => cli.with_short_args(),
        ArgsKind::Long => cli.with_long_args(),
    };

    let cfg = &cli.config;

    cli.run_unit(&[
        "profile",
        "add",
        &cfg.arg('c', "component-url"),
        "http://localhost:9872",
        "p_config",
    ])?;

    let config: ProfileConfig = cli.run(&[
        "profile",
        "config",
        &cfg.arg('p', "profile"),
        "p_config",
        "show",
    ])?;

    let expected = ProfileConfig::default();

    assert_eq!(config, expected);

    cli.run_unit(&[
        "profile",
        "config",
        &cfg.arg('p', "profile"),
        "p_config",
        "format",
        "json",
    ])?;

    let config: ProfileConfig = cli.run(&[
        "profile",
        "config",
        &cfg.arg('p', "profile"),
        "p_config",
        "show",
    ])?;

    let expected = ProfileConfig {
        default_format: Format::Json,
    };

    assert_eq!(config, expected);

    Ok(())
}
