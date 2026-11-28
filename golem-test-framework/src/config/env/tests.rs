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

use super::{
    ComponentCompilationServiceDescriptor, EnvBasedTestDependencies,
    EnvBasedTestDependenciesDescriptor, RdbDescriptor, RegistryServiceDescriptor,
    ShardManagerDescriptor, TestTempDir, WorkerExecutorDescriptor, WorkerServiceDescriptor,
    locate_service_executable,
};
use crate::components::rdb::borrowed_sqlite::BorrowedSqliteRdb;
use crate::config::{EnvBasedTestDependenciesConfig, TestDependencies, TestServiceInputs};
use golem_common::config::{DbConfig, env_config_provider, server_settings};
use golem_common::model::plan::PlanId;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use test_r::test;
use uuid::Uuid;

#[test]
fn supported_settings_prepare_isolated_service_inputs() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let root = TempDir::new().unwrap();
        let target = root.path().join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        let sqlite = root.path().join("sqlite");
        fs::create_dir_all(&sqlite).unwrap();
        let rdb = BorrowedSqliteRdb::new(&sqlite);
        let config = EnvBasedTestDependenciesConfig {
            service_inputs: TestServiceInputs::SupportedSettings,
            registry_grpc_port: 19301,
            compilation_grpc_port: 19302,
            worker_mcp_port: 19309,
            ..EnvBasedTestDependenciesConfig::default()
        };
        let services = server_settings::SERVICES;
        services.into_iter().try_for_each(|service| {
            jail.clear_env();
            jail.set_env("CARGO_TARGET_DIR", target.display());
            jail.set_env("GOLEM_TEST_GATEWAY_SESSION_STORAGE", "sqlite");
            jail.set_env(
                "GOLEM_ADVANCED__WORKER_EXECUTOR__LIMITS__FUEL_TO_BORROW",
                "7",
            );
            jail.set_env("GOLEM__LOGGING__STDOUT__JSON", "false");
            jail.set_env("GOLEM__LOGGING__FILE_NAME", "inherited.log");
            let slug = service.replace('_', "-");
            let binary = if service == "worker_executor" {
                "worker-executor".to_string()
            } else {
                format!("golem-{slug}")
            };
            fs::write(target.join("debug").join(binary), "").unwrap();
            let root = TempDir::new().unwrap();
            let launch = super::EnvBasedTestDependencies::supported_launch(
                &config,
                root.path(),
                service,
                &rdb,
                0,
            )
            .unwrap();
            let command = launch.command(&[]);
            assert!(PathBuf::from(command.get_program()).is_absolute());
            let working = command.get_current_dir().unwrap();
            assert!(working.starts_with(root.path()));
            let file = working.join("config").join(format!("{slug}.toml"));
            assert_eq!(fs::read_to_string(file).unwrap(), "");
            let env = command
                .get_envs()
                .map(|(key, value)| {
                    (
                        key.to_str().unwrap().to_string(),
                        value.unwrap().to_str().unwrap().to_string(),
                    )
                })
                .collect::<std::collections::HashMap<_, _>>();
            assert!(
                env.keys()
                    .all(|key| { !key.contains("FUEL_TO_BORROW") && !key.contains("STDOUT") })
            );
            assert!(
                env.get("GOLEM__LOGGING__FILE_NAME")
                    .is_none_or(|name| name == "executor-0.jsonl")
            );
            let home = PathBuf::from(&env["HOME"]);
            assert!(home.starts_with(root.path()));
            assert_eq!(fs::read_dir(home).unwrap().count(), 0);
            jail.clear_env();
            env.iter().for_each(|(key, value)| jail.set_env(key, value));
            let projected = server_settings::project(service, &env_config_provider());
            if service != "shard_manager" {
                assert_eq!(
                    projected.extract_inner::<String>("blob_storage.config.root")?,
                    root.path().join("blob_storage").to_str().unwrap(),
                );
            }
            match service {
                "registry_service" => assert_eq!(
                    projected.extract_inner::<u16>("component_compilation.config.port")?,
                    19302,
                ),
                "component_compilation_service" => assert_eq!(
                    projected.extract_inner::<u16>("registry_service.config.port")?,
                    19301,
                ),
                "worker_service" => {
                    assert_eq!(projected.extract_inner::<u16>("mcp_port")?, 19309);
                    let db = projected.extract_inner::<DbConfig>("gateway_session_storage")?;
                    match db {
                        DbConfig::Sqlite(db) => assert_eq!(
                            PathBuf::from(db.database),
                            sqlite.join("gateway-sessions.db"),
                        ),
                        other => panic!("expected supported SQLite sessions, got {other:?}"),
                    }
                }
                "worker_executor" => {
                    assert!(PathBuf::from(&env["GOLEM__LOGGING__FILE_DIR"]).is_dir());
                    assert!(projected.extract_inner::<bool>("tracing.file.json_source_location")?);
                    assert!(!projected.extract_inner::<bool>("tracing.file.json_flatten")?);
                    assert!(!projected.extract_inner::<bool>("tracing.file.json_flatten_span")?);
                    assert!(projected.extract_inner::<bool>("tracing.file.json")?);
                }
                _ => {}
            }
            Ok::<(), figment::Error>(())
        })?;
        Ok(())
    });
}

