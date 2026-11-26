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
use figment::providers::{Format, Toml};
use test_r::test;

#[test]
fn generated_store_containers_keep_the_file_source() {
    #[derive(Debug, serde::Deserialize)]
    struct Storage {
        #[serde(rename = "db")]
        _db: super::super::DbConfig,
    }

    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_file(
            "settings.toml",
            "state_store = 'postgres'\npostgres.port = 'invalid-port'\n",
        )?;
        let input = Figment::from(Toml::file_exact("settings.toml"));
        let error = project("registry_service", &input)
            .extract::<Storage>()
            .unwrap_err();
        assert_eq!(error.path, ["db"]);
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("settings.toml"), "{diagnostic}");

        jail.create_file("settings.toml", "logging.stdout = false\n")?;
        let input = Figment::from(Toml::file_exact("settings.toml"));
        let diagnostic = project("worker_executor", &input)
            .extract::<Value>()
            .unwrap_err()
            .to_string();
        assert!(
            diagnostic.contains("logging.stdout must be a table"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("settings.toml"), "{diagnostic}");
        Ok(())
    });
}

#[test]
fn data_disk_root_reaches_the_default_blob_backend() {
    let input = Figment::from(Serialized::default("data_disk.blob_root", "/srv/blob-data"))
        .merge(Serialized::default("data_dir", "/srv/state"));
    let projected = all_in_one(&input).unwrap();
    [
        "registry_service",
        "worker_service",
        "worker_executor",
        "component_compilation_service",
    ]
    .into_iter()
    .for_each(|service| {
        assert_eq!(
            projected
                .extract_inner::<String>(&format!("{service}.blob_storage.config.root"))
                .unwrap(),
            "/srv/blob-data"
        );
    });
    let selected_s3 = all_in_one(&input.merge(Serialized::default("blob_store", "s3"))).unwrap();
    assert_eq!(
        selected_s3
            .extract_inner::<String>("registry_service.blob_storage.type")
            .unwrap(),
        "S3"
    );
}

#[test]
fn advanced_server_data_dir_is_rejected() {
    [false, true].into_iter().for_each(|from_env| {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            jail.create_file("settings.toml", "data_dir = '/supported-file'\n")?;
            if from_env {
                jail.set_env("GOLEM__DATA_DIR", "/supported-env");
            }
            let input = || {
                Figment::from(Toml::file_exact("settings.toml"))
                    .merge(super::super::env_config_provider())
            };
            assert_eq!(
                all_in_one(&input())?.extract_inner::<String>("data_dir")?,
                if from_env {
                    "/supported-env"
                } else {
                    "/supported-file"
                }
            );
            if from_env {
                jail.set_env("GOLEM_ADVANCED__SERVER__DATA_DIR", "/split-root");
            } else {
                jail.create_file(
                    "settings.toml",
                    "data_dir = '/supported-file'\nadvanced.server.data_dir = '/split-root'\n",
                )?;
            }
            let diagnostic = all_in_one(&input())
                .unwrap_err()
                .to_string()
                .to_ascii_lowercase();
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
            Ok(())
        });
    });
}

#[test]
fn advanced_variant_selection_discards_replaced_storage_fields() {
    [
        ("Sqlite", false),
        ("Sqlite", true),
        ("Postgres", false),
        ("Postgres", true),
    ]
    .into_iter()
    .for_each(|(variant, explicit_database)| {
        let mut advanced = json!({"type": variant});
        if explicit_database {
            advanced["config"] = json!({"database": "selected.db"});
        }
        let input = Figment::from(Serialized::defaults(json!({
            "state_store": "postgres",
            "postgres": {
                "host": "state.internal", "database": "tenant_state",
                "max_connections": 43,
            },
            "advanced": {"shard_manager": {"persistence": advanced}},
        })));
        [
            project("shard_manager", &input)
                .extract_inner::<Value>("persistence")
                .unwrap(),
            all_in_one(&input)
                .unwrap()
                .extract_inner::<Value>("shard_manager.persistence")
                .unwrap(),
        ]
        .into_iter()
        .for_each(|store| {
            assert_eq!(store["type"], variant);
            assert_eq!(
                store.pointer("/config/database").and_then(Value::as_str),
                if explicit_database {
                    Some("selected.db")
                } else if variant == "Postgres" {
                    Some("tenant_state")
                } else {
                    None
                }
            );
            assert_eq!(
                store.pointer("/config/host").and_then(Value::as_str),
                (variant == "Postgres").then_some("state.internal")
            );
            assert_eq!(
                store
                    .pointer("/config/max_connections")
                    .and_then(Value::as_u64),
                (variant == "Postgres").then_some(43)
            );
        });
    });
}

