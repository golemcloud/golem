// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::{EnvVarBuilder, SupportedServiceLaunch};
use std::collections::HashMap;
use test_r::test;
use tracing::Level;

#[test]
fn supported_service_listener_identity() {
    let executable = std::env::current_exe().unwrap();
    [
        "registry_service",
        "component_compilation_service",
        "shard_manager",
        "worker_executor",
        "worker_service",
    ]
    .into_iter()
    .for_each(|service| {
        let leaves: &[&str] = if service == "worker_service" {
            &["PORT", "GRPC__PORT", "CUSTOM_REQUEST_PORT", "MCP_PORT"]
        } else {
            &["HTTP_PORT", "GRPC__PORT"]
        };
        let values = [18301, 19301, 18409, 19419];
        let env: HashMap<String, String> = leaves
            .iter()
            .zip(values)
            .map(|(leaf, port)| {
                (
                    format!("GOLEM_ADVANCED__{}__{leaf}", service.to_uppercase()),
                    port.to_string(),
                )
            })
            .collect();
        let launch = SupportedServiceLaunch::new(
            service,
            &executable,
            executable.parent().unwrap().into(),
            env.clone(),
        );
        if service == "worker_service" {
            assert_eq!(launch.listeners::<4>(service), values);
        } else {
            assert_eq!(launch.listeners::<2>(service), [18301, 19301]);
        }
        assert!(std::panic::catch_unwind(|| launch.listeners::<2>("not_this_service")).is_err());
        leaves.iter().for_each(|leaf| {
            let key = format!("GOLEM_ADVANCED__{}__{leaf}", service.to_uppercase());
            let mut missing = env.clone();
            missing.remove(&key);
            let missing = SupportedServiceLaunch::new(
                service,
                &executable,
                executable.parent().unwrap().into(),
                missing,
            );
            assert!(
                std::panic::catch_unwind(|| {
                    if service == "worker_service" {
                        missing.listeners::<4>(service);
                    } else {
                        missing.listeners::<2>(service);
                    }
                })
                .is_err()
            );
            ["0", "65536", "018301", "{port=18301}"]
                .into_iter()
                .for_each(|value| {
                    let mut invalid = env.clone();
                    invalid.insert(key.clone(), value.into());
                    assert!(
                        std::panic::catch_unwind(|| SupportedServiceLaunch::new(
                            service,
                            &executable,
                            executable.parent().unwrap().into(),
                            invalid
                        ))
                        .is_err()
                    );
                });
        });
    });
}

#[test]
fn supported_service_restart_rejects_noncanonical_advanced_inputs() {
    let executable = std::env::current_exe().unwrap();
    let launch = SupportedServiceLaunch::new(
        "worker_executor",
        &executable,
        executable.parent().unwrap().to_path_buf(),
        HashMap::from([
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT".to_string(),
                "19301".to_string(),
            ),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT".to_string(),
                "18301".to_string(),
            ),
        ]),
    );
    let rejected = [
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__LIMITS__FUEL_TO_BORROW",
            "7",
        ),
        (
            "GOLEM__ADVANCED__WORKER_EXECUTOR__LIMITS__FUEL_TO_BORROW",
            "7",
        ),
        ("golem_advanced__worker_executor__grpc__port", "19301"),
        ("GOLEM_ADVANCED__WORKER_SERVICE__GRPC__PORT", "19301"),
        ("GOLEM_ADVANCED__WORKER_EXECUTOR__UNREVIEWED__PORT", "19301"),
        ("GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC", "{port=19301}"),
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT",
            "{port=19301}",
        ),
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__REGISTRY_SERVICE__HOST",
            "{host=localhost}",
        ),
        ("GOLEM_ADVANCED", "{worker_executor={grpc={port=19301}}}"),
        ("GOLEM__ADVANCED", "{worker_executor={grpc={port=19301}}}"),
        ("GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT", "19302"),
        ("GOLEM_ADVANCED__WORKER_EXECUTOR__HTTP_PORT", "18302"),
    ];
    rejected.into_iter().for_each(|(key, value)| {
        let rejected =
            std::panic::catch_unwind(|| launch.command(&[(key.to_string(), value.to_string())]));
        assert!(rejected.is_err(), "accepted {key}");
    });
    let command = launch.command(&[(
        "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT".to_string(),
        "19302".to_string(),
    )]);
    assert_eq!(
        command.get_program(),
        executable.canonicalize().unwrap().as_os_str()
    );
    assert_eq!(
        command
            .get_envs()
            .find(|(key, _)| *key == "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT")
            .unwrap()
            .1
            .unwrap(),
        "19302"
    );
    let key = "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT".to_string();
    launch.command(&[(key.clone(), "19302".into()), (key.clone(), "19301".into())]);
    assert!(
        std::panic::catch_unwind(
            || launch.command(&[(key.clone(), "19301".into()), (key.clone(), "19302".into())])
        )
        .is_err()
    );
    let original = launch.command(&[]);
    assert!(
        !original
            .get_envs()
            .any(|(key, _)| key == "GOLEM_ADVANCED__WORKER_EXECUTOR__SHARD_MANAGER__PORT")
    );
    assert_eq!(
        original
            .get_envs()
            .find(|(key, _)| *key == "GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT")
            .unwrap()
            .1
            .unwrap(),
        "19301"
    );
}

