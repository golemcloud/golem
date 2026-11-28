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

use super::*;
use figment::Jail;
use golem_cli::model::app_raw::LocalServer;
use golem_registry_service::config::ComponentCompilationConfig;
use golem_shard_manager::config::PersistenceConfig;
use golem_worker_executor::services::golem_config::FilesystemSnapshotsConfig;
use golem_worker_service::mcp::session::McpSessionStorageConfig;
use test_r::test;

/// Loads the configuration on a platform without platform defaults, with the validation-only
/// snapshot repository key.
fn load(
    local_server: Option<&ResolvedLocalServer>,
    args: &RunArgs,
) -> anyhow::Result<GolemServerConfig> {
    load_server_config_on(
        LocalPlatform::Other,
        local_server,
        args,
        SnapshotRepositoryKeySource::ValidationOnly,
    )
}

fn local_server(value: LocalServer) -> ResolvedLocalServer {
    ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
}

fn run_args_with_data_dir(data_dir: &str) -> RunArgs {
    RunArgs {
        data_dir: Some(PathBuf::from(data_dir)),
        ..RunArgs::default()
    }
}

fn blob_root(config: &BlobStorageConfig) -> &Path {
    match config {
        BlobStorageConfig::LocalFileSystem(config) => &config.root,
        other => panic!("expected filesystem blobs, got {other:?}"),
    }
}

fn sqlite_path(config: &DbSqliteConfig) -> &Path {
    Path::new(&config.database)
}

fn jail_error(result: anyhow::Result<GolemServerConfig>) -> String {
    format!(
        "{:#}",
        result.expect_err("expected the configuration to be rejected")
    )
}

fn load_on(
    platform: LocalPlatform,
    local_server: Option<&ResolvedLocalServer>,
    args: &RunArgs,
    snapshot_repository_key: SnapshotRepositoryKeySource,
) -> anyhow::Result<GolemServerConfig> {
    load_server_config_on(platform, local_server, args, snapshot_repository_key)
}

fn snapshot_store(
    config: &GolemServerConfig,
) -> &golem_worker_executor::services::golem_config::FilesystemSnapshotStoreConfig {
    let FilesystemSnapshotsConfig::Enabled(store) = &config.worker_executor.filesystem_snapshots
    else {
        panic!("expected enabled filesystem snapshots")
    };
    store
}

#[test]
fn standalone_variant_switches_discard_replaced_storage_fields() {
    [
            (true, false, false),
            (true, true, false),
            (false, true, false),
            (true, false, true),
            (true, true, true),
            (false, true, true),
        ]
        .into_iter()
        .for_each(|(supported_postgres, selection_in_env, explicit_database)| {
            Jail::expect_with(|jail| {
                jail.clear_env();
                jail.create_dir("config")?;
                let lower = if supported_postgres {
                    "[postgres]"
                } else {
                    "[advanced.shard_manager.persistence]\ntype = 'Postgres'\n[advanced.shard_manager.persistence.config]"
                };
                let mut settings = format!(
                    "{}\n{lower}\nhost = 'localhost'\nport = 5432\nusername = 'golem'\npassword = ''\ndatabase = 'tenant_state'\nmax_connections = 43\n",
                    if supported_postgres { "state_store = 'postgres'" } else { "" }
                );
                if selection_in_env {
                    jail.set_env("GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__TYPE", "Sqlite");
                    if explicit_database {
                        jail.set_env(
                            "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__CONFIG__DATABASE",
                            "selected.db",
                        );
                    }
                } else {
                    settings.push_str("\n[advanced.shard_manager.persistence]\ntype = 'Sqlite'\n");
                    if explicit_database {
                        settings.push_str("[advanced.shard_manager.persistence.config]\ndatabase = 'selected.db'\n");
                    }
                }
                jail.create_file("config/shard-manager.toml", &settings)?;
                let loader = golem_shard_manager::config::make_config_loader();
                let PersistenceConfig::Sqlite(native) = loader
                    .default_figment()
                    .extract_inner::<PersistenceConfig>("persistence")?
                else {
                    panic!("expected native SQLite persistence")
                };
                assert_eq!(native.database, "../data/golem_shard_manager.db");
                assert_eq!(native.max_connections, 10);
                let loaded = loader.load()?;
                let PersistenceConfig::Sqlite(sqlite) = loaded.persistence else {
                    panic!("expected selected SQLite persistence")
                };
                assert_eq!(
                    sqlite.database,
                    if explicit_database {
                        "selected.db"
                    } else if supported_postgres {
                        "../data/golem_shard_manager.db"
                    } else {
                        "/var/lib/golem/shard_manager.db"
                    }
                );
                assert_eq!(sqlite.max_connections, if supported_postgres { 10 } else { 4 });
                assert_eq!(sqlite.foreign_keys, !supported_postgres);
                if supported_postgres {
                    let input = Figment::from(Toml::file_exact("config/shard-manager.toml"))
                        .merge(env_config_provider());
                    let projected = server_settings::project("shard_manager", &input);
                    assert!(projected.find_value("persistence.config.host").is_err());
                    assert!(projected.find_value("persistence.config.max_connections").is_err());
                    assert_eq!(
                        projected.extract_inner::<String>("persistence.config.database").ok().as_deref(),
                        explicit_database.then_some("selected.db")
                    );
                }
                assert!(!Path::new("tenant_state").exists());
                assert!(!Path::new("selected.db").exists());
                Ok(())
            });
        });
}

#[test]
fn standalone_variant_switch_requires_fields_without_matching_defaults() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_dir("config")?;
        jail.create_file(
            "config/shard-manager.toml",
            r#"
                    [advanced.shard_manager.persistence]
                    type = "Postgres"
                    [advanced.shard_manager.persistence.config]
                    host = "localhost"
                    port = 5432
                    username = "golem"
                    password = ""
                    max_connections = 43
                "#,
        )?;
        let error = golem_shard_manager::config::make_config_loader()
            .load()
            .unwrap_err()
            .to_string();
        assert!(error.contains("database"), "{error}");
        assert!(error.contains("persistence"), "{error}");
        assert!(error.contains("shard-manager.toml"), "{error}");
        Ok(())
    });
}

#[test]
fn standalone_variant_switches_keep_fieldless_variants() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_dir("config")?;
        jail.create_file(
            "config/worker-executor.toml",
            r#"
                    [advanced.worker_executor.http_client]
                    type = "Disabled"
                    [advanced.worker_executor.oplog.oplog_processor_snapshotting]
                    type = "Disabled"
                "#,
        )?;
        let loaded = golem_worker_executor::services::golem_config::make_config_loader().load()?;
        assert!(matches!(
            loaded.http_client,
            golem_worker_executor::services::golem_config::HttpClientConfig::Disabled(_)
        ));
        assert!(matches!(
            loaded.oplog.oplog_processor_snapshotting,
            golem_worker_executor::services::golem_config::SnapshotPolicy::Disabled
        ));
        Ok(())
    });
}

#[test]
fn etcd_tls_survives_advanced_variant_selection_in_both_loaders() {
    [false, true].into_iter().for_each(|selection_in_env| {
        Jail::expect_with(|jail| {
            jail.create_dir("config")?;
            let tls = r#"
                    [etcd.tls]
                    ca_cert_file = "/certs/etcd-ca.pem"
                    client_cert_file = "/certs/etcd-client.pem"
                    client_key_file = "/certs/etcd-client.key"
                    server_name = "etcd.internal"
                "#;
            let selection = r#"
                    [advanced.shard_manager.persistence]
                    type = "Etcd"
                    [advanced.shard_manager.persistence.config]
                    endpoints = ["https://etcd.internal:2379"]
                    username = "etcd-user"
                "#;
            let settings = format!(
                "state_store = \"postgres\"\n{}",
                if selection_in_env { tls } else { selection }
            );
            if selection_in_env {
                jail.set_env("GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__TYPE", "Etcd");
                jail.set_env(
                    "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__CONFIG__ENDPOINTS",
                    r#"["https://etcd.internal:2379"]"#,
                );
                jail.set_env(
                    "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__CONFIG__USERNAME",
                    "etcd-user",
                );
            } else {
                jail.set_env("GOLEM__ETCD__TLS__CA_CERT_FILE", "/certs/etcd-ca.pem");
                jail.set_env(
                    "GOLEM__ETCD__TLS__CLIENT_CERT_FILE",
                    "/certs/etcd-client.pem",
                );
                jail.set_env(
                    "GOLEM__ETCD__TLS__CLIENT_KEY_FILE",
                    "/certs/etcd-client.key",
                );
                jail.set_env("GOLEM__ETCD__TLS__SERVER_NAME", "etcd.internal");
            }
            jail.create_file("settings.toml", &settings)?;
            jail.create_file("config/shard-manager.toml", &settings)?;
            let standalone = golem_shard_manager::config::make_config_loader().load()?;
            let embedded = load(
                None,
                &RunArgs {
                    config: Some("settings.toml".into()),
                    ..run_args_with_data_dir("/data")
                },
            )
            .unwrap();
            [&standalone.persistence, &embedded.shard_manager.persistence]
                .into_iter()
                .for_each(|persistence| {
                    let PersistenceConfig::Etcd(etcd) = persistence else {
                        panic!("expected etcd persistence")
                    };
                    assert_eq!(etcd.username.as_deref(), Some("etcd-user"));
                    let tls = etcd.tls.as_ref().expect("supported etcd TLS must survive");
                    assert_eq!(
                        tls.ca_cert_file.as_deref(),
                        Some(Path::new("/certs/etcd-ca.pem"))
                    );
                    assert_eq!(
                        tls.client_cert_file.as_deref(),
                        Some(Path::new("/certs/etcd-client.pem"))
                    );
                    assert_eq!(
                        tls.client_key_file.as_deref(),
                        Some(Path::new("/certs/etcd-client.key"))
                    );
                    assert_eq!(tls.server_name.as_deref(), Some("etcd.internal"));
                });
            Ok(())
        });
    });
}

#[test]
fn repeated_etcd_selection_keeps_lower_advanced_tls_overrides() {
    Jail::expect_with(|jail| {
        jail.create_dir("config")?;
        let settings = r#"
                [etcd.tls]
                server_name = "supported.internal"
                [advanced.shard_manager.persistence]
                type = "Etcd"
                [advanced.shard_manager.persistence.config]
                endpoints = ["https://etcd.internal:2379"]
                [advanced.shard_manager.persistence.config.tls]
                server_name = "advanced.internal"
            "#;
        jail.set_env("GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__TYPE", "Etcd");
        jail.create_file("settings.toml", settings)?;
        jail.create_file("config/shard-manager.toml", settings)?;
        let standalone = golem_shard_manager::config::make_config_loader().load()?;
        let embedded = load(
            None,
            &RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            },
        )
        .unwrap();
        [&standalone.persistence, &embedded.shard_manager.persistence]
            .into_iter()
            .for_each(|persistence| {
                let PersistenceConfig::Etcd(etcd) = persistence else {
                    panic!("expected etcd persistence")
                };
                assert_eq!(
                    etcd.tls.as_ref().unwrap().server_name.as_deref(),
                    Some("advanced.internal")
                );
            });
        Ok(())
    });
}

#[test]
fn snapshot_retention_survives_advanced_enablement_in_both_loaders() {
    [(false, false), (true, false), (false, true), (true, true)]
        .into_iter()
        .for_each(|(selection_in_env, advanced_retention)| {
            Jail::expect_with(|jail| {
                jail.create_dir("config")?;
                let settings = if selection_in_env {
                    jail.set_env(
                        "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__TYPE",
                        "Enabled",
                    );
                    "retained_update_snapshots = 7"
                } else {
                    jail.set_env("GOLEM__RETAINED_UPDATE_SNAPSHOTS", "7");
                    r#"
                        [advanced.worker_executor.filesystem_snapshots]
                        type = "Enabled"
                    "#
                };
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__CONFIG__REPOSITORY_KEY",
                    "2a".repeat(64),
                );
                if advanced_retention {
                    jail.set_env(
                        "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__CONFIG__RETAINED_UPDATE_SNAPSHOTS",
                        "11",
                    );
                }
                jail.create_file("settings.toml", settings)?;
                jail.create_file("config/worker-executor.toml", settings)?;
                let standalone =
                    golem_worker_executor::services::golem_config::make_config_loader().load()?;
                let embedded = load(
                    None,
                    &RunArgs {
                        config: Some("settings.toml".into()),
                        ..run_args_with_data_dir("/data")
                    },
                )
                .unwrap();
                [
                    &standalone.filesystem_snapshots,
                    &embedded.worker_executor.filesystem_snapshots,
                ]
                .into_iter()
                .for_each(|snapshots| {
                    let FilesystemSnapshotsConfig::Enabled(store) = snapshots else {
                        panic!("expected enabled snapshots")
                    };
                    assert_eq!(
                        store.uploads().retained_update_snapshots().get(),
                        if advanced_retention { 11 } else { 7 }
                    );
                });
                Ok(())
            });
        });
}

#[test]
fn repeated_snapshot_selection_keeps_lower_advanced_retention() {
    Jail::expect_with(|jail| {
        jail.create_dir("config")?;
        let settings = format!(
            r#"
                    retained_update_snapshots = 7
                    [advanced.worker_executor.filesystem_snapshots]
                    type = "Enabled"
                    [advanced.worker_executor.filesystem_snapshots.config]
                    retained_update_snapshots = 11
                    repository_key = "{}"
                "#,
            "2a".repeat(64)
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_SNAPSHOTS__TYPE",
            "Enabled",
        );
        jail.create_file("settings.toml", &settings)?;
        jail.create_file("config/worker-executor.toml", &settings)?;
        let standalone =
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
        let embedded = load(
            None,
            &RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            },
        )
        .unwrap();
        [
            &standalone.filesystem_snapshots,
            &embedded.worker_executor.filesystem_snapshots,
        ]
        .into_iter()
        .for_each(|snapshots| {
            let FilesystemSnapshotsConfig::Enabled(store) = snapshots else {
                panic!("expected enabled snapshots")
            };
            assert_eq!(store.uploads().retained_update_snapshots().get(), 11);
        });
        Ok(())
    });
}