#[test]
fn supported_settings_prepare_postgres_and_redis_inputs() {
    use crate::components::rdb::PostgresInfo;
    use crate::components::rdb::provided_postgres::ProvidedPostgresRdb;
    use golem_common::config::RedisConfig;

    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let root = TempDir::new().unwrap();
        let target = root.path().join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        let rdb = ProvidedPostgresRdb::new(PostgresInfo {
            public_host: "198.51.100.41".into(),
            public_port: 15437,
            private_host: "203.0.113.42".into(),
            private_port: 25439,
            database_name: "prepared_state".into(),
            username: "prepared_user".into(),
            password: "fixture-only".into(),
        });
        let config = EnvBasedTestDependenciesConfig {
            service_inputs: TestServiceInputs::SupportedSettings,
            registry_http_port: 18411,
            registry_grpc_port: 19413,
            compilation_http_port: 18417,
            compilation_grpc_port: 19419,
            shard_manager_http_port: 18423,
            shard_manager_grpc_port: 19429,
            worker_http_port: 18431,
            worker_grpc_port: 19433,
            worker_custom_request_port: 18437,
            worker_mcp_port: 19441,
            executor_base_http_port: 18443,
            executor_base_grpc_port: 19447,
            redis_host: "198.51.100.27".into(),
            redis_port: 16391,
            redis_key_prefix: "tracer-prepared:".into(),
            ..EnvBasedTestDependenciesConfig::default()
        };
        [
            (
                "registry_service",
                "golem-registry-service",
                0,
                "http_port",
                18411,
                19413,
                "processes/registry-service-0",
            ),
            (
                "component_compilation_service",
                "golem-component-compilation-service",
                0,
                "http_port",
                18417,
                19419,
                "processes/component-compilation-service-0",
            ),
            (
                "shard_manager",
                "golem-shard-manager",
                0,
                "http_port",
                18423,
                19429,
                "processes/shard-manager-0",
            ),
            (
                "worker_service",
                "golem-worker-service",
                0,
                "port",
                18431,
                19433,
                "processes/worker-service-0",
            ),
            (
                "worker_executor",
                "worker-executor",
                5,
                "http_port",
                18448,
                19452,
                "processes/worker-executor-5",
            ),
        ]
        .into_iter()
        .try_for_each(
            |(service, binary, index, http_key, http, grpc, process_dir)| {
                jail.clear_env();
                jail.set_env("CARGO_TARGET_DIR", target.display());
                jail.set_env("GOLEM_TEST_GATEWAY_SESSION_STORAGE", "redis");
                let executable = target.join("debug").join(binary);
                fs::write(&executable, "").unwrap();
                let launch = EnvBasedTestDependencies::supported_launch(
                    &config,
                    root.path(),
                    service,
                    &rdb,
                    index,
                )
                .unwrap();
                let command = launch.command(&[]);
                assert_eq!(
                    PathBuf::from(command.get_program()),
                    executable.canonicalize().unwrap()
                );
                let process_dir = root.path().join(process_dir);
                let working = process_dir.join("work");
                assert_eq!(command.get_current_dir(), Some(working.as_path()));
                let config_name = binary.strip_prefix("golem-").unwrap_or(binary);
                let config_file = working.join("config").join(format!("{config_name}.toml"));
                assert_eq!(fs::read_to_string(config_file).unwrap(), "");
                let env = command
                    .get_envs()
                    .map(|(key, value)| {
                        (
                            key.to_str().unwrap().to_string(),
                            value.unwrap().to_str().unwrap().to_string(),
                        )
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                assert_eq!(PathBuf::from(&env["HOME"]), process_dir.join("home"));
                assert_eq!(env["HOME"], env["USERPROFILE"]);
                assert_eq!(fs::read_dir(process_dir.join("home")).unwrap().count(), 0);
                assert_eq!(env["GOLEM__STATE_STORE"], "postgres");
                jail.clear_env();
                env.iter().for_each(|(key, value)| jail.set_env(key, value));
                let projected = server_settings::project(service, &env_config_provider());
                assert_eq!(projected.extract_inner::<u16>(http_key)?, http);
                assert_eq!(projected.extract_inner::<u16>("grpc.port")?, grpc);
                if service != "shard_manager" {
                    assert_eq!(
                        projected.extract_inner::<PathBuf>("blob_storage.config.root")?,
                        root.path().join("blob_storage")
                    );
                }
                let stores: &[(&str, &str)] = match service {
                    "registry_service" => &[("db", "golem_registry_service")],
                    "shard_manager" => &[("persistence", "golem_shard_manager")],
                    "worker_executor" => &[
                        ("key_value_storage", "golem_worker_executor"),
                        ("indexed_storage", "golem_worker_executor_indexed"),
                        ("scheduler_storage", "golem_worker_executor_scheduler"),
                    ],
                    _ => &[],
                };
                stores.iter().try_for_each(|(path, schema)| {
                    let DbConfig::Postgres(db) = projected.extract_inner::<DbConfig>(path)? else {
                        panic!("expected PostgreSQL at {service}.{path}")
                    };
                    assert_eq!(db.host, "198.51.100.41");
                    assert_eq!(db.port, 15437);
                    assert_eq!(db.database, "prepared_state");
                    assert_eq!(db.username, "prepared_user");
                    assert_eq!(db.password, "fixture-only");
                    assert_eq!(db.max_connections, 20);
                    assert_eq!(db.schema.as_deref(), Some(*schema));
                    Ok::<(), figment::Error>(())
                })?;
                match service {
                    "worker_service" => {
                        assert_eq!(
                            projected.extract_inner::<u16>("custom_request_port")?,
                            18437
                        );
                        assert_eq!(projected.extract_inner::<u16>("mcp_port")?, 19441);
                        assert_eq!(
                            projected.extract_inner::<String>("gateway_session_storage.type")?,
                            "Redis"
                        );
                        let redis = projected
                            .extract_inner::<RedisConfig>("gateway_session_storage.config")?;
                        assert_eq!(redis.host, "198.51.100.27");
                        assert_eq!(redis.port, 16391);
                        assert_eq!(redis.key_prefix, "tracer-prepared:");
                    }
                    "worker_executor" => {
                        assert_eq!(
                            projected.extract_inner::<PathBuf>("tracing.file_dir")?,
                            root.path().join("executor-logs")
                        );
                        assert!(root.path().join("executor-logs").is_dir());
                        assert_eq!(
                            projected.extract_inner::<String>("tracing.file_name")?,
                            "executor-5.jsonl"
                        );
                        assert!(projected.extract_inner::<bool>("tracing.file.enabled")?);
                        assert!(projected.extract_inner::<bool>("tracing.file.json")?);
                        assert!(
                            projected.extract_inner::<bool>("tracing.file.json_source_location")?
                        );
                        assert!(!projected.extract_inner::<bool>("tracing.file.json_flatten")?);
                        assert!(
                            !projected.extract_inner::<bool>("tracing.file.json_flatten_span")?
                        );
                    }
                    _ => {}
                }
                Ok::<(), figment::Error>(())
            },
        )?;
        Ok(())
    });
}