#[test]
fn service_environment_forwards_supported_inputs_and_applies_overrides() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        [
            ("GOLEM__LOGGING__FILE_NAME", "forwarded.log"),
            ("GOLEM__LOGGING__STDOUT__ANSI", "true"),
            ("GOLEM__LOGGING__STDOUT__ENABLED", "false"),
            ("GOLEM__LOGGING__STDOUT__JSON", "false"),
            ("GOLEM__LOGGING__STDOUT__JSON_FLATTEN", "true"),
            ("GOLEM__LOGGING__STDOUT__JSON_FLATTEN_SPAN", "true"),
            ("GOLEM__LOGGING__STDOUT__JSON_SOURCE_LOCATION", "false"),
            ("GOLEM__OTLP__ENABLED", "false"),
            ("GOLEM__OTLP__HOST", "inherited-collector"),
            ("GOLEM__OTLP__PORT", "7321"),
            ("GOLEM__OTLP__SERVICE_NAME", "inherited-service"),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__OPLOG__MAX_OPERATIONS",
                "79",
            ),
            ("GOLEM__LOGGING_OTHER", "excluded"),
            ("GOLEM__OTLP_OTHER", "excluded"),
            ("GOLEM_ADVANCED__WORKER_EXECUTOR__OPLOG_OTHER", "excluded"),
            (
                "GOLEM_ADVANCED__WORKER_SERVICE__OPLOG__MAX_OPERATIONS",
                "excluded",
            ),
            ("GOLEM__DATA_DIR", "excluded"),
        ]
        .into_iter()
        .for_each(|(name, value)| jail.set_env(name, value));

        let inherited = EnvVarBuilder::golem_service(Level::WARN)
            .with_optional_otlp("requested-service", false)
            .build();
        [
            ("GOLEM__LOGGING__FILE_NAME", "forwarded.log"),
            ("GOLEM__LOGGING__STDOUT__ANSI", "false"),
            ("GOLEM__LOGGING__STDOUT__ENABLED", "true"),
            ("GOLEM__LOGGING__STDOUT__JSON", "true"),
            ("GOLEM__LOGGING__STDOUT__JSON_FLATTEN", "false"),
            ("GOLEM__LOGGING__STDOUT__JSON_FLATTEN_SPAN", "false"),
            ("GOLEM__LOGGING__STDOUT__JSON_SOURCE_LOCATION", "true"),
            ("GOLEM__OTLP__ENABLED", "false"),
            ("GOLEM__OTLP__HOST", "inherited-collector"),
            ("GOLEM__OTLP__PORT", "7321"),
            ("GOLEM__OTLP__SERVICE_NAME", "inherited-service"),
            (
                "GOLEM_ADVANCED__WORKER_EXECUTOR__OPLOG__MAX_OPERATIONS",
                "79",
            ),
        ]
        .into_iter()
        .for_each(|(name, value)| {
            assert_eq!(
                inherited.get(name).map(String::as_str),
                Some(value),
                "{name}"
            );
        });
        [
            "GOLEM__LOGGING_OTHER",
            "GOLEM__OTLP_OTHER",
            "GOLEM_ADVANCED__WORKER_EXECUTOR__OPLOG_OTHER",
            "GOLEM_ADVANCED__WORKER_SERVICE__OPLOG__MAX_OPERATIONS",
            "GOLEM__DATA_DIR",
        ]
        .into_iter()
        .for_each(|name| assert!(!inherited.contains_key(name), "{name}"));

        let enabled = EnvVarBuilder::golem_service(Level::WARN)
            .with_optional_otlp("requested-service", true)
            .build();
        [
            ("GOLEM__OTLP__ENABLED", "true"),
            ("GOLEM__OTLP__HOST", "localhost"),
            ("GOLEM__OTLP__PORT", "4318"),
            ("GOLEM__OTLP__SERVICE_NAME", "requested-service"),
        ]
        .into_iter()
        .for_each(|(name, value)| {
            assert_eq!(enabled.get(name).map(String::as_str), Some(value), "{name}");
        });
        Ok(())
    });
}