#[test]
fn server_settings_reach_embedded_and_standalone_services() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_dir("config")?;
        let settings = r#"
                state_store = "postgres"
                blob_store = "s3"
                retained_update_snapshots = 7
                [outbound_sockets]
                enabled = true
                [postgres]
                host = "db.internal"
                port = 25432
                database = "installation_db"
                username = "service_user"
                password = "test-postgres-password"
                max_connections = 17
                acquire_timeout = "53s"
                [object_storage]
                endpoint_url = "https://s3.internal"
                region = "eu-west-2"
                path_style = true
                object_prefix = "installation-a/blobs"
                components_bucket = "components"
                data_bucket = "agent-data"
                [object_storage.credentials]
                access_key_id = "test-access-id"
                secret_access_key = "test-secret-key"
                provider_name = "settings-test"
                [logging]
                file_dir = "/installation/logs"
                file_name = "installation.log"
                file_truncate = false
                console = true
                dtor_friendly = true
                [logging.stdout]
                json = true
                json_flatten = false
                json_flatten_span = false
                json_source_location = true
                ansi = false
                [logging.stderr]
                enabled = true
                ansi = true
                compact = true
                without_time = true
                span_events_active = true
                [logging.file]
                enabled = true
                json_flatten = false
                json_flatten_span = false
                pretty = true
                span_events_full = true
                [otlp]
                enabled = true
                host = "collector.internal"
                port = 14318
                service_name = "installation-traces"
                [cache]
                enabled = true
                host = "cache.internal"
                port = 16379
                database = 3
                key_prefix = "installation-cache:"
                username = "cache-user"
                password = "synthetic-cache-password"
                tls = true
                pool_size = 13
                tracing = true
                [cache.retries]
                max_attempts = 7
                min_delay = "17ms"
                max_delay = "11s"
                multiplier = 3.0
                max_jitter_factor = 0.3
                [internal_grpc_tls]
                enabled = true
                cert_file = "/certs/service.pem"
                key_file = "/certs/service.key"
                ca_cert_file = "/certs/ca.pem"
                server_name = "grpc.internal"
                [filesystem_snapshots]
                enabled = true
                storage_call_deadline = "47s"
                restore_reader_threads = 11
                save_threads = 5
                max_concurrent_uploads = 9
                max_concurrent_restores = 3
                confirmation_wait = "43s"
                store_check_limit = "13s"
                capture_wait = "19s"
                max_pending_deletes_per_agent = 97
                [filesystem_snapshots.storage_retry]
                max_attempts = 6
                min_delay = "23ms"
                max_delay = "7s"
                multiplier = 3.0
                max_jitter_factor = 0.4
            "#;
        jail.set_env(
            "GOLEM__FILESYSTEM_SNAPSHOTS__REPOSITORY_KEY",
            "2a".repeat(64),
        );
        [
            "registry-service",
            "shard-manager",
            "worker-service",
            "worker-executor",
            "component-compilation-service",
        ]
        .into_iter()
        .try_for_each(|service| {
            jail.create_file(format!("config/{service}.toml"), settings)
                .map(|_| ())
        })?;
        jail.create_file("settings.toml", settings)?;
        let embedded = load(
            None,
            &RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            },
        )
        .unwrap();
        let registry = golem_registry_service::config::make_config_loader().load()?;
        let shard = golem_shard_manager::config::make_config_loader().load()?;
        let worker = golem_worker_service::config::make_worker_service_config_loader().load()?;
        let executor =
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
        let compilation =
            golem_component_compilation_service::config::make_config_loader().load()?;
        [
            (
                [
                    serde_json::to_value(&embedded.registry_service).unwrap(),
                    serde_json::to_value(&registry).unwrap(),
                ],
                &["/component_compilation/config/tls"][..],
            ),
            (
                [
                    serde_json::to_value(&embedded.shard_manager).unwrap(),
                    serde_json::to_value(&shard).unwrap(),
                ],
                &["/registry_service/tls", "/worker_executors/tls"][..],
            ),
            (
                [
                    serde_json::to_value(&embedded.worker_service).unwrap(),
                    serde_json::to_value(&worker).unwrap(),
                ],
                &[
                    "/registry_service/tls",
                    "/worker_executor/tls",
                    "/shard_manager/tls",
                ][..],
            ),
            (
                [
                    serde_json::to_value(&embedded.worker_executor).unwrap(),
                    serde_json::to_value(&executor).unwrap(),
                ],
                &[
                    "/registry_service/tls",
                    "/shard_manager/tls",
                    "/public_worker_api/tls",
                ][..],
            ),
            (
                [
                    serde_json::to_value(&embedded.component_compilation_service).unwrap(),
                    serde_json::to_value(&compilation).unwrap(),
                ],
                &["/compile_worker/tls"][..],
            ),
        ]
        .into_iter()
        .for_each(|(services, clients)| {
            services.into_iter().for_each(|service| {
                assert_eq!(
                    service["grpc"]["tls"],
                    serde_json::json!({
                        "type": "Enabled",
                        "config": {
                            "server_cert": null,
                            "server_cert_file": "/certs/service.pem",
                            "server_key": null,
                            "server_key_file": "/certs/service.key",
                            "client_ca_cert": null,
                            "client_ca_cert_file": "/certs/ca.pem",
                        },
                    })
                );
                clients.iter().for_each(|path| {
                    assert_eq!(
                        service.pointer(path).unwrap(),
                        &serde_json::json!({
                            "type": "Enabled",
                            "config": {
                                "client_cert": null,
                                "client_cert_file": "/certs/service.pem",
                                "client_key": null,
                                "client_key_file": "/certs/service.key",
                                "server_ca_cert": null,
                                "server_ca_cert_file": "/certs/ca.pem",
                                "server_domain_name": "grpc.internal",
                            },
                        }),
                        "{path}"
                    );
                });
            });
        });
        [
            (
                serde_json::to_value(&embedded.registry_service.db).unwrap(),
                "golem_registry_service",
            ),
            (
                serde_json::to_value(&registry.db).unwrap(),
                "golem_registry_service",
            ),
            (
                serde_json::to_value(&embedded.shard_manager.persistence).unwrap(),
                "golem_shard_manager",
            ),
            (
                serde_json::to_value(&shard.persistence).unwrap(),
                "golem_shard_manager",
            ),
            (
                serde_json::to_value(&embedded.worker_executor.indexed_storage).unwrap(),
                "golem_worker_executor_indexed",
            ),
            (
                serde_json::to_value(&executor.indexed_storage).unwrap(),
                "golem_worker_executor_indexed",
            ),
            (
                serde_json::to_value(&embedded.worker_executor.scheduler_storage).unwrap(),
                "golem_worker_executor_scheduler",
            ),
            (
                serde_json::to_value(&executor.scheduler_storage).unwrap(),
                "golem_worker_executor_scheduler",
            ),
        ]
        .into_iter()
        .for_each(|(store, schema)| {
            assert_eq!(store["type"], "Postgres");
            assert_eq!(store["config"]["host"], "db.internal");
            assert_eq!(store["config"]["port"], 25432);
            assert_eq!(store["config"]["database"], "installation_db");
            assert_eq!(store["config"]["username"], "service_user");
            assert_eq!(store["config"]["password"], "test-postgres-password");
            assert_eq!(store["config"]["max_connections"], 17);
            assert_eq!(store["config"]["acquire_timeout"], "53s");
            assert_eq!(store["config"]["schema"], schema);
        });
        [&embedded.worker_executor, &executor]
            .into_iter()
            .for_each(|executor| {
                let store = serde_json::to_value(&executor.key_value_storage).unwrap();
                assert_eq!(store["type"], "NamespaceRouted");
                assert_eq!(store["config"]["persistent"]["type"], "Postgres");
                assert_eq!(
                    store["config"]["persistent"]["config"]["host"],
                    "db.internal"
                );
                let postgres = &store["config"]["persistent"]["config"];
                assert_eq!(postgres["port"], 25432);
                assert_eq!(postgres["database"], "installation_db");
                assert_eq!(postgres["username"], "service_user");
                assert_eq!(postgres["password"], "test-postgres-password");
                assert_eq!(postgres["max_connections"], 17);
                assert_eq!(postgres["acquire_timeout"], "53s");
                assert_eq!(postgres["schema"], "golem_worker_executor");
                assert_eq!(store["config"]["cache"]["config"]["host"], "cache.internal");
                assert_eq!(store["config"]["cache"]["config"]["port"], 16379);
                let cache = &store["config"]["cache"]["config"];
                assert_eq!(cache["database"], 3);
                assert_eq!(cache["key_prefix"], "installation-cache:");
                assert_eq!(cache["username"], "cache-user");
                assert_eq!(cache["password"], "synthetic-cache-password");
                assert_eq!(cache["tls"], true);
                assert_eq!(cache["pool_size"], 13);
                assert_eq!(cache["tracing"], true);
                assert_eq!(cache["retries"]["max_attempts"], 7);
                assert_eq!(cache["retries"]["min_delay"], "17ms");
                assert_eq!(cache["retries"]["max_delay"], "11s");
                assert_eq!(cache["retries"]["multiplier"], 3.0);
                assert_eq!(cache["retries"]["max_jitter_factor"], 0.3);
                let FilesystemSnapshotsConfig::Enabled(snapshots) = &executor.filesystem_snapshots
                else {
                    panic!("expected enabled snapshots")
                };
                assert_eq!(snapshots.uploads().retained_update_snapshots().get(), 7);
                let snapshots = serde_json::to_value(snapshots).unwrap();
                assert_eq!(snapshots["repository_key"], "2a".repeat(64));
                assert_eq!(snapshots["storage_call_deadline"], "47s");
                assert_eq!(snapshots["restore_reader_threads"], 11);
                assert_eq!(snapshots["save_threads"], 5);
                assert_eq!(snapshots["max_concurrent_uploads"], 9);
                assert_eq!(snapshots["max_concurrent_restores"], 3);
                assert_eq!(snapshots["confirmation_wait"], "43s");
                assert_eq!(snapshots["store_check_limit"], "13s");
                assert_eq!(snapshots["capture_wait"], "19s");
                assert_eq!(snapshots["max_pending_deletes_per_agent"], 97);
                assert_eq!(snapshots["storage_retry"]["max_attempts"], 6);
                assert_eq!(snapshots["storage_retry"]["min_delay"], "23ms");
                assert_eq!(snapshots["storage_retry"]["max_delay"], "7s");
                assert_eq!(snapshots["storage_retry"]["multiplier"], 3.0);
                assert_eq!(snapshots["storage_retry"]["max_jitter_factor"], 0.4);
            });
        [&embedded.worker_service, &worker]
            .into_iter()
            .for_each(|worker| {
                let store = serde_json::to_value(&worker.gateway_session_storage).unwrap();
                assert_eq!(store["type"], "Redis");
                assert_eq!(store["config"]["host"], "cache.internal");
                assert_eq!(store["config"]["port"], 16379);
                assert_eq!(store["config"]["database"], 3);
                assert_eq!(store["config"]["key_prefix"], "installation-cache:");
                assert_eq!(store["config"]["username"], "cache-user");
                assert_eq!(store["config"]["password"], "synthetic-cache-password");
                assert_eq!(store["config"]["tls"], true);
                assert_eq!(store["config"]["pool_size"], 13);
                assert_eq!(store["config"]["tracing"], true);
                assert_eq!(store["config"]["retries"]["max_attempts"], 7);
                assert_eq!(store["config"]["retries"]["min_delay"], "17ms");
                assert_eq!(store["config"]["retries"]["max_delay"], "11s");
                assert_eq!(store["config"]["retries"]["multiplier"], 3.0);
                assert_eq!(store["config"]["retries"]["max_jitter_factor"], 0.3);
            });
        [
            &embedded.registry_service.blob_storage,
            &registry.blob_storage,
            &embedded.worker_service.blob_storage,
            &worker.blob_storage,
            &embedded.worker_executor.blob_storage,
            &executor.blob_storage,
            &embedded.component_compilation_service.blob_storage,
            &compilation.blob_storage,
        ]
        .into_iter()
        .for_each(|storage| {
            let BlobStorageConfig::S3(config) = storage else {
                panic!("expected S3")
            };
            assert_eq!(
                config.aws_endpoint_url.as_deref(),
                Some("https://s3.internal")
            );
            assert_eq!(config.region, "eu-west-2");
            assert_eq!(config.aws_path_style, Some(true));
            assert_eq!(config.object_prefix, "installation-a/blobs");
            assert!(config.namespace_prefixes);
            assert_eq!(config.components_bucket, "components");
            assert_eq!(config.custom_data_bucket, "agent-data");
            assert_eq!(config.oplog_payload_bucket, "agent-data");
            assert_eq!(config.compilation_cache_bucket, "agent-data");
            assert_eq!(config.initial_agent_files_bucket, "agent-data");
            assert_eq!(config.filesystem_snapshots_bucket, "agent-data");
            assert_eq!(config.compressed_oplog_buckets, ["agent-data"]);
            let credentials = config
                .aws_credentials
                .as_ref()
                .expect("explicit credentials");
            assert_eq!(credentials.access_key_id, "test-access-id");
            assert_eq!(credentials.secret_access_key, "test-secret-key");
            assert_eq!(credentials.provider_name, "settings-test");
        });
        assert!(executor.agent_sockets.enabled);
        assert!(embedded.worker_executor.agent_sockets.enabled);
        [
            &embedded.logging,
            &registry.tracing,
            &shard.tracing,
            &worker.tracing,
            &executor.tracing,
            &compilation.tracing,
        ]
        .into_iter()
        .for_each(|config| {
            assert!(config.stdout.json);
            assert!(!config.stdout.json_flatten);
            assert!(!config.stdout.json_flatten_span);
            assert!(config.stdout.json_source_location);
            assert!(!config.stdout.ansi);
            assert!(config.stderr.enabled);
            assert!(config.stderr.ansi);
            assert!(config.stderr.compact);
            assert!(config.stderr.without_time);
            assert!(config.stderr.span_events_active);
            assert!(!config.stderr.span_events_full);
            assert!(config.file.enabled);
            assert!(!config.file.json_flatten);
            assert!(!config.file.json_flatten_span);
            assert!(config.file.pretty);
            assert!(!config.file.ansi);
            assert!(!config.file.span_events_active);
            assert!(config.file.span_events_full);
            assert_eq!(config.file_dir.as_deref(), Some("/installation/logs"));
            assert_eq!(config.file_name.as_deref(), Some("installation.log"));
            assert!(!config.file_truncate);
            assert!(config.console);
            assert!(config.dtor_friendly);
            assert!(config.otlp.enabled);
            assert_eq!(config.otlp.service_name, "installation-traces");
            assert_eq!(config.otlp.host, "collector.internal");
            assert_eq!(config.otlp.port, 14318);
        });
        jail.set_env("GOLEM__CACHE__ENABLED", "false");
        jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", "false");
        jail.set_env("GOLEM__OUTBOUND_SOCKETS__ENABLED", "false");
        let uncached = load(
            None,
            &RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            },
        )
        .unwrap();
        let executor =
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
        [&uncached.worker_executor, &executor]
            .into_iter()
            .for_each(|executor| {
                let store = serde_json::to_value(&executor.key_value_storage).unwrap();
                assert_eq!(store["type"], "Postgres");
                let postgres = &store["config"];
                assert_eq!(postgres["host"], "db.internal");
                assert_eq!(postgres["port"], 25432);
                assert_eq!(postgres["database"], "installation_db");
                assert_eq!(postgres["username"], "service_user");
                assert_eq!(postgres["password"], "test-postgres-password");
                assert_eq!(postgres["max_connections"], 17);
                assert_eq!(postgres["acquire_timeout"], "53s");
                assert_eq!(postgres["schema"], "golem_worker_executor");
                assert!(matches!(
                    executor.filesystem_snapshots,
                    FilesystemSnapshotsConfig::Disabled(_)
                ));
                assert!(!executor.agent_sockets.enabled);
            });
        let worker = golem_worker_service::config::make_worker_service_config_loader().load()?;
        [&uncached.worker_service, &worker]
            .into_iter()
            .for_each(|worker| {
                let SessionStoreConfig::Postgres(postgres) = &worker.gateway_session_storage else {
                    panic!("expected PostgreSQL sessions")
                };
                let postgres = serde_json::to_value(postgres).unwrap();
                assert_eq!(postgres["host"], "db.internal");
                assert_eq!(postgres["port"], 25432);
                assert_eq!(postgres["database"], "installation_db");
                assert_eq!(postgres["username"], "service_user");
                assert_eq!(postgres["password"], "test-postgres-password");
                assert_eq!(postgres["max_connections"], 17);
                assert_eq!(postgres["acquire_timeout"], "53s");
                assert_eq!(postgres["schema"], "golem_worker_service_session_store");
            });
        jail.set_env("GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__TYPE", "Etcd");
        jail.set_env(
            "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__CONFIG__ENDPOINTS",
            r#"["https://etcd.internal:2379"]"#,
        );
        jail.set_env("GOLEM__ETCD__TLS__CA_CERT_FILE", "/certs/etcd-ca.pem");
        jail.set_env("GOLEM__ETCD__TLS__CLIENT_CERT_FILE", "/certs/etcd.pem");
        jail.set_env("GOLEM__ETCD__TLS__CLIENT_KEY_FILE", "/certs/etcd.key");
        let shard = golem_shard_manager::config::make_config_loader().load()?;
        let PersistenceConfig::Etcd(etcd) = shard.persistence else {
            panic!("expected etcd persistence")
        };
        let tls = etcd.tls.unwrap();
        assert_eq!(
            tls.ca_cert_file.as_deref(),
            Some(Path::new("/certs/etcd-ca.pem"))
        );
        assert_eq!(
            tls.client_cert_file.as_deref(),
            Some(Path::new("/certs/etcd.pem"))
        );
        assert_eq!(
            tls.client_key_file.as_deref(),
            Some(Path::new("/certs/etcd.key"))
        );
        Ok(())
    });
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_dir("config")?;
        let settings = r#"
                state_store = "sqlite"
                blob_store = "data_disk"
                data_dir = "/srv/installation-state"
                [data_disk]
                blob_root = "/mnt/installation-blobs"
                [cache]
                enabled = false
                [filesystem_snapshots]
                enabled = false
            "#;
        [
            "registry-service",
            "shard-manager",
            "worker-service",
            "worker-executor",
            "component-compilation-service",
        ]
        .into_iter()
        .try_for_each(|service| {
            jail.create_file(format!("config/{service}.toml"), settings)
                .map(|_| ())
        })?;
        jail.create_file("settings.toml", settings)?;
        let args = RunArgs {
            config: Some("settings.toml".into()),
            ..RunArgs::default()
        };
        let embedded = load(None, &args).unwrap();
        let registry = golem_registry_service::config::make_config_loader().load()?;
        let shard = golem_shard_manager::config::make_config_loader().load()?;
        let worker = golem_worker_service::config::make_worker_service_config_loader().load()?;
        let executor =
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
        let compilation =
            golem_component_compilation_service::config::make_config_loader().load()?;
        [
            (
                [
                    serde_json::to_value(&embedded.registry_service.db).unwrap(),
                    serde_json::to_value(&registry.db).unwrap(),
                ],
                "/srv/installation-state/registry.db",
            ),
            (
                [
                    serde_json::to_value(&embedded.shard_manager.persistence).unwrap(),
                    serde_json::to_value(&shard.persistence).unwrap(),
                ],
                "/srv/installation-state/shard_manager.db",
            ),
            (
                [
                    serde_json::to_value(&embedded.worker_service.gateway_session_storage).unwrap(),
                    serde_json::to_value(&worker.gateway_session_storage).unwrap(),
                ],
                "/srv/installation-state/gateway-sessions.db",
            ),
            (
                [
                    serde_json::to_value(&embedded.worker_executor.scheduler_storage).unwrap(),
                    serde_json::to_value(&executor.scheduler_storage).unwrap(),
                ],
                "/srv/installation-state/scheduler.db",
            ),
        ]
        .into_iter()
        .for_each(|(stores, database)| {
            stores.into_iter().for_each(|store| {
                assert_eq!(store["type"], "Sqlite");
                assert_eq!(store["config"]["database"], database);
            });
        });
        [&embedded.worker_executor, &executor]
            .into_iter()
            .for_each(|executor| {
                let kv = serde_json::to_value(&executor.key_value_storage).unwrap();
                assert_eq!(kv["type"], "MultiSqlite");
                assert_eq!(kv["config"]["root_dir"], "/srv/installation-state/kv-store");
                assert!(matches!(
                    executor.indexed_storage,
                    IndexedStorageConfig::KVStoreMultiSqlite(_)
                ));
            });
        [
            &embedded.registry_service.blob_storage,
            &registry.blob_storage,
            &embedded.worker_service.blob_storage,
            &worker.blob_storage,
            &embedded.worker_executor.blob_storage,
            &executor.blob_storage,
            &embedded.component_compilation_service.blob_storage,
            &compilation.blob_storage,
        ]
        .into_iter()
        .for_each(|storage| {
            assert_eq!(blob_root(storage), Path::new("/mnt/installation-blobs"));
        });
        jail.set_env("GOLEM__CACHE__ENABLED", "true");
        let cached = load(None, &args).unwrap();
        let executor =
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
        [&cached.worker_executor, &executor]
            .into_iter()
            .for_each(|executor| {
                let kv = serde_json::to_value(&executor.key_value_storage).unwrap();
                assert_eq!(kv["type"], "NamespaceRouted");
                let persistent = &kv["config"]["persistent"];
                assert_eq!(persistent["type"], "MultiSqlite");
                assert_eq!(
                    persistent["config"]["root_dir"],
                    "/srv/installation-state/kv-store"
                );
                let indexed = serde_json::to_value(&executor.indexed_storage).unwrap();
                assert_eq!(indexed["type"], "MultiSqlite");
                assert_eq!(
                    indexed["config"]["root_dir"],
                    "/srv/installation-state/kv-store"
                );
            });
        Ok(())
    });
}