#[test]
fn supported_settings_refuse_suite_policy_overrides() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let root = TempDir::new().unwrap();
        jail.set_env(
            "CARGO_TARGET_DIR",
            root.path().join("unused-target").display(),
        );
        let rdb = BorrowedSqliteRdb::new(&root.path().join("not-created-sqlite"));
        let suite = EnvBasedTestDependenciesConfig {
            service_inputs: TestServiceInputs::Suite,
            environment_state_cache_capacity: Some(17),
            number_of_shards_override: Some(23),
            oplog_archive_interval: Some(std::time::Duration::from_secs(31)),
            ..EnvBasedTestDependenciesConfig::default()
        };
        assert!(
            EnvBasedTestDependencies::supported_launch(
                &suite,
                root.path(),
                "worker_executor",
                &rdb,
                5
            )
            .is_none()
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        [
            EnvBasedTestDependenciesConfig {
                service_inputs: TestServiceInputs::SupportedSettings,
                environment_state_cache_capacity: Some(37),
                ..EnvBasedTestDependenciesConfig::default()
            },
            EnvBasedTestDependenciesConfig {
                service_inputs: TestServiceInputs::SupportedSettings,
                number_of_shards_override: Some(41),
                ..EnvBasedTestDependenciesConfig::default()
            },
            EnvBasedTestDependenciesConfig {
                service_inputs: TestServiceInputs::SupportedSettings,
                oplog_archive_interval: Some(std::time::Duration::from_secs(43)),
                ..EnvBasedTestDependenciesConfig::default()
            },
        ]
        .into_iter()
        .for_each(|config| {
            let failure = std::panic::catch_unwind(|| {
                EnvBasedTestDependencies::supported_launch(
                    &config,
                    root.path(),
                    "worker_executor",
                    &rdb,
                    5,
                )
            })
            .err()
            .expect("supported launch accepted a Suite policy override");
            let message = failure
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| failure.downcast_ref::<&str>().copied())
                .unwrap();
            assert_eq!(
                message,
                "supported-settings fixture cannot override service policy"
            );
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
        });
        Ok(())
    });
}