#[test]
fn sqlite_with_a_redis_cache_has_independent_indexed_storage() {
    [
        json!({"cache": {"enabled": true}}),
        json!({"state_store": "sqlite", "cache": {"enabled": true}}),
    ]
    .into_iter()
    .for_each(|settings| {
        let input = Figment::from(Serialized::defaults(settings))
            .merge(Serialized::default("data_dir", "/state"));
        let executor = project("worker_executor", &input);
        assert_eq!(
            executor
                .extract_inner::<String>("key_value_storage.type")
                .unwrap(),
            "NamespaceRouted"
        );
        assert_eq!(
            executor
                .extract_inner::<String>("indexed_storage.type")
                .unwrap(),
            "MultiSqlite"
        );
        assert_eq!(
            executor
                .extract_inner::<String>("indexed_storage.config.root_dir")
                .unwrap(),
            "/state/kv-store"
        );
        assert_eq!(
            executor
                .extract_inner::<String>("key_value_storage.config.persistent.type")
                .unwrap(),
            "MultiSqlite"
        );
    });
    let uncached = project(
        "worker_executor",
        &Figment::from(Serialized::defaults(json!({"state_store": "sqlite"}))),
    );
    assert_eq!(
        uncached
            .extract_inner::<String>("key_value_storage.type")
            .unwrap(),
        "MultiSqlite"
    );
    assert!(
        ["KVStoreMultiSqlite", "MultiSqlite"].contains(
            &uncached
                .extract_inner::<String>("indexed_storage.type")
                .unwrap()
                .as_str()
        )
    );
}

#[test]
fn worker_service_sessions_use_the_selected_store() {
    [("sqlite", "Sqlite"), ("postgres", "Postgres")]
        .into_iter()
        .for_each(|(state_store, expected)| {
            let input = Figment::from(Serialized::defaults(json!({
                "state_store": state_store,
                "cache": {"enabled": false},
            })));
            let persistent = project("worker_service", &input);
            assert_eq!(
                persistent
                    .extract_inner::<String>("gateway_session_storage.type")
                    .unwrap(),
                expected
            );
            assert_eq!(
                persistent
                    .extract_inner::<humantime_serde::Serde<std::time::Duration>>(
                        "gateway_session_storage.config.pending_login_expiration"
                    )
                    .unwrap()
                    .into_inner(),
                std::time::Duration::from_secs(3600)
            );
            assert_eq!(
                persistent
                    .extract_inner::<humantime_serde::Serde<std::time::Duration>>(
                        "gateway_session_storage.config.cleanup_interval"
                    )
                    .unwrap()
                    .into_inner(),
                std::time::Duration::from_secs(300)
            );
            let cached = project(
                "worker_service",
                &input.merge(Serialized::default(
                    "cache",
                    json!({
                        "enabled": true,
                        "host": "cache.internal", "port": 16380,
                    }),
                )),
            );
            assert_eq!(
                cached
                    .extract_inner::<String>("gateway_session_storage.type")
                    .unwrap(),
                "Redis"
            );
            assert_eq!(
                cached
                    .extract_inner::<String>("gateway_session_storage.config.host")
                    .unwrap(),
                "cache.internal"
            );
            assert_eq!(
                cached
                    .extract_inner::<u16>("gateway_session_storage.config.port")
                    .unwrap(),
                16380
            );
            assert_eq!(
                cached
                    .extract_inner::<humantime_serde::Serde<std::time::Duration>>(
                        "gateway_session_storage.config.pending_login_expiration"
                    )
                    .unwrap()
                    .into_inner(),
                std::time::Duration::from_secs(3600)
            );
        });
}