#[test]
fn advanced_sections_fail_only_their_own_readers() {
    [
        "advanced.registry_service",
        "advanced.shard_manager",
        "advanced.worker_service",
        "advanced.worker_executor",
        "advanced.component_compilation_service",
        "advanced.server",
        "advanced",
        "cache",
    ]
    .into_iter()
    .for_each(|path| {
        [false, true].into_iter().for_each(|environment| {
            Jail::expect_with(|jail| {
                jail.clear_env();
                jail.create_dir("config")?;
                let settings = if environment {
                    let variable = match path.strip_prefix("advanced.") {
                        Some(service) => format!("GOLEM_ADVANCED__{}", service.to_uppercase()),
                        None => format!("GOLEM__{}", path.to_uppercase()),
                    };
                    jail.set_env(variable, "false");
                    String::new()
                } else {
                    format!("{path} = false")
                };
                [
                    "registry-service",
                    "shard-manager",
                    "worker-service",
                    "worker-executor",
                    "component-compilation-service",
                ]
                .into_iter()
                .try_for_each(|service| {
                    jail.create_file(format!("config/{service}.toml"), &settings)
                        .map(|_| ())
                })?;
                jail.create_file("settings.toml", &settings)?;
                [
                    (
                        "registry_service",
                        golem_registry_service::config::make_config_loader()
                            .load()
                            .map(|_| ()),
                    ),
                    (
                        "shard_manager",
                        golem_shard_manager::config::make_config_loader()
                            .load()
                            .map(|_| ()),
                    ),
                    (
                        "worker_service",
                        golem_worker_service::config::make_worker_service_config_loader()
                            .load()
                            .map(|_| ()),
                    ),
                    (
                        "worker_executor",
                        golem_worker_executor::services::golem_config::make_config_loader()
                            .load()
                            .map(|_| ()),
                    ),
                    (
                        "component_compilation_service",
                        golem_component_compilation_service::config::make_config_loader()
                            .load()
                            .map(|_| ()),
                    ),
                ]
                .into_iter()
                .for_each(|(reader, result)| {
                    let owns = path == "advanced"
                        || path == "cache"
                        || path == format!("advanced.{reader}");
                    assert_eq!(
                        result.is_err(),
                        owns,
                        "{reader}: {path}, environment={environment}: {result:?}"
                    );
                    if let Err(error) = result {
                        let message = error.to_string();
                        assert!(message.contains(path), "{message}");
                        assert!(
                            message.contains(if environment { "environment" } else { ".toml" }),
                            "{message}"
                        );
                    }
                });
                let result = load(
                    None,
                    &RunArgs {
                        config: Some("settings.toml".into()),
                        ..run_args_with_data_dir("/data")
                    },
                );
                let error = result.expect_err("embedded reader owns every section");
                assert!(format!("{error:#}").contains(path), "{error:#}");
                Ok(())
            });
        });
    });
}

#[test]
fn advanced_executor_leaf_does_not_change_other_services() {
    [false, true].into_iter().for_each(|environment| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_dir("config")?;
            let settings = if environment {
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__RUNTIME_METRICS_SAMPLING_INTERVAL",
                    "37s",
                );
                ""
            } else {
                "advanced.worker_executor.runtime_metrics_sampling_interval = '37s'"
            };
            [
                "registry-service",
                "shard-manager",
                "worker-service",
                "worker-executor",
                "component-compilation-service",
            ]
            .into_iter()
            .try_for_each(|service| {
                jail.create_file(format!("config/{service}.toml"), settings)
                    .map(|_| ())
            })?;
            jail.create_file("settings.toml", settings)?;
            let embedded = load(
                None,
                &RunArgs {
                    config: Some("settings.toml".into()),
                    ..run_args_with_data_dir("/data")
                },
            )
            .unwrap();
            [
                golem_registry_service::config::make_config_loader()
                    .load()?
                    .runtime_metrics_sampling_interval,
                golem_shard_manager::config::make_config_loader()
                    .load()?
                    .runtime_metrics_sampling_interval,
                golem_worker_service::config::make_worker_service_config_loader()
                    .load()?
                    .runtime_metrics_sampling_interval,
                golem_component_compilation_service::config::make_config_loader()
                    .load()?
                    .runtime_metrics_sampling_interval,
                embedded.registry_service.runtime_metrics_sampling_interval,
                embedded.shard_manager.runtime_metrics_sampling_interval,
                embedded.worker_service.runtime_metrics_sampling_interval,
                embedded
                    .component_compilation_service
                    .runtime_metrics_sampling_interval,
            ]
            .into_iter()
            .for_each(|interval| assert_eq!(interval, std::time::Duration::from_secs(5)));
            [
                golem_worker_executor::services::golem_config::make_config_loader()
                    .load()?
                    .runtime_metrics_sampling_interval,
                embedded.worker_executor.runtime_metrics_sampling_interval,
            ]
            .into_iter()
            .for_each(|interval| assert_eq!(interval, std::time::Duration::from_secs(37)));
            Ok(())
        });
    });
}

#[test]
fn litestream_checks_every_effective_postgres_state_store_without_writes() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let data = jail.directory().join("uncreated-state");
        let pg = GolemServerConfig::postgres_s3_example(&data).unwrap();
        let pg = serde_json::to_value(toml::Value::try_from(pg).unwrap()).unwrap();
        let mut cases: Vec<(&str, serde_json::Value)> = [
            "registry_service.db",
            "shard_manager.persistence",
            "worker_service.gateway_session_storage",
            "worker_executor.key_value_storage",
            "worker_executor.indexed_storage",
            "worker_executor.scheduler_storage",
        ]
        .into_iter()
        .map(|path| {
            (
                path,
                pg.pointer(&format!("/{}", path.replace('.', "/")))
                    .unwrap()
                    .clone(),
            )
        })
        .collect();
        let gateway = &pg["worker_service"]["gateway_session_storage"];
        cases.push(("worker_service.mcp.session_storage", gateway.clone()));
        ["cache", "persistent"].into_iter().for_each(|inner| {
            let other = if inner == "cache" {
                "persistent"
            } else {
                "cache"
            };
            cases.push((
                "worker_executor.key_value_storage",
                serde_json::json!({
                    "type": "NamespaceRouted", "config": {
                        (inner): pg["worker_executor"]["key_value_storage"].clone(),
                        (other): {"type": "InMemory", "config": {}}
                    }
                }),
            ));
        });
        cases.into_iter().for_each(|(path, store)| {
            let advanced = path
                .rsplit('.')
                .fold(store, |value, key| serde_json::json!({(key): value}));
            let settings: toml::Value = Figment::from(Serialized::defaults(serde_json::json!({
                "state_store": "sqlite", "cache": {"enabled": false},
                "filesystem_snapshots": {"enabled": true}, "advanced": advanced,
            })))
            .extract()
            .unwrap();
            jail.create_file("litestream.toml", &toml::to_string(&settings).unwrap())
                .unwrap();
            let args = RunArgs {
                config: Some("litestream.toml".into()),
                data_dir: Some(data.clone()),
                clean: true,
                ..Default::default()
            };
            let loaded =
                load_server_config(None, &args, SnapshotRepositoryKeySource::ValidationOnly)
                    .unwrap();
            let loaded = serde_json::to_value(loaded).unwrap();
            let selected = loaded
                .pointer(&format!("/{}", path.replace('.', "/")))
                .unwrap();
            assert!(
                selected.to_string().contains("Postgres"),
                "{path}: {selected}"
            );
            let error = check_litestream_config(&args).unwrap_err();
            assert!(
                error.to_string().contains("GOLEM_LITESTREAM=true"),
                "{error:#}"
            );
            assert!(error.to_string().contains("PostgreSQL"), "{error:#}");
            assert!(error.to_string().contains(path), "{error:#}");
            assert!(!data.exists());
        });
        Ok(())
    });
}