fn ports(config: &EnvBasedTestDependenciesConfig) -> Vec<u16> {
    let mut ports = vec![
        config.redis_port,
        config.registry_http_port,
        config.registry_grpc_port,
        config.shard_manager_http_port,
        config.shard_manager_grpc_port,
        config.compilation_http_port,
        config.compilation_grpc_port,
        config.worker_http_port,
        config.worker_grpc_port,
        config.worker_custom_request_port,
        config.worker_mcp_port,
    ];
    for index in 0..config.worker_executor_cluster_size as u16 {
        ports.push(config.executor_base_http_port + index);
        ports.push(config.executor_base_grpc_port + index);
    }
    ports
}

#[test]
fn bin_profile_defaults_and_overrides() {
    let defaults = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|_| None);
    assert_eq!(defaults.bin_profile, "debug");
    let repo_root = TempDir::new().unwrap();
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        Some(path) => std::env::current_dir().unwrap().join(path),
        None => repo_root.path().join("target"),
    };
    for profile in ["debug", "dev-ci"] {
        let config =
            EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| match key {
                "GOLEM_TEST_BIN_PROFILE" => Some(profile.to_string()),
                "GOLEM_REPO_ROOT" => Some(repo_root.path().to_str().unwrap().to_string()),
                _ => None,
            });
        assert_eq!(config.bin_profile, profile);
        assert_eq!(config.service_binaries_dir(), target_dir.join(profile));
    }
    for profile in ["", "release", "../debug", "DEV-CI"] {
        assert!(
            std::panic::catch_unwind(|| {
                EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
                    (key == "GOLEM_TEST_BIN_PROFILE").then(|| profile.to_string())
                })
            })
            .is_err(),
            "accepted invalid binary profile {profile}"
        );
    }
}

#[test]
fn port_offset_defaults_and_subflows() {
    let defaults = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|_| None);
    let expected = vec![
        6379, 8081, 9091, 9021, 9020, 8083, 9094, 8082, 9092, 9093, 9095, 9000, 9100, 9001, 9101,
        9002, 9102, 9003, 9103,
    ];
    assert_eq!(ports(&defaults), expected);
    let mut seen = std::collections::HashSet::new();
    for offset in [0u16, 10000, 20000, 30000] {
        let config = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
            (key == "GOLEM_TEST_PORT_OFFSET").then(|| offset.to_string())
        });
        assert_eq!(
            ports(&config),
            expected
                .iter()
                .map(|port| port + offset)
                .collect::<Vec<_>>()
        );
        for port in ports(&config) {
            assert!(seen.insert(port), "subflows share port {port}");
        }
    }
}