#[test]
fn postgres_and_s3_reach_every_store_without_changing_service_schemas() {
    let input = Figment::from(Toml::string(
        r#"
            state_store = "postgres"
            blob_store = "s3"
            [postgres]
            host = "state.internal"
            port = 25432
            database = "installation"
            username = "owner"
            password = ""
            [object_storage]
            endpoint_url = "https://objects.internal"
            components_bucket = "components"
            data_bucket = "agent-data"
            "#,
    ));
    [
        ("registry_service", "db", "golem_registry_service"),
        ("shard_manager", "persistence", "golem_shard_manager"),
        (
            "worker_executor",
            "indexed_storage",
            "golem_worker_executor_indexed",
        ),
        (
            "worker_executor",
            "key_value_storage",
            "golem_worker_executor",
        ),
        (
            "worker_executor",
            "scheduler_storage",
            "golem_worker_executor_scheduler",
        ),
        (
            "worker_service",
            "gateway_session_storage",
            "golem_worker_service_session_store",
        ),
    ]
    .into_iter()
    .for_each(|(service, path, schema)| {
        let config = project(service, &input);
        assert_eq!(
            config
                .extract_inner::<String>(&format!("{path}.type"))
                .unwrap(),
            "Postgres"
        );
        assert_eq!(
            config
                .extract_inner::<String>(&format!("{path}.config.host"))
                .unwrap(),
            "state.internal"
        );
        assert_eq!(
            config
                .extract_inner::<u16>(&format!("{path}.config.port"))
                .unwrap(),
            25432
        );
        assert_eq!(
            config
                .extract_inner::<String>(&format!("{path}.config.schema"))
                .unwrap(),
            schema
        );
        assert_eq!(
            config
                .extract_inner::<u32>(&format!("{path}.config.max_connections"))
                .unwrap(),
            10
        );
    });
    [
        "registry_service",
        "worker_service",
        "worker_executor",
        "component_compilation_service",
    ]
    .into_iter()
    .for_each(|service| {
        let config = project(service, &input);
        assert_eq!(
            config.extract_inner::<String>("blob_storage.type").unwrap(),
            "S3"
        );
        assert_eq!(
            config
                .extract_inner::<String>("blob_storage.config.aws_endpoint_url")
                .unwrap(),
            "https://objects.internal"
        );
        assert_eq!(
            config
                .extract_inner::<String>("blob_storage.config.components_bucket")
                .unwrap(),
            "components"
        );
        assert_eq!(
            config
                .extract_inner::<String>("blob_storage.config.oplog_payload_bucket")
                .unwrap(),
            "agent-data"
        );
    });
}

#[test]
fn advanced_settings_are_scoped_to_the_named_service() {
    let input = Figment::from(Toml::string(
        r#"
            [advanced.worker_executor]
            http_port = 18123
            [advanced.worker_service]
            port = 18124
            "#,
    ));
    let executor = project("worker_executor", &input);
    assert_eq!(executor.extract_inner::<u16>("http_port").unwrap(), 18123);
    assert!(executor.extract_inner::<u16>("port").is_err());
    let worker = project("worker_service", &input);
    assert_eq!(worker.extract_inner::<u16>("port").unwrap(), 18124);
    assert!(worker.extract_inner::<u16>("http_port").is_err());
}