#[test]
fn litestream_uses_the_normal_configuration_precedence() {
    [
        ("state_store='postgres'", Some("sqlite"), true),
        ("state_store='sqlite'", Some("postgres"), false),
        (
            "state_store='sqlite'\n[postgres]\nhost='unused.invalid'",
            None,
            true,
        ),
        (
            "state_store='sqlite'\nblob_store='s3'\n[cache]\nenabled=true",
            None,
            true,
        ),
        ("state_store='invalid'", None, false),
    ]
    .into_iter()
    .for_each(|(settings, state, allowed)| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("litestream.toml", settings)?;
            if let Some(state) = state {
                jail.set_env("GOLEM__STATE_STORE", state);
            }
            let data = jail.directory().join("absent-state");
            let args = RunArgs {
                config: Some("litestream.toml".into()),
                data_dir: Some(data.clone()),
                ..Default::default()
            };
            assert_eq!(
                check_litestream_config(&args).is_ok(),
                allowed,
                "{settings}, {state:?}"
            );
            assert!(!data.exists());
            Ok(())
        });
    });
    [false, true].into_iter().for_each(|replace_with_sqlite| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let data = jail.directory().join("absent-state");
            let pg = GolemServerConfig::postgres_s3_example(&data).unwrap();
            let indexed_storage =
                toml::Value::try_from(pg.worker_executor.indexed_storage).unwrap();
            let settings: toml::Value = Figment::from(Serialized::defaults(serde_json::json!({
                "state_store": "sqlite",
                "advanced": {"worker_executor": {"indexed_storage": indexed_storage}}
            })))
            .extract()
            .unwrap();
            jail.create_file("litestream.toml", &toml::to_string(&settings).unwrap())?;
            jail.set_env("GOLEM__STATE_STORE", "sqlite");
            if replace_with_sqlite {
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__TYPE",
                    "Sqlite",
                );
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__DATABASE",
                    "uncreated/indexed.db",
                );
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__MAX_CONNECTIONS",
                    "2",
                );
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__FOREIGN_KEYS",
                    "false",
                );
            }
            let args = RunArgs {
                config: Some("litestream.toml".into()),
                data_dir: Some(data.clone()),
                ..Default::default()
            };
            let loaded =
                load_server_config(None, &args, SnapshotRepositoryKeySource::ValidationOnly)
                    .unwrap();
            assert_eq!(
                matches!(
                    loaded.worker_executor.indexed_storage,
                    IndexedStorageConfig::Sqlite(_)
                ),
                replace_with_sqlite
            );
            assert_eq!(check_litestream_config(&args).is_ok(), replace_with_sqlite);
            assert!(!data.exists());
            assert!(!Path::new("uncreated").exists());
            Ok(())
        });
    });
}

#[test]
fn server_settings_examples_load_in_every_binary() {
    const SECRETS: &[&str] = &[
        "password",
        "access_key_id",
        "secret_access_key",
        "token",
        "hmac_key",
        "repository_key",
        "key",
        "private_key",
        "client_secret",
        "server_key",
        "client_key",
    ];
    fn url_has_secret(value: &str) -> bool {
        let Some((_, address)) = value.split_once("://") else {
            return false;
        };
        address
            .split(['/', '?', '#'])
            .next()
            .and_then(|authority| authority.rsplit_once('@'))
            .is_some_and(|(userinfo, _)| !userinfo.is_empty())
            || address.split_once('?').is_some_and(|(_, query)| {
                query.split('#').next().unwrap().split('&').any(|field| {
                    field.split_once('=').is_some_and(|(key, value)| {
                        let key = key.to_ascii_lowercase();
                        !value.is_empty()
                            && (SECRETS.contains(&key.as_str())
                                || [
                                    "access_token",
                                    "api_key",
                                    "x-amz-credential",
                                    "x-amz-signature",
                                ]
                                .contains(&key.as_str()))
                    })
                })
            })
    }
    fn secret_fields(value: &serde_json::Value, path: &str, found: Vec<String>) -> Vec<String> {
        match value {
            serde_json::Value::Object(fields) => {
                fields.iter().fold(found, |mut found, (key, value)| {
                    let path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    if SECRETS.contains(&key.as_str()) && !value.is_null() && value != "" {
                        found.push(path.clone());
                    }
                    secret_fields(value, &path, found)
                })
            }
            serde_json::Value::Array(items) => items
                .iter()
                .enumerate()
                .fold(found, |found, (index, value)| {
                    secret_fields(value, &format!("{path}[{index}]"), found)
                }),
            serde_json::Value::String(value) if url_has_secret(value) => {
                let mut found = found;
                found.push(path.to_string());
                found
            }
            _ => found,
        }
    }
    let contains_raw_secret = |text: &str| {
        text.contains("-----BEGIN PRIVATE KEY-----")
            || text.lines().any(|line| {
                line.split('#').any(|part| {
                    part.split_once('=').is_some_and(|(key, value)| {
                        let key = key
                            .split_whitespace()
                            .next_back()
                            .unwrap_or("")
                            .rsplit('.')
                            .next()
                            .unwrap()
                            .rsplit("__")
                            .next()
                            .unwrap()
                            .trim_matches(['\'', '"'])
                            .to_ascii_lowercase();
                        SECRETS.contains(&key.as_str())
                            && !value.trim().trim_matches(['\'', '"']).is_empty()
                    })
                })
            })
    };
    ["server_key", "client_key", "password", "secret_access_key"]
        .into_iter()
        .for_each(|key| {
            ["", "# ", "host = 'postgres.internal' # "]
                .into_iter()
                .for_each(|prefix| {
                    assert!(contains_raw_secret(&format!(
                        "{prefix}{key} = 'synthetic-secret'"
                    )));
                    assert!(contains_raw_secret(&format!(
                        "{prefix}\"{key}\" = 'synthetic-secret'"
                    )));
                });
        });
    ["# ", "host = 'postgres.internal' # "]
        .into_iter()
        .for_each(|prefix| {
            assert!(contains_raw_secret(&format!(
                "{prefix}GOLEM__POSTGRES__PASSWORD=synthetic-secret"
            )));
        });
    [
        "# Supply the password through the environment.",
        "host = 'postgres.internal' # Supply credentials separately.",
        "server_key_file = '/certs/key.pem'\nclient_key_file = '/certs/client.pem'",
        "host = 'postgres.internal' # GOLEM__POSTGRES__PASSWORD_FILE=/run/secrets/password",
    ]
    .into_iter()
    .for_each(|control| assert!(!contains_raw_secret(control)));
    [
        (
            "https://mirror.example/python.wasm?token=synthetic-bearer",
            true,
        ),
        (
            "https://synthetic-user:synthetic-password@mirror.example/python.wasm",
            true,
        ),
        (
            "https://mirror.example/python.wasm?access_token=synthetic-bearer",
            true,
        ),
        (
            "https://mirror.example/python.wasm?X-Amz-Signature=synthetic-signature",
            true,
        ),
        ("https://mirror.example/python.wasm?version=2", false),
        ("https://s3.internal:9443", false),
        ("file:///certs/private-key.pem", false),
    ]
    .into_iter()
    .for_each(|(url, expected)| {
        assert_eq!(
            !secret_fields(&serde_json::json!({"url": url}), "", Vec::new()).is_empty(),
            expected
        );
    });

    [
            (include_str!("../../config/self-host-single-node.toml"), "sqlite"),
            (include_str!("../../config/self-host-multi-node.toml"), "postgres"),
        ]
        .into_iter()
        .for_each(|(settings, state)| {
            let example: serde_json::Value = Figment::from(Toml::string(settings)).extract().unwrap();
            assert_eq!(secret_fields(&example, "", Vec::new()), Vec::<String>::new());
            assert!(!contains_raw_secret(settings));
            assert_eq!(example["state_store"], state);
            assert_eq!(example["blob_store"], "s3");
            [
                ("postgres.password", serde_json::json!({"postgres": {"password": "test-inserted-password"}})),
                ("advanced.worker_service.invocation_session_tokens.active_key.key", serde_json::json!({
                    "advanced": {"worker_service": {"invocation_session_tokens": {"active_key": {"key": "test-inserted-key"}}}}
                })),
                ("advanced.registry_service.grpc.tls.config.server_key", serde_json::json!({
                    "advanced": {"registry_service": {"grpc": {"tls": {"config": {"server_key": "synthetic-server-key"}}}}}
                })),
                ("advanced.worker_service.registry_service.tls.config.client_key", serde_json::json!({
                    "advanced": {"worker_service": {"registry_service": {"tls": {"config": {"client_key": "synthetic-client-key"}}}}}
                })),
                ("advanced.registry_service.builtin_artifacts.source_overrides.python.url", serde_json::json!({
                    "advanced": {"registry_service": {"builtin_artifacts": {"source_overrides": {"python": {
                        "url": "https://private-mirror.example/python.wasm?token=synthetic-download-bearer"
                    }}}}}
                })),
                ("advanced.registry_service.builtin_artifacts.source_overrides.python.url", serde_json::json!({
                    "advanced": {"registry_service": {"builtin_artifacts": {"source_overrides": {"python": {
                        "url": "https://synthetic-user:synthetic-password@private-mirror.example/python.wasm"
                    }}}}}
                })),
            ].into_iter().for_each(|(path, secret)| {
                let with_secret: serde_json::Value = Figment::from(Serialized::defaults(&example))
                    .merge(Serialized::defaults(secret)).extract().unwrap();
                assert_eq!(secret_fields(&with_secret, "", Vec::new()), [path]);
            });
            Jail::expect_with(|jail| {
                jail.clear_env();
                jail.create_dir("config")?;
                [
                    "registry-service",
                    "shard-manager",
                    "worker-service",
                    "worker-executor",
                    "component-compilation-service",
                ]
                .into_iter()
                .try_for_each(|service| {
                    jail.create_file(format!("config/{service}.toml"), settings)
                        .map(|_| ())
                })?;
                jail.create_file("settings.toml", settings)?;
                let embedded = load(
                    None,
                    &RunArgs {
                        config: Some("settings.toml".into()),
                        ..RunArgs::default()
                    },
                )
                .unwrap();
                let registry = golem_registry_service::config::make_config_loader().load()?;
                let shard = golem_shard_manager::config::make_config_loader().load()?;
                let worker = golem_worker_service::config::make_worker_service_config_loader().load()?;
                let executor = golem_worker_executor::services::golem_config::make_config_loader().load()?;
                let compilation = golem_component_compilation_service::config::make_config_loader().load()?;
                [
                    &embedded.registry_service.blob_storage, &registry.blob_storage,
                    &embedded.worker_service.blob_storage, &worker.blob_storage,
                    &embedded.worker_executor.blob_storage, &executor.blob_storage,
                    &embedded.component_compilation_service.blob_storage, &compilation.blob_storage,
                ].into_iter().for_each(|storage| {
                    let BlobStorageConfig::S3(config) = storage else { panic!("expected S3 in {state} example") };
                    assert_eq!(config.region, "us-east-1");
                    assert_eq!(config.components_bucket, "golem-components");
                    assert_eq!(config.initial_agent_files_bucket, "golem-data");
                    assert_eq!(config.oplog_payload_bucket, "golem-data");
                    assert_eq!(config.custom_data_bucket, "golem-data");
                    assert_eq!(config.compilation_cache_bucket, "golem-data");
                    assert_eq!(config.filesystem_snapshots_bucket, "golem-data");
                    assert_eq!(config.compressed_oplog_buckets, ["golem-data"]);
                    assert!(config.namespace_prefixes);
                    assert!(config.aws_credentials.is_none());
                });
                let expected = if state == "postgres" { "Postgres" } else { "Sqlite" };
                [
                    serde_json::to_value(&embedded.registry_service.db).unwrap(), serde_json::to_value(&registry.db).unwrap(),
                    serde_json::to_value(&embedded.shard_manager.persistence).unwrap(), serde_json::to_value(&shard.persistence).unwrap(),
                    serde_json::to_value(&embedded.worker_service.gateway_session_storage).unwrap(), serde_json::to_value(&worker.gateway_session_storage).unwrap(),
                    serde_json::to_value(embedded.worker_service.mcp.session.effective_session_storage(&embedded.worker_service.gateway_session_storage)).unwrap(),
                    serde_json::to_value(worker.mcp.session.effective_session_storage(&worker.gateway_session_storage)).unwrap(),
                    serde_json::to_value(&embedded.worker_executor.scheduler_storage).unwrap(), serde_json::to_value(&executor.scheduler_storage).unwrap(),
                ].into_iter().for_each(|store| {
                    assert_eq!(store["type"], expected);
                    if state == "postgres" {
                        assert_eq!(store["config"]["host"], "postgres.internal");
                        assert_eq!(store["config"]["port"], 5432);
                        assert_eq!(store["config"]["database"], "golem");
                        assert_eq!(store["config"]["username"], "golem");
                    } else {
                        assert!(store["config"]["database"].as_str().unwrap().starts_with("/var/lib/golem/"));
                    }
                });
                [&embedded.worker_executor, &executor].into_iter().for_each(|executor| {
                    let kv = serde_json::to_value(&executor.key_value_storage).unwrap();
                    let indexed = serde_json::to_value(&executor.indexed_storage).unwrap();
                    if state == "postgres" {
                        assert_eq!(kv["type"], "Postgres");
                        assert_eq!(indexed["type"], "Postgres");
                        [&kv, &indexed].into_iter().for_each(|store| assert_eq!(store["config"]["host"], "postgres.internal"));
                    } else {
                        assert_eq!(kv["type"], "MultiSqlite");
                        assert_eq!(kv["config"]["root_dir"], "/var/lib/golem/kv-store");
                        assert_eq!(indexed["type"], "KVStoreMultiSqlite");
                    }
                });
                Ok(())
            });
        });
}