#[test]
fn port_offset_explicit_ports_are_absolute() {
    for offset in [None, Some("10000")] {
        let config = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
            match key {
                "GOLEM_TEST_PORT_OFFSET" => offset,
                "REDIS_PORT" => Some("12345"),
                "REGISTRY_HTTP_PORT" => Some("23456"),
                _ => None,
            }
            .map(str::to_string)
        });
        assert_eq!(config.redis_port, 12345);
        assert_eq!(config.registry_http_port, 23456);
        assert_eq!(
            config.registry_grpc_port,
            if offset.is_some() { 19091 } else { 9091 }
        );
    }
}

#[test]
fn port_offset_invalid_and_overflow_are_rejected() {
    for offset in ["", "invalid", "-1", "65536", "60000", "56433"] {
        assert!(
            std::panic::catch_unwind(|| {
                EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
                    (key == "GOLEM_TEST_PORT_OFFSET").then(|| offset.to_string())
                })
            })
            .is_err(),
            "accepted invalid or overflowing offset {offset}"
        );
    }
    let config = EnvBasedTestDependenciesConfig::default().with_env_overrides_from(|key| {
        (key == "GOLEM_TEST_PORT_OFFSET").then(|| "56432".to_string())
    });
    assert_eq!(ports(&config).into_iter().max(), Some(u16::MAX));
}

#[test]
fn service_executable_prefers_the_test_binary_profile_directory() {
    let target = TempDir::new().unwrap();
    let release = target.path().join("release");
    let debug = target.path().join("debug");
    fs::create_dir_all(&release).unwrap();
    fs::create_dir_all(&debug).unwrap();
    fs::write(release.join("golem-worker-service"), b"").unwrap();
    fs::write(debug.join("golem-worker-service"), b"").unwrap();

    assert_eq!(
        locate_service_executable(Some(&release), &debug, "golem-worker-service"),
        release.join("golem-worker-service")
    );
}

#[test]
fn service_executable_falls_back_to_the_shared_services_directory() {
    let archive = TempDir::new().unwrap();
    let shared = TempDir::new().unwrap();
    fs::write(shared.path().join("golem-worker-service"), b"").unwrap();

    assert_eq!(
        locate_service_executable(Some(archive.path()), shared.path(), "golem-worker-service"),
        shared.path().join("golem-worker-service")
    );
    assert_eq!(
        locate_service_executable(None, shared.path(), "golem-worker-service"),
        shared.path().join("golem-worker-service")
    );
}

#[test]
fn owned_drop_deletes_directory() {
    let td = TempDir::new().unwrap();
    let path = td.path().to_path_buf();
    let marker = path.join("marker");
    fs::write(&marker, b"x").unwrap();

    let wrapper = TestTempDir::Owned(td);
    assert_eq!(wrapper.path(), path);
    drop(wrapper);

    assert!(
        !path.exists(),
        "TestTempDir::Owned must delete the underlying TempDir on drop",
    );
}

#[test]
fn borrowed_drop_does_not_delete_directory() {
    // Simulates the worker-side reconstruction: dropping the
    // borrowed handle must NOT delete the parent-owned tree.
    let td = TempDir::new().unwrap();
    let path = td.path().to_path_buf();
    let marker = path.join("marker");
    fs::write(&marker, b"parent-owned").unwrap();

    {
        let wrapper = TestTempDir::Borrowed(path.clone());
        assert_eq!(wrapper.path(), path);
    }

    assert!(
        path.exists(),
        "TestTempDir::Borrowed must NOT delete the parent-owned directory on drop",
    );
    let contents = fs::read(&marker).unwrap();
    assert_eq!(contents, b"parent-owned");
}