#[test]
fn internal_tls_reaches_each_service_and_its_clients() {
    [true, false].into_iter().for_each(|enabled| {
        let input = Figment::from(Serialized::defaults(json!({
            "internal_grpc_tls": {
                "enabled": enabled,
                "cert_file": "/certs/service.pem",
                "key_file": "/certs/service.key",
                "ca_cert_file": "/certs/internal-ca.pem",
                "server_name": "services.internal",
            },
            "etcd": {"tls": {"ca_cert_file": "/certs/etcd-ca.pem"}},
        })));
        [
            ("registry_service", &["component_compilation.config"][..]),
            ("shard_manager", &["registry_service", "worker_executors"]),
            (
                "worker_service",
                &["registry_service", "worker_executor", "shard_manager"],
            ),
            (
                "worker_executor",
                &["registry_service", "shard_manager", "public_worker_api"],
            ),
            ("component_compilation_service", &["compile_worker"]),
        ]
        .into_iter()
        .for_each(|(service, clients)| {
            let config = project(service, &input).extract::<Value>().unwrap();
            let server = if enabled {
                json!({"type": "Enabled", "config": {
                    "server_cert_file": "/certs/service.pem",
                    "server_key_file": "/certs/service.key",
                    "client_ca_cert_file": "/certs/internal-ca.pem",
                }})
            } else {
                json!({"type": "Disabled", "config": {}})
            };
            assert_eq!(config.pointer("/grpc/tls"), Some(&server), "{service}");
            clients.iter().for_each(|client| {
                let tls = project(service, &input)
                    .extract_inner::<Value>(&format!("{client}.tls"))
                    .unwrap();
                let expected = if enabled {
                    json!({"type": "Enabled", "config": {
                        "client_cert_file": "/certs/service.pem",
                        "client_key_file": "/certs/service.key",
                        "server_ca_cert_file": "/certs/internal-ca.pem",
                        "server_domain_name": "services.internal",
                    }})
                } else {
                    json!({"type": "Disabled", "config": {}})
                };
                assert_eq!(tls, expected, "{service}: {client}");
            });
            assert_eq!(
                config
                    .pointer("/persistence/config/tls/ca_cert_file")
                    .and_then(Value::as_str),
                (service == "shard_manager").then_some("/certs/etcd-ca.pem"),
                "{service}"
            );
        });
    });

    let input = Figment::from(Serialized::defaults(json!({
        "internal_grpc_tls": {
            "enabled": true,
            "cert_file": "/certs/service.pem",
            "key_file": "/certs/service.key",
            "ca_cert_file": "/certs/internal-ca.pem",
            "server_name": "services.internal",
        },
        "advanced": {"worker_executor": {"grpc": {"tls": {
            "type": "Disabled", "config": {},
        }}}},
    })));
    assert_eq!(
        project("worker_executor", &input)
            .extract_inner::<Value>("grpc.tls")
            .unwrap(),
        json!({"type": "Disabled", "config": {}})
    );
}

#[test]
fn outbound_sockets_use_the_enabled_switch() {
    [true, false].into_iter().for_each(|enabled| {
        let input = reference(
            "worker_executor",
            &Figment::from(Serialized::defaults(json!({
                "agent_sockets": {"enabled": enabled},
                "filesystem_snapshots": {
                    "type": if enabled { "Enabled" } else { "Disabled" },
                    "config": {"repository_key": "configured-key"},
                },
            }))),
        );
        assert_eq!(
            input.extract_inner::<Value>("outbound_sockets").unwrap(),
            json!({"enabled": enabled})
        );
        assert_eq!(
            input
                .extract_inner::<Value>("filesystem_snapshots")
                .unwrap(),
            json!({"enabled": enabled, "repository_key": "configured-key"})
        );
        assert_eq!(
            project("worker_executor", &input)
                .extract_inner::<bool>("agent_sockets.enabled")
                .unwrap(),
            enabled
        );
        assert_eq!(
            all_in_one(&input)
                .unwrap()
                .extract_inner::<bool>("worker_executor.agent_sockets.enabled")
                .unwrap(),
            enabled
        );
    });
}

#[test]
fn field_only_snapshots_preserve_unspecified_enablement() {
    [
        ("Enabled", false),
        ("Disabled", false),
        ("Enabled", true),
        ("Disabled", true),
    ]
    .into_iter()
    .for_each(|(baseline, disabled)| {
        let mut snapshots = json!({"storage_call_deadline": "83s"});
        if disabled {
            snapshots["enabled"] = json!(false);
        }
        let input = Figment::from(Serialized::default("filesystem_snapshots", snapshots));
        [
            project("worker_executor", &input)
                .extract_inner::<Value>("filesystem_snapshots")
                .unwrap(),
            all_in_one(&input)
                .unwrap()
                .extract_inner::<Value>("worker_executor.filesystem_snapshots")
                .unwrap(),
        ]
        .into_iter()
        .for_each(|projected| {
            assert_eq!(
                projected.get("type").and_then(Value::as_str),
                disabled.then_some("Disabled")
            );
            assert_eq!(
                projected
                    .pointer("/config/storage_call_deadline")
                    .and_then(Value::as_str),
                (!disabled).then_some("83s")
            );
            let effective = Figment::from(Serialized::default(
                "filesystem_snapshots",
                json!({
                    "type": baseline, "config": {"storage_call_deadline": "60s"},
                }),
            ))
            .merge(Serialized::default("filesystem_snapshots", projected));
            assert_eq!(
                effective
                    .extract_inner::<String>("filesystem_snapshots.type")
                    .unwrap(),
                if disabled { "Disabled" } else { baseline }
            );
            if !disabled {
                assert_eq!(
                    effective
                        .extract_inner::<String>(
                            "filesystem_snapshots.config.storage_call_deadline"
                        )
                        .unwrap(),
                    "83s"
                );
            }
        });
    });
}