#[test]
fn removed_server_settings_are_rejected_by_both_loaders() {
    [
        "number_of_shards",
        "retained_periodic_snapshots",
        "filesystem_snapshots.retained_periodic_snapshots",
    ]
    .into_iter()
    .flat_map(|key| [false, true].map(|from_env| (key, from_env)))
    .for_each(|(key, from_env)| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_dir("config")?;
            let base = format!(
                "[filesystem_snapshots]\nenabled=true\nrepository_key='{}'\n",
                "2a".repeat(64)
            );
            let files = [
                "config/worker-executor.toml",
                "config/shard-manager.toml",
                "settings.toml",
            ];
            files
                .into_iter()
                .try_for_each(|file| jail.create_file(file, &base).map(|_| ()))?;
            let args = RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            };
            let control = load(None, &args).unwrap();
            assert!(matches!(
                control.worker_executor.filesystem_snapshots,
                FilesystemSnapshotsConfig::Enabled(_)
            ));
            golem_worker_executor::services::golem_config::make_config_loader().load()?;
            golem_shard_manager::config::make_config_loader().load()?;
            if from_env {
                jail.set_env(
                    format!("GOLEM__{}", key.replace('.', "__").to_uppercase()),
                    "17",
                );
            } else {
                let settings = if key.starts_with("filesystem_snapshots.") {
                    format!("{base}retained_periodic_snapshots=17\n")
                } else {
                    format!("{key}=17\n{base}")
                };
                files
                    .into_iter()
                    .try_for_each(|file| jail.create_file(file, &settings).map(|_| ()))?;
            }
            [
                golem_worker_executor::services::golem_config::make_config_loader()
                    .load()
                    .map(|_| ())
                    .map_err(anyhow::Error::from),
                golem_shard_manager::config::make_config_loader()
                    .load()
                    .map(|_| ())
                    .map_err(anyhow::Error::from),
                load(None, &args).map(|_| ()),
            ]
            .into_iter()
            .for_each(|result| {
                let error = result.expect_err("removed surface setting must not load");
                assert!(
                    format!("{error:#}").to_ascii_lowercase().contains(key),
                    "{error:#}"
                );
            });
            Ok(())
        });
    });
}

#[test]
fn server_settings_choices_and_switches_name_the_accepted_values() {
    [
        ("state_store", "\"Postgres\"", "sqlite or postgres"),
        ("blob_store", "\"S3\"", "data_disk or s3"),
        ("cache.enabled", "\"Redis\"", "true or false"),
        (
            "filesystem_snapshots.enabled",
            "\"Enabled\"",
            "true or false",
        ),
        ("internal_grpc_tls.enabled", "\"on\"", "true or false"),
        ("otlp.enabled", "1", "true or false"),
        ("outbound_sockets.enabled", "\"off\"", "true or false"),
        ("data_dir", "false", "string"),
        ("internal_grpc_tls", "true", "table"),
        ("postgres", "5432", "table"),
        ("data_disk", "\"/state\"", "table"),
        ("object_storage", "\"s3\"", "table"),
        ("object_storage.credentials", "\"credentials\"", "table"),
        ("cache", "true", "table"),
        ("cache.retries", "false", "table"),
        ("filesystem_snapshots", "false", "table"),
        ("filesystem_snapshots.storage_retry", "false", "table"),
        ("outbound_sockets", "false", "table"),
        ("logging", "true", "table"),
        ("logging.stdout", "true", "table"),
        ("logging.stderr", "false", "table"),
        ("logging.file", "\"/logs\"", "table"),
        ("logging.otlp", "false", "table"),
        ("otlp", "false", "table"),
        ("etcd", "\"etcd.internal\"", "table"),
        ("etcd.tls", "true", "table"),
    ]
    .into_iter()
    .for_each(|(key, invalid, accepted)| {
        [false, true].into_iter().for_each(|from_env| {
            Jail::expect_with(|jail| {
                jail.create_dir("config")?;
                if from_env {
                    jail.set_env(
                        format!("GOLEM__{}", key.replace('.', "__").to_uppercase()),
                        invalid,
                    );
                    jail.create_file("config/worker-executor.toml", "")?;
                    jail.create_file("settings.toml", "")?;
                } else {
                    let settings = format!("{key} = {invalid}");
                    jail.create_file("config/worker-executor.toml", &settings)?;
                    jail.create_file("settings.toml", &settings)?;
                }
                let standalone =
                    golem_worker_executor::services::golem_config::make_config_loader()
                        .load()
                        .unwrap_err()
                        .to_string();
                let embedded = jail_error(load(
                    None,
                    &RunArgs {
                        config: Some("settings.toml".into()),
                        ..if key == "data_dir" {
                            RunArgs::default()
                        } else {
                            run_args_with_data_dir("/data")
                        }
                    },
                ));
                [standalone, embedded].into_iter().for_each(|error| {
                    assert!(error.to_ascii_lowercase().contains(key), "{error}");
                    assert!(error.contains(accepted), "{error}");
                });
                Ok(())
            });
        });
    });
}

#[test]
fn advanced_server_data_dir_is_rejected_by_projection_and_loaders() {
    [false, true].into_iter().for_each(|from_env| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let settings = if from_env {
                jail.set_env("GOLEM_ADVANCED__SERVER__DATA_DIR", "split-root");
                "data_dir = 'supported-root'"
            } else {
                "data_dir = 'supported-root'\nadvanced.server.data_dir = 'split-root'"
            };
            jail.create_file("settings.toml", settings)?;
            let input =
                Figment::from(Toml::file_exact("settings.toml")).merge(env_config_provider());
            let direct = server_settings::all_in_one(&input).unwrap_err().to_string();
            let loaded = jail_error(load(
                None,
                &RunArgs {
                    config: Some("settings.toml".into()),
                    data_dir: Some("flag-root".into()),
                    ..RunArgs::default()
                },
            ));
            [direct, loaded].into_iter().for_each(|diagnostic| {
                let diagnostic = diagnostic.to_ascii_lowercase();
                assert!(
                    diagnostic.contains("advanced.server.data_dir"),
                    "{diagnostic}"
                );
                assert!(diagnostic.contains("supported data_dir"), "{diagnostic}");
                assert!(
                    diagnostic.contains(if from_env {
                        "environment variable"
                    } else {
                        "settings.toml"
                    }),
                    "{diagnostic}"
                );
            });
            ["supported-root", "split-root", "flag-root"]
                .into_iter()
                .for_each(|path| assert!(!Path::new(path).exists()));
            Ok(())
        });
    });
}

#[test]
fn supported_data_dir_precedence_keeps_storage_roots_together() {
    [
        (false, false, "file-root"),
        (true, false, "env-root"),
        (true, true, "flag-root"),
    ]
    .into_iter()
    .for_each(|(from_env, from_flag, expected)| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("settings.toml", "data_dir = 'file-root'")?;
            if from_env {
                jail.set_env("GOLEM__DATA_DIR", "env-root");
            }
            let loaded = load(
                None,
                &RunArgs {
                    config: Some("settings.toml".into()),
                    data_dir: from_flag.then(|| "flag-root".into()),
                    ..RunArgs::default()
                },
            )
            .unwrap();
            assert_eq!(loaded.data_dir, Path::new(expected));
            let DbConfig::Sqlite(db) = &loaded.registry_service.db else {
                panic!("expected SQLite registry")
            };
            assert_eq!(sqlite_path(db), Path::new(expected).join("registry.db"));
            assert_eq!(
                blob_root(&loaded.registry_service.blob_storage),
                Path::new(expected).join("blobs")
            );
            assert!(!Path::new(expected).exists());
            Ok(())
        });
    });
}

#[test]
fn filesystem_settings_follow_the_platform_and_the_layers() {
    for platform in [LocalPlatform::MacOs, LocalPlatform::Other] {
        Jail::expect_with(|jail| {
            let data_dir = jail.directory().join("data");
            let args = RunArgs {
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            let mode = |manifest: Option<&ResolvedLocalServer>, args: &RunArgs| {
                load_on(
                    platform,
                    manifest,
                    args,
                    SnapshotRepositoryKeySource::ValidationOnly,
                )
                .unwrap()
                .worker_executor
                .filesystem_storage
                .mode
            };

            let defaults = load_on(
                platform,
                None,
                &args,
                SnapshotRepositoryKeySource::ValidationOnly,
            )
            .unwrap();
            match platform {
                LocalPlatform::MacOs => {
                    assert_eq!(
                        defaults.worker_executor.filesystem_storage.mode,
                        FilesystemStorageMode::Apfs {
                            root: data_dir.join("agents").into()
                        }
                    );
                    assert!(matches!(
                        defaults.worker_executor.filesystem_snapshots,
                        FilesystemSnapshotsConfig::Enabled(_)
                    ));
                }
                LocalPlatform::Other => {
                    assert_eq!(
                        defaults.worker_executor.filesystem_storage.mode,
                        FilesystemStorageMode::Temporary
                    );
                    assert!(matches!(
                        defaults.worker_executor.filesystem_snapshots,
                        FilesystemSnapshotsConfig::Disabled(_)
                    ));
                }
            }

            [(false, false), (false, true), (true, false), (true, true)]
                .into_iter()
                .try_for_each(|(from_env, disabled)| -> figment::Result<()> {
                    jail.clear_env();
                    let settings = if from_env {
                        jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__STORAGE_CALL_DEADLINE", "83s");
                        if disabled {
                            jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", "false");
                        }
                        String::new()
                    } else {
                        format!(
                            "[filesystem_snapshots]\nstorage_call_deadline = '83s'\n{}",
                            if disabled { "enabled = false" } else { "" }
                        )
                    };
                    jail.create_file("snapshots.toml", &settings)?;
                    let loaded = load_on(
                        platform,
                        None,
                        &RunArgs {
                            config: Some("snapshots.toml".into()),
                            data_dir: Some(data_dir.clone()),
                            ..RunArgs::default()
                        },
                        SnapshotRepositoryKeySource::ValidationOnly,
                    )
                    .unwrap();
                    if matches!(platform, LocalPlatform::MacOs) && !disabled {
                        assert_eq!(
                            snapshot_store(&loaded).storage_call_deadline(),
                            Duration::from_secs(83)
                        );
                    } else {
                        assert!(matches!(
                            loaded.worker_executor.filesystem_snapshots,
                            FilesystemSnapshotsConfig::Disabled(_)
                        ));
                    }
                    Ok(())
                })?;
            jail.clear_env();

            let manifest = local_server(LocalServer {
                agent_filesystem_root: Some(PathBuf::from("manifest-agents")),
                ..LocalServer::default()
            });
            assert_eq!(
                mode(Some(&manifest), &args),
                platform.rooted_mode(Path::new("/tmp/test-app/manifest-agents"))
            );

            let flag_args = RunArgs {
                agent_filesystem_root: Some(PathBuf::from("flag-agents")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            assert_eq!(
                mode(Some(&manifest), &flag_args),
                platform.rooted_mode(Path::new("flag-agents"))
            );

            jail.create_file(
                "golem.toml",
                r#"
                        [advanced.worker_executor.filesystem_storage.mode]
                        type = "Directory"

                        [advanced.worker_executor.filesystem_storage.mode.config]
                        root = "file-agents"
                    "#,
            )?;
            let file_args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            assert_eq!(
                mode(Some(&manifest), &file_args),
                FilesystemStorageMode::Directory {
                    root: Path::new("file-agents").into()
                }
            );

            jail.set_env(
                "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__CONFIG__ROOT",
                "env-agents",
            );
            assert_eq!(
                mode(Some(&manifest), &file_args),
                FilesystemStorageMode::Directory {
                    root: Path::new("env-agents").into()
                }
            );

            let file_and_flag_args = RunArgs {
                agent_filesystem_root: Some(PathBuf::from("flag-agents")),
                config: Some(PathBuf::from("golem.toml")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };
            assert_eq!(
                mode(Some(&manifest), &file_and_flag_args),
                platform.rooted_mode(Path::new("flag-agents"))
            );

            assert!(!data_dir.exists());
            Ok(())
        });
    }
}

#[test]
fn environment_can_disable_storage_and_snapshots_on_macos() {
    Jail::expect_with(|jail| {
        let data_dir = jail.directory().join("data");
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__TYPE",
            "Temporary",
        );
        jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__ENABLED", "false");
        let args = RunArgs {
            data_dir: Some(data_dir.clone()),
            ..RunArgs::default()
        };

        let config = load_on(
            LocalPlatform::MacOs,
            None,
            &args,
            SnapshotRepositoryKeySource::DataDir,
        )
        .unwrap();

        assert_eq!(
            config.worker_executor.filesystem_storage.mode,
            FilesystemStorageMode::Temporary
        );
        assert!(matches!(
            config.worker_executor.filesystem_snapshots,
            FilesystemSnapshotsConfig::Disabled(_)
        ));
        assert!(!data_dir.exists());
        Ok(())
    });
}

#[test]
fn temporary_storage_rejects_a_root() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__TYPE",
            "Temporary",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__FILESYSTEM_STORAGE__MODE__CONFIG__ROOT",
            "agents",
        );
        assert!(load(None, &run_args_with_data_dir("/data")).is_err());
        Ok(())
    });
}