fn sample_descriptor(rdb: RdbDescriptor) -> EnvBasedTestDependenciesDescriptor {
    EnvBasedTestDependenciesDescriptor {
        temp_directory_path: PathBuf::from("/abs/tmp/td"),
        test_component_dir: PathBuf::from("/abs/components"),
        blob_storage_root: PathBuf::from("/abs/blob"),
        redis_host: "rh".to_string(),
        redis_port: 6379,
        redis_prefix: "p".to_string(),
        rdb,
        shard_manager: ShardManagerDescriptor {
            host: "sm".to_string(),
            http_port: 8011,
            grpc_port: 8001,
        },
        component_compilation_service: ComponentCompilationServiceDescriptor {
            host: "cc".to_string(),
            http_port: 8012,
            grpc_port: 8002,
        },
        worker_service: WorkerServiceDescriptor {
            host: "ws".to_string(),
            http_port: 8003,
            grpc_port: 8004,
            custom_request_port: 8005,
            mcp_port: 8006,
        },
        worker_executor_cluster: vec![
            WorkerExecutorDescriptor {
                host: "we1".to_string(),
                grpc_port: 9091,
                http_port: Some(9191),
            },
            WorkerExecutorDescriptor {
                host: "we2".to_string(),
                grpc_port: 9092,
                http_port: None,
            },
        ],
        registry_service: RegistryServiceDescriptor {
            host: "rs".to_string(),
            http_port: 7001,
            grpc_port: 7002,
            admin_account_id: Uuid::nil(),
            admin_account_email: "admin@example.com".to_string(),
            admin_account_token: "tok".to_string(),
            builtin_plugin_owner_account_id: Uuid::nil(),
            default_plan_id: Uuid::nil(),
            low_fuel_plan_id: Some(Uuid::nil()),
            low_disk_space_plan_id: Some(Uuid::nil()),
            low_http_calls_plan_id: Some(Uuid::nil()),
            low_rpc_calls_plan_id: Some(Uuid::nil()),
        },
    }
}

fn assert_descriptor_round_trip(rdb: RdbDescriptor) {
    let original = sample_descriptor(rdb);
    let bytes = serde_json::to_vec(&original).expect("serialize");
    let parsed: EnvBasedTestDependenciesDescriptor =
        serde_json::from_slice(&bytes).expect("deserialize");

    assert_eq!(parsed.temp_directory_path, original.temp_directory_path);
    assert_eq!(parsed.test_component_dir, original.test_component_dir);
    assert_eq!(parsed.blob_storage_root, original.blob_storage_root);
    assert_eq!(parsed.redis_host, original.redis_host);
    assert_eq!(parsed.redis_port, original.redis_port);
    assert_eq!(parsed.redis_prefix, original.redis_prefix);
    assert_eq!(parsed.shard_manager.host, original.shard_manager.host);
    assert_eq!(
        parsed.shard_manager.grpc_port,
        original.shard_manager.grpc_port
    );
    assert_eq!(
        parsed.worker_executor_cluster.len(),
        original.worker_executor_cluster.len()
    );
    for (got, want) in parsed
        .worker_executor_cluster
        .iter()
        .zip(original.worker_executor_cluster.iter())
    {
        assert_eq!(got.host, want.host);
        assert_eq!(got.grpc_port, want.grpc_port);
        assert_eq!(got.http_port, want.http_port);
    }
    assert_eq!(
        parsed.registry_service.admin_account_token,
        original.registry_service.admin_account_token
    );
    // Re-serialise to guarantee stable JSON shape across the
    // round-trip; the only allowed difference is field order, which
    // serde_json preserves for structs.
    let again = serde_json::to_vec(&parsed).expect("re-serialize");
    assert_eq!(again, bytes);
}

#[test]
fn descriptor_serde_round_trip_sqlite() {
    assert_descriptor_round_trip(RdbDescriptor::Sqlite {
        path: PathBuf::from("/abs/sqlite/db"),
    });
}