#[test]
fn snapshot_retention_follows_the_executor_variant() {
    [
            (json!({"filesystem_snapshots": {"enabled": true}}), Some(2)),
            (json!({"filesystem_snapshots": {"enabled": false}}), None),
            (json!({"retained_update_snapshots": 7}), Some(7)),
            (json!({"advanced": {"worker_executor": {"filesystem_snapshots": {"type": "Enabled"}}}}), Some(2)),
            (json!({"filesystem_snapshots": {"enabled": false}, "retained_update_snapshots": 7, "advanced": {"worker_executor": {"filesystem_snapshots": {"type": "Enabled"}}}}), Some(7)),
            (json!({"retained_update_snapshots": 2, "advanced": {"worker_executor": {"filesystem_snapshots": {"type": "Enabled", "config": {"retained_update_snapshots": 7}}}}}), Some(7)),
            (json!({"filesystem_snapshots": {"enabled": true}, "advanced": {"worker_executor": {"filesystem_snapshots": {"config": {"retained_update_snapshots": 7}}}}}), Some(7)),
        ]
        .into_iter()
        .for_each(|(value, retained)| {
            let input = Figment::from(Serialized::defaults(value));
            let executor = project("worker_executor", &input).extract::<Value>().unwrap();
            assert_eq!(
                executor.pointer("/filesystem_snapshots/config/retained_update_snapshots").and_then(Value::as_u64),
                retained
            );
            let worker = project("worker_service", &input).extract::<Value>().unwrap();
            assert!(worker.get("filesystem_snapshots").is_none());
        });
}

#[test]
fn server_advanced_inherits_only_settings_for_new_variants() {
    let lower = Figment::from(Serialized::defaults(json!({
        "retained_update_snapshots": 7,
        "etcd": {"tls": {"ca_cert_file": "/certs/inherited-etcd.pem"}},
        "advanced": {
            "shard_manager": {"persistence": {"type": "InMemory"}},
            "worker_executor": {"filesystem_snapshots": {"type": "Disabled"}},
        },
    })));
    let layer = Figment::from(Serialized::defaults(json!({
        "advanced": {
            "shard_manager": {"persistence": {"type": "Etcd"}},
            "worker_executor": {"filesystem_snapshots": {"type": "Enabled"}},
        },
    })));
    let projected = server_advanced(&layer, &lower).unwrap();
    assert_eq!(
        projected
            .extract_inner::<String>("shard_manager.persistence.config.tls.ca_cert_file")
            .unwrap(),
        "/certs/inherited-etcd.pem"
    );
    assert_eq!(
        projected
            .extract_inner::<u32>(
                "worker_executor.filesystem_snapshots.config.retained_update_snapshots"
            )
            .unwrap(),
        7
    );
    let repeated = server_advanced(&layer, &lower.clone().merge(&layer)).unwrap();
    assert!(
        repeated
            .extract_inner::<Value>("shard_manager.persistence.config.tls")
            .is_err()
    );
    [
            json!({"advanced": {"worker_executor": {"filesystem_snapshots": {"type": "Enabled", "config": {"retained_update_snapshots": 11}}}}}),
            json!({"filesystem_snapshots": {"enabled": true}, "advanced": {"worker_executor": {"filesystem_snapshots": {"config": {"retained_update_snapshots": 11}}}}}),
        ]
        .into_iter()
        .for_each(|settings| {
            let lower = Figment::from(Serialized::default("retained_update_snapshots", 7))
                .merge(Serialized::defaults(settings));
            let projected = all_in_one(&lower)
                .unwrap()
                .merge(server_advanced(&layer, &lower).unwrap());
            assert_eq!(
                projected
                    .extract_inner::<u32>(
                        "worker_executor.filesystem_snapshots.config.retained_update_snapshots"
                    )
                    .unwrap(),
                11
            );
        });
    let unchanged = server_advanced(&Figment::new(), &lower).unwrap();
    assert!(
        unchanged
            .extract_inner::<Value>("shard_manager.persistence.config.tls")
            .is_err()
    );
    assert!(
        unchanged
            .extract_inner::<Value>(
                "worker_executor.filesystem_snapshots.config.retained_update_snapshots"
            )
            .is_err()
    );
}