#[test]
fn enabled_snapshots_without_a_key_use_the_key_retained_in_the_data_dir() {
    Jail::expect_with(|jail| {
        let data_dir = jail.directory().join("data");
        jail.create_file(
            "golem.toml",
            r#"
                    [filesystem_snapshots]
                    enabled = true
                "#,
        )?;
        let args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            data_dir: Some(data_dir.clone()),
            ..RunArgs::default()
        };
        let key_file = data_dir.join(filesystem_config::KEY_FILE);

        let validated = load_on(
            LocalPlatform::Other,
            None,
            &args,
            SnapshotRepositoryKeySource::ValidationOnly,
        )
        .unwrap();
        assert_eq!(
            snapshot_store(&validated).repository_key().bytes(),
            &[0; 64]
        );
        assert!(!data_dir.exists());

        let first = load_on(
            LocalPlatform::Other,
            None,
            &args,
            SnapshotRepositoryKeySource::DataDir,
        )
        .unwrap();
        let retained =
            golem_worker_executor::services::golem_config::FilesystemSnapshotRepositoryKey::parse(
                &std::fs::read_to_string(&key_file).unwrap(),
            )
            .unwrap();
        assert_eq!(
            snapshot_store(&first).repository_key().bytes(),
            retained.bytes()
        );

        let second = load_on(
            LocalPlatform::Other,
            None,
            &args,
            SnapshotRepositoryKeySource::DataDir,
        )
        .unwrap();
        assert_eq!(
            snapshot_store(&second).repository_key().bytes(),
            retained.bytes()
        );
        Ok(())
    });
}

#[test]
fn a_configured_repository_key_leaves_the_retained_key_alone() {
    Jail::expect_with(|jail| {
        let data_dir = jail.directory().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let key_file = data_dir.join(filesystem_config::KEY_FILE);
        std::fs::write(&key_file, b"corrupt").unwrap();
        jail.create_file(
            "golem.toml",
            &format!(
                r#"
                        [filesystem_snapshots]
                        enabled = true
                        repository_key = "{}"
                        save_threads = 2
                    "#,
                "2a".repeat(64)
            ),
        )?;
        let args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            data_dir: Some(data_dir.clone()),
            ..RunArgs::default()
        };
        let load_with_key = || {
            load_on(
                LocalPlatform::Other,
                None,
                &args,
                SnapshotRepositoryKeySource::DataDir,
            )
            .unwrap()
        };

        let from_file = load_with_key();
        assert_eq!(
            snapshot_store(&from_file).repository_key().bytes(),
            &[0x2a; 64]
        );

        jail.set_env(
            "GOLEM__FILESYSTEM_SNAPSHOTS__REPOSITORY_KEY",
            "3b".repeat(64),
        );
        jail.set_env("GOLEM__FILESYSTEM_SNAPSHOTS__SAVE_THREADS", "3");
        let from_env = load_with_key();
        let store = snapshot_store(&from_env);
        assert_eq!(store.repository_key().bytes(), &[0x3b; 64]);
        assert_eq!(store.save_threads().get(), 3);

        assert!(std::fs::read(&key_file).unwrap() == b"corrupt");
        Ok(())
    });
}

#[test]
fn invalid_settings_do_not_create_the_retained_key() {
    for settings in [
        r#"
                [filesystem_snapshots]
                enabled = true
                save_threads = 0
            "#,
        r#"
                [filesystem_snapshots]
                enabled = true

                [advanced.worker_executor.filesystem_storage.mode]
                type = "Directory"
            "#,
    ] {
        Jail::expect_with(|jail| {
            let data_dir = jail.directory().join("data");
            jail.create_file("golem.toml", settings)?;
            let args = RunArgs {
                config: Some(PathBuf::from("golem.toml")),
                data_dir: Some(data_dir.clone()),
                ..RunArgs::default()
            };

            assert!(
                load_on(
                    LocalPlatform::Other,
                    None,
                    &args,
                    SnapshotRepositoryKeySource::DataDir,
                )
                .is_err()
            );
            assert!(!data_dir.exists());
            Ok(())
        });
    }
}

#[test]
fn defaults_keep_all_state_in_the_data_dir() {
    let data_dir = Path::new("/data");
    let config = GolemServerConfig::for_data_dir(data_dir).unwrap();

    assert_eq!(config.router.address, "127.0.0.1");
    assert_eq!(config.router.port, DEFAULT_LOCAL_ROUTER_PORT);
    assert!(config.uses_local_registry_db());
    let DbConfig::Sqlite(registry_db) = &config.registry_service.db else {
        panic!("expected a SQLite registry database")
    };
    assert_eq!(sqlite_path(registry_db), data_dir.join("registry.db"));
    let PersistenceConfig::Sqlite(shard_manager_db) = &config.shard_manager.persistence else {
        panic!("expected a SQLite shard manager database")
    };
    assert_eq!(
        sqlite_path(shard_manager_db),
        data_dir.join("shard_manager.db")
    );
    let SchedulerStorageConfig::Sqlite(scheduler_db) = &config.worker_executor.scheduler_storage
    else {
        panic!("expected a SQLite scheduler database")
    };
    assert_eq!(sqlite_path(scheduler_db), data_dir.join("scheduler.db"));
    let SessionStoreConfig::Sqlite(sessions) = &config.worker_service.gateway_session_storage
    else {
        panic!("expected a SQLite session store")
    };
    assert_eq!(
        sqlite_path(&sessions.sqlite_config),
        data_dir.join("gateway-sessions.db")
    );
    let McpSessionStorageConfig::Sqlite(mcp_sessions) = config
        .worker_service
        .mcp
        .session
        .effective_session_storage(&config.worker_service.gateway_session_storage)
    else {
        panic!("expected the MCP sessions in the gateway's SQLite database")
    };
    assert_eq!(mcp_sessions.sqlite_config, sessions.sqlite_config);
    let KeyValueStorageConfig::MultiSqlite(key_value) = &config.worker_executor.key_value_storage
    else {
        panic!("expected one SQLite database per agent")
    };
    assert_eq!(key_value.root_dir, data_dir.join("kv-store"));
    assert!(matches!(
        config.worker_executor.indexed_storage,
        IndexedStorageConfig::KVStoreMultiSqlite(_)
    ));
    for blob_storage in [
        &config.registry_service.blob_storage,
        &config.worker_service.blob_storage,
        &config.worker_executor.blob_storage,
        &config.component_compilation_service.blob_storage,
    ] {
        assert_eq!(blob_root(blob_storage), data_dir.join("blobs"));
    }
    assert_eq!(
        config.registry_service.builtin_artifacts.cache_dir,
        Some(data_dir.join("builtin-artifacts"))
    );
    assert!(matches!(
        config.registry_service.login,
        LoginConfig::Disabled(_)
    ));
    assert_eq!(
        config.registry_service.initial_accounts["root"]
            .token
            .as_ref()
            .map(|token| token.secret()),
        Some(golem_client::LOCAL_WELL_KNOWN_TOKEN)
    );
    assert_eq!(config.worker_executor.memory.worker_memory_ratio, 0.8);
    assert!(config.worker_executor.memory.enable_measured_admission);
}

#[test]
fn manifest_local_server_values_are_used_when_cli_args_are_absent() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let manifest = local_server(LocalServer {
            system_memory_override: std::num::NonZeroU64::new(2147483648),
            router_addr: Some("127.0.0.2".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });

        let config = load(Some(&manifest), &RunArgs::default()).unwrap();

        assert_eq!(config.router.address, "127.0.0.2");
        assert_eq!(config.router.port, 9882);
        assert_eq!(config.worker_service.custom_request_port, 9008);
        assert_eq!(config.worker_service.mcp_port, 9009);
        assert_eq!(
            config.worker_executor.memory.system_memory_override,
            Some(2147483648)
        );
        assert_eq!(
            config.ports_file,
            Some(PathBuf::from("/tmp/test-app/.golem/ports.json"))
        );
        assert_eq!(config.data_dir, PathBuf::from("/tmp/test-app/.golem/data"));
        assert_eq!(
            blob_root(&config.worker_executor.blob_storage),
            Path::new("/tmp/test-app/.golem/data/blobs")
        );
        assert_eq!(
            config.worker_executor.filesystem_storage.mode,
            FilesystemStorageMode::Directory {
                root: Path::new("/tmp/test-app/.golem/agents").into()
            }
        );
        Ok(())
    });
}

#[test]
fn cli_args_override_manifest_local_server_values() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let manifest = local_server(LocalServer {
            system_memory_override: std::num::NonZeroU64::new(2147483648),
            router_addr: Some("127.0.0.2".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });
        let run_args = RunArgs {
            config: None,
            system_memory_override: std::num::NonZeroU64::new(1073741824),
            router_addr: Some("0.0.0.0".to_string()),
            router_port: Some(10000),
            custom_request_port: Some(10001),
            mcp_port: Some(10002),
            ports_file: Some(PathBuf::from("cli-ports.json")),
            data_dir: Some(PathBuf::from("cli-data")),
            clean: false,
            agent_filesystem_root: Some(PathBuf::from("cli-agents")),
        };

        let config = load(Some(&manifest), &run_args).unwrap();

        assert_eq!(config.router.address, "0.0.0.0");
        assert_eq!(config.router.port, 10000);
        assert_eq!(config.worker_service.custom_request_port, 10001);
        assert_eq!(config.worker_service.mcp_port, 10002);
        assert_eq!(
            config.worker_executor.memory.system_memory_override,
            Some(1073741824)
        );
        assert_eq!(config.ports_file, Some(PathBuf::from("cli-ports.json")));
        assert_eq!(config.data_dir, PathBuf::from("cli-data"));
        assert_eq!(
            config.worker_executor.filesystem_storage.mode,
            FilesystemStorageMode::Directory {
                root: Path::new("cli-agents").into()
            }
        );
        Ok(())
    });
}

#[test]
fn layers_apply_in_order_of_precedence() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    data_dir = "/file-data"

                    [advanced.server.router]
                    address = "127.0.0.3"
                    port = 7000

                    [advanced.worker_service]
                    custom_request_port = 7001
                    mcp_port = 7002
                "#,
        )?;
        jail.set_env("GOLEM_ADVANCED__SERVER__ROUTER__PORT", "8000");
        jail.set_env("GOLEM_ADVANCED__WORKER_SERVICE__MCP_PORT", "8002");
        let manifest = local_server(LocalServer {
            router_addr: Some("127.0.0.2".to_string()),
            router_port: Some(6000),
            custom_request_port: Some(6001),
            mcp_port: Some(6002),
            data_dir: Some(PathBuf::from("/manifest-data")),
            ..LocalServer::default()
        });
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            mcp_port: Some(9002),
            ..RunArgs::default()
        };

        let config = load(Some(&manifest), &run_args).unwrap();

        assert_eq!(config.data_dir, PathBuf::from("/file-data"));
        assert_eq!(
            blob_root(&config.registry_service.blob_storage),
            Path::new("/file-data/blobs")
        );
        assert_eq!(config.router.address, "127.0.0.3");
        assert_eq!(config.router.port, 8000);
        assert_eq!(config.worker_service.custom_request_port, 7001);
        assert_eq!(config.worker_service.mcp_port, 9002);
        Ok(())
    });
}

#[test]
fn missing_config_file_is_an_error() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let run_args = RunArgs {
            config: Some(PathBuf::from("/nonexistent/golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let error = jail_error(load(None, &run_args));

        assert!(error.contains("/nonexistent/golem.toml"), "{error}");
        Ok(())
    });
}

#[test]
fn service_sections_override_individual_fields() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.registry_service.initial_plans.default]
                    component_limit = 25
                    monthly_compute_gcu = 2

                    [advanced.registry_service.initial_accounts.root]
                    token = "a-secret-admin-token-of-sufficient-length"

                    [advanced.worker_executor.memory]
                    system_memory_override = 4294967296
                "#,
        )?;
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__COMPUTE",
            "true",
        );
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let config = load(None, &run_args).unwrap();

        let plan = &config.registry_service.initial_plans["default"];
        assert_eq!(plan.component_limit, 25);
        assert_eq!(plan.app_limit, UNLIMITED);
        assert_eq!(plan.plan_id, local_plan_id());
        let root = &config.registry_service.initial_accounts["root"];
        assert_eq!(
            root.token.as_ref().map(|token| token.secret()),
            Some("a-secret-admin-token-of-sufficient-length")
        );
        assert_eq!(root.email, AccountEmail::new("initial@user"));
        assert_eq!(config.registry_service.initial_accounts.len(), 3);
        assert_eq!(
            config.worker_executor.memory.system_memory_override,
            Some(4294967296)
        );
        assert!(config.worker_executor.resource_usage_metering.compute);
        assert!(!config.worker_executor.resource_usage_metering.memory);
        Ok(())
    });
}

#[test]
fn local_resource_policy_defaults_preserve_opt_in_and_storage_capability() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        let config = load(None, &run_args_with_data_dir("/data")).unwrap();
        let plan = &config.registry_service.initial_plans["default"];
        assert_eq!(plan.monthly_compute_gcu, 0);
        assert_eq!(plan.monthly_memory_gb_seconds, 0);
        assert_eq!(plan.monthly_durable_storage_gb_month, 0);
        assert_eq!(plan.monthly_ephemeral_storage_gb_month, 0);
        assert!(!plan.overage_eligible);
        assert!(!plan.max_storage_per_agent_enabled);
        assert_eq!(plan.max_memory_per_agent, UNLIMITED);
        assert!(!config.worker_executor.resource_usage_metering.compute);
        assert!(!config.worker_executor.resource_usage_metering.memory);
        assert!(!config.worker_executor.resource_usage_metering.filesystem);
        Ok(())
    });
}