#[test]
async fn descriptor_optional_suite_plans_preserve_independent_values_and_owners() {
    use futures::StreamExt;
    use test_r::core::AsyncHostedDep;

    let ids = [11, 23, 37, 49].map(|id| Some(Uuid::from_u128(id)));
    futures::stream::iter([
        ids,
        [None; 4],
        [None, ids[1], None, ids[3]],
        [ids[0], None, ids[2], None],
    ])
    .for_each(|plans| async move {
        let owner = TempDir::new().unwrap();
        let marker = owner.path().join("parent-marker");
        fs::write(&marker, b"parent-owned").unwrap();
        let blob_root = owner.path().join("blob_storage");
        fs::create_dir(&blob_root).unwrap();
        let mut descriptor = sample_descriptor(RdbDescriptor::Sqlite {
            path: owner.path().to_path_buf(),
        });
        descriptor.temp_directory_path = owner.path().to_path_buf();
        descriptor.test_component_dir = owner.path().to_path_buf();
        descriptor.blob_storage_root = blob_root;
        descriptor.registry_service.admin_account_id = Uuid::from_u128(101);
        descriptor.registry_service.builtin_plugin_owner_account_id = Uuid::from_u128(103);
        descriptor.registry_service.default_plan_id = Uuid::from_u128(107);
        descriptor.registry_service.low_fuel_plan_id = plans[0];
        descriptor.registry_service.low_disk_space_plan_id = plans[1];
        descriptor.registry_service.low_http_calls_plan_id = plans[2];
        descriptor.registry_service.low_rpc_calls_plan_id = plans[3];
        let bytes = serde_json::to_vec(&descriptor).unwrap();
        let attached = EnvBasedTestDependencies::from_descriptor(&bytes).await;
        let registry = attached.registry_service();
        assert_eq!(registry.admin_account_id().0, Uuid::from_u128(101));
        assert_eq!(
            registry.admin_account_email().to_string(),
            "admin@example.com"
        );
        assert_eq!(
            registry.builtin_plugin_owner_account_id().0,
            Uuid::from_u128(103)
        );
        assert_eq!(registry.default_plan().0, Uuid::from_u128(107));
        assert_eq!(
            [
                registry.low_fuel_plan(),
                registry.low_disk_space_plan(),
                registry.low_http_calls_plan(),
                registry.low_rpc_calls_plan()
            ],
            plans.map(|id| id.map(PlanId))
        );
        let encoded = attached.descriptor();
        let again = EnvBasedTestDependencies::from_descriptor(&encoded).await;
        let registry = again.registry_service();
        assert_eq!(
            [
                registry.low_fuel_plan(),
                registry.low_disk_space_plan(),
                registry.low_http_calls_plan(),
                registry.low_rpc_calls_plan()
            ],
            plans.map(|id| id.map(PlanId))
        );
        assert_eq!(registry.default_plan().0, Uuid::from_u128(107));
        drop(again);
        drop(attached);
        assert_eq!(fs::read(marker).unwrap(), b"parent-owned");
    })
    .await;
}

#[test]
fn descriptor_serde_round_trip_postgres() {
    assert_descriptor_round_trip(RdbDescriptor::Postgres {
        public_host: "pgpub".to_string(),
        public_port: 5432,
        private_host: "pgpriv".to_string(),
        private_port: 5433,
        database_name: "db".to_string(),
        username: "u".to_string(),
        password: "p".to_string(),
    });
}

#[test]
fn descriptor_sqlite_uses_snake_case_kind_tag() {
    // Locks the wire format: workers must see `kind: "sqlite"` /
    // `kind: "postgres"`, so renaming an RDB variant in source is a
    // visible breaking change to the cross-process descriptor
    // exchange.
    let bytes = serde_json::to_string(&sample_descriptor(RdbDescriptor::Sqlite {
        path: PathBuf::from("/abs/sqlite/db"),
    }))
    .unwrap();
    assert!(
        bytes.contains("\"kind\":\"sqlite\""),
        "expected snake_case kind tag in {bytes}",
    );
}

#[test]
fn descriptor_postgres_uses_snake_case_kind_tag() {
    let bytes = serde_json::to_string(&sample_descriptor(RdbDescriptor::Postgres {
        public_host: "h".to_string(),
        public_port: 1,
        private_host: "h".to_string(),
        private_port: 2,
        database_name: "d".to_string(),
        username: "u".to_string(),
        password: "p".to_string(),
    }))
    .unwrap();
    assert!(
        bytes.contains("\"kind\":\"postgres\""),
        "expected snake_case kind tag in {bytes}",
    );
}

#[test]
fn descriptor_canonical_helper_yields_absolute_paths() {
    // Mirrors what `AsyncHostedDep::descriptor` does: passing a
    // relative path through `std::fs::canonicalize` produces an
    // absolute path that worker subprocesses (potentially with a
    // different cwd) can still resolve.
    let td = TempDir::new().unwrap();
    let abs_root = td.path().canonicalize().unwrap();

    let abs = std::fs::canonicalize(td.path()).expect("canonicalize");
    assert!(abs.is_absolute());
    assert_eq!(abs, abs_root);
}