#[test]
fn per_agent_storage_capability_requires_managed_xfs_without_billing() {
    for mode in [
        FilesystemStorageMode::Temporary,
        FilesystemStorageMode::Directory {
            root: PathBuf::from("/data").into_boxed_path(),
        },
        FilesystemStorageMode::ReflinkXfs {
            root: PathBuf::from("/data").into_boxed_path(),
        },
        FilesystemStorageMode::Apfs {
            root: PathBuf::from("/data").into_boxed_path(),
        },
        FilesystemStorageMode::ManagedXfs {
            root: PathBuf::from("/data").into_boxed_path(),
            xfs_quotad_socket: PathBuf::from("/data/xfs-quotad.sock").into_boxed_path(),
        },
    ] {
        let managed = matches!(mode, FilesystemStorageMode::ManagedXfs { .. });
        let mut config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
        config.worker_executor.filesystem_storage.mode = mode;
        assert!(!config.worker_executor.resource_usage_metering.filesystem);
        let plan = config
            .registry_service
            .initial_plans
            .get_mut("default")
            .unwrap();
        plan.max_storage_per_agent_enabled = true;
        plan.max_storage_per_agent = 100;
        let result = validate_resource_policy(&config, &[]);
        assert_eq!(result.is_ok(), managed, "{result:?}");
        if let Err(error) = result {
            assert!(error.to_string().contains("managed XFS"), "{error}");
        }
        config
            .registry_service
            .initial_plans
            .get_mut("default")
            .unwrap()
            .max_storage_per_agent_enabled = false;
        assert!(validate_resource_policy(&config, &[]).is_ok());
    }
}

#[test]
fn enabled_local_meter_requires_an_explicit_amount_and_accepts_explicit_zero() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__RESOURCE_USAGE_METERING__MEMORY",
            "true",
        );
        let args = run_args_with_data_dir("/data");
        let error = jail_error(load(None, &args));
        assert!(
            error.contains("registry_service.initial_plans.default.monthly_memory_gb_seconds"),
            "{error}"
        );
        jail.set_env(
            "GOLEM_ADVANCED__REGISTRY_SERVICE__INITIAL_PLANS__DEFAULT__MONTHLY_MEMORY_GB_SECONDS",
            "0",
        );
        let config = load(None, &args).unwrap();
        assert!(config.worker_executor.resource_usage_metering.memory);
        assert_eq!(
            config.registry_service.initial_plans["default"].monthly_memory_gb_seconds,
            0
        );
        Ok(())
    });
}

#[test]
fn selecting_another_variant_drops_the_fields_of_the_default_variant() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.worker_executor.key_value_storage]
                    type = "Postgres"

                    [advanced.worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 20
                    schema = "golem_worker_executor"
                "#,
        )?;
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__TYPE",
            "Postgres",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__HOST",
            "db",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__PORT",
            "5432",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__USERNAME",
            "golem",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__PASSWORD",
            "golem",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__MAX_CONNECTIONS",
            "5",
        );
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        // The scheduler's default SQLite variant has a `database` field too, so without
        // dropping it the file path would become the Postgres database name.
        let error = jail_error(load(None, &run_args));
        assert!(error.contains("database"), "{error}");

        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__DATABASE",
            "golem",
        );
        let config = load(None, &run_args).unwrap();

        let KeyValueStorageConfig::Postgres(key_value) = &config.worker_executor.key_value_storage
        else {
            panic!("expected Postgres key-value storage")
        };
        assert_eq!(key_value.postgres.host, "db");
        assert_eq!(key_value.postgres.max_connections, 20);
        let SchedulerStorageConfig::Postgres(scheduler) = &config.worker_executor.scheduler_storage
        else {
            panic!("expected Postgres scheduler storage")
        };
        assert_eq!(scheduler.postgres.database, "golem");
        assert_eq!(scheduler.postgres.schema, None);
        Ok(())
    });
}

#[test]
fn launcher_owned_keys_are_rejected() {
    Jail::expect_with(|jail| {
        jail.set_env("GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT", "9000");
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(error.contains("worker_executor.grpc.port"), "{error}");
        assert!(
            error.contains("GOLEM_ADVANCED__WORKER_EXECUTOR__GRPC__PORT"),
            "{error}"
        );
        Ok(())
    });
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.registry_service.tracing.stdout]
                    enabled = false
                "#,
        )?;
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };
        let error = jail_error(load(None, &run_args));
        assert!(error.contains("registry_service.tracing"), "{error}");
        assert!(error.contains("golem.toml"), "{error}");
        Ok(())
    });
}

#[test]
fn the_golem_issuer_is_set_once_for_the_whole_server() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__SERVER__GOLEM_ISSUER",
            "https://id.acme.example",
        );
        let config = load(None, &run_args_with_data_dir("/data")).unwrap();
        let issuer: GolemIssuer = "https://id.acme.example".parse().unwrap();
        assert_eq!(config.golem_issuer, Some(issuer));

        jail.set_env(
            "GOLEM_ADVANCED__SERVER__GOLEM_ISSUER",
            "http://id.acme.example",
        );
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(error.contains("http://id.acme.example"), "{error}");
        Ok(())
    });
}

#[test]
fn an_empty_or_blank_golem_issuer_counts_as_not_set() {
    for value in ["", "   "] {
        Jail::expect_with(|jail| {
            jail.set_env("GOLEM_ADVANCED__SERVER__GOLEM_ISSUER", value);
            let config = load(None, &run_args_with_data_dir("/data")).unwrap();
            assert_eq!(config.golem_issuer, None);
            Ok(())
        });
    }
}

#[test]
fn the_golem_issuer_of_one_service_is_rejected() {
    for service in ["registry_service", "worker_service", "worker_executor"] {
        Jail::expect_with(|jail| {
            let variable = format!("GOLEM_ADVANCED__{}__GOLEM_ISSUER", service.to_uppercase());
            jail.set_env(&variable, "https://id.acme.example");
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(
                error.contains(&format!("{service}.golem_issuer")),
                "{error}"
            );
            assert!(error.contains(&variable), "{error}");
            Ok(())
        });
    }
}

#[test]
fn unknown_top_level_keys_are_rejected() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__SERVER__RESOURCE_USAGE_METERING__COMPUTE",
            "true",
        );
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(error.contains("resource_usage_metering"), "{error}");
        Ok(())
    });
    [
        ("GOLEM__ROUTER__PORT", "18444"),
        (
            "GOLEM__GOLEM_ISSUER",
            "urn:golem:issuer:12345678-1234-4234-8234-123456789012",
        ),
        ("GOLEM__PORTS_FILE", "ports.json"),
    ]
    .into_iter()
    .for_each(|(key, value)| {
        Jail::expect_with(|jail| {
            jail.set_env(key, value);
            let error = jail_error(load(None, &run_args_with_data_dir("/data")));
            assert!(error.contains("advanced channel"), "{key}: {error}");
            Ok(())
        });
    });
}

#[test]
fn blob_root_alone_reaches_every_embedded_blob_consumer() {
    [false, true].into_iter().for_each(|environment| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            let mut args = run_args_with_data_dir("/data");
            if environment {
                jail.set_env("GOLEM__DATA_DISK__BLOB_ROOT", "/srv/blob-data");
            } else {
                jail.create_file(
                    "settings.toml",
                    "[data_disk]\nblob_root = '/srv/blob-data'\n",
                )?;
                args.config = Some("settings.toml".into());
            }
            let config = load(None, &args).unwrap();
            [
                &config.registry_service.blob_storage,
                &config.worker_service.blob_storage,
                &config.worker_executor.blob_storage,
                &config.component_compilation_service.blob_storage,
            ]
            .into_iter()
            .for_each(|storage| assert_eq!(blob_root(storage), Path::new("/srv/blob-data")));
            Ok(())
        });
    });
}

#[test]
fn invalid_supported_settings_name_the_original_field_and_source() {
    [false, true].into_iter().for_each(|environment_failure| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file(
                "settings.toml",
                if environment_failure {
                    "[logging.stdout]\nenabled = true\n"
                } else {
                    "[logging.stdout]\nenabled = 'not-a-boolean'\n"
                },
            )?;
            let expected_source = if environment_failure {
                jail.set_env("GOLEM__LOGGING__STDOUT__ENABLED", "not-a-boolean");
                "environment variable"
            } else {
                jail.set_env("GOLEM__LOGGING__STDERR__ENABLED", "true");
                "settings.toml"
            };
            let args = RunArgs {
                config: Some(PathBuf::from("settings.toml")),
                ..run_args_with_data_dir("/data")
            };
            let embedded = jail_error(load(None, &args));
            let mut standalone = golem_worker_service::config::make_worker_service_config_loader();
            standalone.config_file_name = PathBuf::from("settings.toml");
            let standalone = standalone.load().unwrap_err().to_string();
            [embedded, standalone].into_iter().for_each(|error| {
                assert!(
                    error
                        .to_ascii_lowercase()
                        .contains("logging.stdout.enabled"),
                    "{error}"
                );
                assert!(error.contains(expected_source), "{error}");
                if environment_failure {
                    assert!(!error.contains("settings.toml"), "{error}");
                } else {
                    assert!(!error.contains("environment variable"), "{error}");
                }
            });
            Ok(())
        });
    });
}

#[test]
fn invalid_advanced_settings_name_the_field_and_source() {
    ["file", "environment"].into_iter().for_each(|source| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("settings.toml", "")?;
            let mut args = run_args_with_data_dir("/data");
            let expected_source = if source == "file" {
                jail.create_file(
                    "settings.toml",
                    r#"
                            [advanced.worker_service.route_resolver]
                            trusted_ingress_addresses = ["10.0.0.1"]
                        "#,
                )?;
                args.config = Some(PathBuf::from("settings.toml"));
                "settings.toml"
            } else {
                jail.set_env(
                    "GOLEM_ADVANCED__WORKER_SERVICE__ROUTE_RESOLVER__TRUSTED_INGRESS_ADDRESSES",
                    r#"["10.0.0.1"]"#,
                );
                "environment variable"
            };
            let embedded = jail_error(load(None, &args));
            let mut standalone = golem_worker_service::config::make_worker_service_config_loader();
            standalone.config_file_name = "settings.toml".into();
            let standalone = standalone.load().unwrap_err().to_string();
            [embedded, standalone].into_iter().for_each(|error| {
                assert!(
                    error
                        .to_ascii_lowercase()
                        .contains("worker_service.route_resolver.trusted_ingress_addresses"),
                    "{error}"
                );
                assert!(error.contains(expected_source), "{error}");
            });
            Ok(())
        });
    });
}

#[test]
fn projected_storage_failures_keep_the_file_source() {
    [
        (
            "state_store = 'postgres'\npostgres.port = 'invalid-port'\n",
            "postgres",
            true,
        ),
        (
            "blob_store = 's3'\nobject_storage.path_style = 'invalid-boolean'\n",
            "object_storage",
            false,
        ),
        (
            "blob_store = 's3'\nobject_storage.data_bucket = 17\n",
            "object_storage",
            false,
        ),
    ]
    .into_iter()
    .for_each(|(settings, original_container, postgres)| {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("settings.toml", settings)?;
            let args = RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            };
            let embedded = jail_error(load(None, &args));
            let standalone = if postgres {
                let mut loader = golem_registry_service::config::make_config_loader();
                loader.config_file_name = "settings.toml".into();
                loader.load().map(|_| ())
            } else {
                let mut loader = golem_worker_service::config::make_worker_service_config_loader();
                loader.config_file_name = "settings.toml".into();
                loader.load().map(|_| ())
            }
            .unwrap_err()
            .to_string();
            [embedded, standalone].into_iter().for_each(|diagnostic| {
                assert!(diagnostic.contains("settings.toml"), "{diagnostic}");
                assert!(diagnostic.contains(original_container), "{diagnostic}");
            });
            Ok(())
        });
    });
}

#[test]
fn advanced_logging_repairs_invalid_supported_values_before_extraction() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_file(
            "settings.toml",
            r#"
                [logging.stdout]
                enabled = "not-a-boolean"
                [advanced.server.logging.stdout]
                enabled = true
            "#,
        )?;
        let embedded = load(
            None,
            &RunArgs {
                config: Some("settings.toml".into()),
                ..run_args_with_data_dir("/data")
            },
        )
        .unwrap();
        assert!(embedded.logging.stdout.enabled);
        jail.create_file(
            "standalone.toml",
            r#"
                [logging.stdout]
                enabled = "not-a-boolean"
                [advanced.worker_service.tracing.stdout]
                enabled = true
            "#,
        )?;
        let mut standalone = golem_worker_service::config::make_worker_service_config_loader();
        standalone.config_file_name = "standalone.toml".into();
        assert!(standalone.load().unwrap().tracing.stdout.enabled);
        Ok(())
    });
}

#[test]
fn postgres_example_uses_the_schemas_of_the_per_service_deployment() {
    let compose = include_str!("../../../../docker-examples/published-postgres/compose.yaml");
    let config = GolemServerConfig::postgres_s3_example(Path::new("/data")).unwrap();

    let DbConfig::Postgres(registry) = &config.registry_service.db else {
        panic!("expected a Postgres registry database")
    };
    let PersistenceConfig::Postgres(shard_manager) = &config.shard_manager.persistence else {
        panic!("expected a Postgres shard manager database")
    };
    let KeyValueStorageConfig::Postgres(key_value) = &config.worker_executor.key_value_storage
    else {
        panic!("expected Postgres key-value storage")
    };
    let IndexedStorageConfig::Postgres(indexed) = &config.worker_executor.indexed_storage else {
        panic!("expected Postgres indexed storage")
    };
    let SchedulerStorageConfig::Postgres(scheduler) = &config.worker_executor.scheduler_storage
    else {
        panic!("expected Postgres scheduler storage")
    };

    for (variable, postgres) in [
        (
            "GOLEM_ADVANCED__REGISTRY_SERVICE__DB__CONFIG__SCHEMA",
            registry,
        ),
        (
            "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE__CONFIG__SCHEMA",
            shard_manager,
        ),
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__SCHEMA",
            &key_value.postgres,
        ),
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__INDEXED_STORAGE__CONFIG__SCHEMA",
            &indexed.postgres,
        ),
        (
            "GOLEM_ADVANCED__WORKER_EXECUTOR__SCHEDULER_STORAGE__CONFIG__SCHEMA",
            &scheduler.postgres,
        ),
    ] {
        let schema = postgres.schema.as_deref().unwrap();
        assert!(
            compose.contains(&format!("{variable}: {schema}")),
            "{variable}: {schema} is not in the per-service compose file"
        );
    }
    for blob_storage in [
        &config.registry_service.blob_storage,
        &config.worker_service.blob_storage,
        &config.worker_executor.blob_storage,
        &config.component_compilation_service.blob_storage,
    ] {
        let BlobStorageConfig::S3(s3) = blob_storage else {
            panic!("expected S3 blob storage")
        };
        assert!(s3.namespace_prefixes);
        s3.validate().unwrap();
        let mut buckets = vec![
            &s3.compilation_cache_bucket,
            &s3.custom_data_bucket,
            &s3.oplog_payload_bucket,
            &s3.initial_agent_files_bucket,
            &s3.components_bucket,
            &s3.filesystem_snapshots_bucket,
        ];
        buckets.extend(&s3.compressed_oplog_buckets);
        assert!(buckets.iter().all(|bucket| *bucket == EXAMPLE_S3_BUCKET));
    }
    assert!(!config.uses_local_registry_db());
}

#[test]
fn reference_config_is_a_valid_config_file() {
    for make in [
        GolemServerConfig::for_data_dir as fn(&Path) -> anyhow::Result<GolemServerConfig>,
        GolemServerConfig::postgres_s3_example,
    ] {
        let reference = toml::to_string(&configurable_value(make).unwrap()).unwrap();
        for data_dir in [REFERENCE_DATA_DIR, "/moved"] {
            let reference = reference.replace(
                &format!("data_dir = \"{REFERENCE_DATA_DIR}\""),
                &format!("data_dir = \"{data_dir}\""),
            );
            Jail::expect_with(|jail| {
                jail.create_file("golem.toml", &reference)?;
                let run_args = RunArgs {
                    config: Some(PathBuf::from("golem.toml")),
                    ..RunArgs::default()
                };

                let loaded = load(None, &run_args).unwrap();

                assert_eq!(
                    serde_json::to_value(&loaded).unwrap(),
                    serde_json::to_value(make(Path::new(data_dir)).unwrap()).unwrap()
                );
                Ok(())
            });
        }
    }
}

#[test]
fn selecting_a_variant_without_fields_needs_no_config() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__REGISTRY_SERVICE__COMPONENT_COMPILATION__TYPE",
            "Disabled",
        );
        jail.set_env(
            "GOLEM_ADVANCED__REGISTRY_SERVICE__BUILTIN_PLUGINS__TYPE",
            "Disabled",
        );
        let config = load(None, &run_args_with_data_dir("/data")).unwrap();
        assert!(matches!(
            config.registry_service.component_compilation,
            ComponentCompilationConfig::Disabled(_)
        ));
        assert!(matches!(
            config.registry_service.builtin_plugins,
            BuiltinPluginsConfig::Disabled(_)
        ));
        Ok(())
    });
}

#[test]
fn selecting_a_unit_variant_over_a_lower_layer_drops_its_config() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.worker_executor.filesystem_storage.mode]
                    type = "Temporary"
                "#,
        )?;
        let manifest = local_server(LocalServer {
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
            ..LocalServer::default()
        });
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let config = load(Some(&manifest), &run_args).unwrap();

        assert_eq!(
            config.worker_executor.filesystem_storage.mode,
            FilesystemStorageMode::Temporary
        );
        Ok(())
    });
}

#[test]
fn variant_fields_do_not_leak_between_user_layers() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.worker_executor.key_value_storage]
                    type = "Postgres"

                    [advanced.worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 77
                    schema = "golem_worker_executor"
                "#,
        )?;
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE",
            "Sqlite",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__DATABASE",
            "/data/kv.db",
        );
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__FOREIGN_KEYS",
            "false",
        );
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let error = jail_error(load(None, &run_args));
        assert!(error.contains("max_connections"), "{error}");

        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__CONFIG__MAX_CONNECTIONS",
            "3",
        );
        let config = load(None, &run_args).unwrap();
        let KeyValueStorageConfig::Sqlite(sqlite) = &config.worker_executor.key_value_storage
        else {
            panic!("expected SQLite key-value storage")
        };
        assert_eq!(sqlite.max_connections, 3);
        assert_eq!(sqlite.database, "/data/kv.db");
        Ok(())
    });
}

#[test]
fn the_session_store_counts_with_its_default_schema() {
    let mut config = GolemServerConfig::postgres_s3_example(Path::new("/data")).unwrap();
    let (
        SessionStoreConfig::Postgres(gateway_sessions),
        McpSessionStorageConfig::Postgres(mcp_sessions),
    ) = (
        &config.worker_service.gateway_session_storage,
        config
            .worker_service
            .mcp
            .session
            .effective_session_storage(&config.worker_service.gateway_session_storage),
    )
    else {
        panic!("expected the MCP sessions in the gateway's Postgres schema")
    };
    assert_eq!(
        mcp_sessions.postgres_config,
        gateway_sessions.postgres_config_with_schema()
    );
    let SessionStoreConfig::Postgres(sessions) = &mut config.worker_service.gateway_session_storage
    else {
        panic!("expected a Postgres session store")
    };
    sessions.postgres_config.schema = None;
    assert!(validate(&config).is_ok());

    let DbConfig::Postgres(registry) = &mut config.registry_service.db else {
        panic!("expected a Postgres registry database")
    };
    registry.schema = Some(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA.to_string());
    let error = validate(&config).unwrap_err().to_string();
    assert!(
        error.contains("worker_service.gateway_session_storage"),
        "{error}"
    );
    assert!(
        error.contains(DEFAULT_POSTGRES_SESSION_STORE_SCHEMA),
        "{error}"
    );
}

#[test]
fn postgres_stores_need_separate_schemas() {
    Jail::expect_with(|jail| {
        let postgres = |section: &str| {
            format!(
                r#"
                        [{section}]
                        type = "Postgres"

                        [{section}.config]
                        host = "db"
                        port = 5432
                        database = "golem"
                        username = "golem"
                        password = "golem"
                        max_connections = 10
                    "#
            )
        };
        jail.create_file(
            "golem.toml",
            &format!(
                "{}{}",
                postgres("advanced.registry_service.db"),
                postgres("advanced.shard_manager.persistence")
            ),
        )?;
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let error = jail_error(load(None, &run_args));

        assert!(error.contains("registry_service.db"), "{error}");
        assert!(error.contains("shard_manager.persistence"), "{error}");
        assert!(error.contains("public"), "{error}");
        Ok(())
    });
}

#[test]
fn webhook_keys_must_match() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_SERVICE__WEBHOOK_CALLBACK_HANDLER__HMAC_KEY",
            "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
        );
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(
            error.contains("worker_executor.agent_webhooks_service.hmac_key"),
            "{error}"
        );
        Ok(())
    });
}

#[test]
fn the_management_host_is_reserved() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__REGISTRY_SERVICE__DOMAIN_REGISTRATION__RESERVED_DOMAINS",
            r#"["status.golem.example.com"]"#,
        );
        jail.set_env(
            "GOLEM_ADVANCED__SERVER__ROUTER__MANAGEMENT_HOST",
            "api.golem.example.com",
        );
        let config = load(None, &run_args_with_data_dir("/data")).unwrap();
        assert_eq!(
            config.router.management_host.as_deref(),
            Some("api.golem.example.com")
        );
        assert_eq!(
            config.reserved_domains(),
            ["status.golem.example.com", "api.golem.example.com"]
        );
        Ok(())
    });

    let config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
    assert!(config.reserved_domains().is_empty());
}

#[test]
fn invalid_management_host_is_rejected() {
    Jail::expect_with(|jail| {
        jail.set_env(
            "GOLEM_ADVANCED__SERVER__ROUTER__MANAGEMENT_HOST",
            "https://api.golem.example.com",
        );
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(error.contains("router.management_host"), "{error}");
        Ok(())
    });
}

#[test]
fn unknown_router_fields_are_rejected() {
    Jail::expect_with(|jail| {
        jail.set_env("GOLEM_ADVANCED__SERVER__ROUTER__ADRESS", "0.0.0.0");
        let error = jail_error(load(None, &run_args_with_data_dir("/data")));
        assert!(error.contains("adress"), "{error}");
        Ok(())
    });
}

#[test]
fn selecting_the_default_variant_again_restores_its_fields() {
    Jail::expect_with(|jail| {
        jail.create_file(
            "golem.toml",
            r#"
                    [advanced.worker_executor.key_value_storage]
                    type = "Postgres"

                    [advanced.worker_executor.key_value_storage.config]
                    host = "db"
                    port = 5432
                    database = "golem"
                    username = "golem"
                    password = "golem"
                    max_connections = 10
                    schema = "golem_worker_executor"
                "#,
        )?;
        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__KEY_VALUE_STORAGE__TYPE",
            "MultiSqlite",
        );
        let run_args = RunArgs {
            config: Some(PathBuf::from("golem.toml")),
            ..run_args_with_data_dir("/data")
        };

        let config = load(None, &run_args).unwrap();

        let KeyValueStorageConfig::MultiSqlite(key_value) =
            &config.worker_executor.key_value_storage
        else {
            panic!("expected one SQLite database per agent")
        };
        assert_eq!(key_value.root_dir, Path::new("/data/kv-store"));
        assert_eq!(key_value.max_connections, 4);
        Ok(())
    });
}

#[test]
fn state_location_is_reported_per_store() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().join("data");
    let clean_root = std::fs::canonicalize(temp_dir.path()).unwrap().join("data");

    let local = GolemServerConfig::for_data_dir(&data_dir).unwrap();
    assert!(local.state_outside(&clean_root).is_empty());

    let remote = GolemServerConfig::postgres_s3_example(&data_dir).unwrap();
    assert_eq!(
        remote.state_outside(&clean_root),
        vec![
            "registry_service.db",
            "registry_service.blob_storage",
            "shard_manager.persistence",
            "worker_service.gateway_session_storage",
            "worker_service.blob_storage",
            "worker_executor.key_value_storage",
            "worker_executor.indexed_storage",
            "worker_executor.scheduler_storage",
            "worker_executor.blob_storage",
            "component_compilation_service.blob_storage",
        ]
    );

    let mut moved_scheduler = local.clone();
    moved_scheduler.worker_executor.scheduler_storage =
        sqlite_scheduler(temp_dir.path().join("elsewhere/scheduler.db"));
    assert_eq!(
        moved_scheduler.state_outside(&clean_root),
        vec!["worker_executor.scheduler_storage"]
    );

    let local_key_value =
        KeyValueStorageInnerConfig::MultiSqlite(KeyValueStorageMultiSqliteConfig {
            root_dir: data_dir.join("kv-store"),
            max_connections: 4,
            foreign_keys: false,
        });
    let mut routed = local.clone();
    routed.worker_executor.key_value_storage = KeyValueStorageConfig::NamespaceRouted(
        golem_worker_executor::services::golem_config::KeyValueStorageNamespaceRoutedConfig {
            cache: local_key_value.clone(),
            persistent: local_key_value,
        },
    );
    assert!(routed.state_outside(&clean_root).is_empty());
    let KeyValueStorageConfig::NamespaceRouted(routed_key_value) =
        &mut routed.worker_executor.key_value_storage
    else {
        unreachable!()
    };
    routed_key_value.persistent =
        KeyValueStorageInnerConfig::Redis(golem_common::config::RedisConfig::default());
    assert_eq!(
        routed.state_outside(&clean_root),
        vec!["worker_executor.key_value_storage"]
    );
}

fn sqlite_scheduler(database: PathBuf) -> SchedulerStorageConfig {
    SchedulerStorageConfig::Sqlite(DbSqliteConfig {
        database: database.to_string_lossy().into_owned(),
        max_connections: 4,
        foreign_keys: false,
    })
}

#[test]
fn parent_components_that_leave_the_data_directory_are_outside() {
    let temp_dir = tempfile::tempdir().unwrap();
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let clean_root = std::fs::canonicalize(&data_dir).unwrap();
    let mut config = GolemServerConfig::for_data_dir(&data_dir).unwrap();

    config.worker_executor.scheduler_storage =
        sqlite_scheduler(data_dir.join("../external/scheduler.db"));
    assert_eq!(
        config.state_outside(&clean_root),
        vec!["worker_executor.scheduler_storage"]
    );

    config.worker_executor.scheduler_storage =
        sqlite_scheduler(data_dir.join("not-created/../scheduler.db"));
    assert!(config.state_outside(&clean_root).is_empty());
}

#[cfg(unix)]
mod unix {
    use super::*;
    use test_r::test;

    #[test]
    fn symlinks_that_leave_the_data_directory_are_outside() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(temp_dir.path()).unwrap();
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();

        let data_dir = base.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        symlink(&elsewhere, data_dir.join("kv-store")).unwrap();
        symlink(
            elsewhere.join("scheduler.db"),
            data_dir.join("scheduler.db"),
        )
        .unwrap();
        let config = GolemServerConfig::for_data_dir(&data_dir).unwrap();
        assert_eq!(
            config.state_outside(&data_dir),
            vec![
                "worker_executor.key_value_storage",
                "worker_executor.scheduler_storage",
            ]
        );

        let linked_data_dir = base.join("linked-data");
        symlink(base.join("real-data"), &linked_data_dir).unwrap();
        let config = GolemServerConfig::for_data_dir(&linked_data_dir).unwrap();
        let outside = config.state_outside(&linked_data_dir);
        assert!(outside.contains(&"registry_service.db"), "{outside:?}");
        assert_eq!(outside, config.state_outside(&base.join("unrelated")));
    }
}

#[test]
fn every_listener_binds_loopback_by_default() {
    for make in [
        GolemServerConfig::for_data_dir as fn(&Path) -> anyhow::Result<GolemServerConfig>,
        GolemServerConfig::postgres_s3_example,
    ] {
        let config = make(Path::new("/data")).unwrap();
        let addresses = config.listener_addresses();
        assert_eq!(
            addresses.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec![
                "router.address",
                "registry_service.bind_address",
                "shard_manager.bind_address",
                "worker_service.bind_address",
                "worker_executor.bind_address",
                "component_compilation_service.bind_address",
            ]
        );
        for (key, address) in addresses {
            assert_eq!(address, "127.0.0.1", "{key}");
        }
    }
}

#[test]
fn registry_wiring_is_set_by_the_launcher() {
    let config = GolemServerConfig::for_data_dir(Path::new("/data")).unwrap();
    let ComponentCompilationConfig::Enabled(compilation) =
        &config.registry_service.component_compilation
    else {
        panic!("expected component compilation to be enabled")
    };
    assert_eq!(compilation.host, INTERNAL_HOST);
}
