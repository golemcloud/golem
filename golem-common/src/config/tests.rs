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

use super::{ConfigError, ConfigLoader, LoadOrDump, dump_config, server_settings};
use crate::SafeDisplay;
use figment::Figment;
use figment::providers::{Format, Toml};
use serde::{Deserialize, Serialize};
use test_r::test;

#[derive(Debug, Default, Serialize, Deserialize)]
struct LoaderTestConfig {
    loader_test_port: u16,
}

impl SafeDisplay for LoaderTestConfig {
    fn to_safe_string(&self) -> String {
        String::new()
    }
}

fn loader_with_file(contents: &str) -> (tempfile::TempDir, ConfigLoader<LoaderTestConfig>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loader-test.toml");
    std::fs::write(&path, contents).unwrap();
    (dir, ConfigLoader::new(&path))
}

#[test]
fn invalid_value_is_a_load_error() {
    let (_dir, loader) = loader_with_file("loader_test_port = \"notanumber\"\n");
    let err = loader.load_or_dump_config_for_flag("").unwrap_err();
    assert!(matches!(err, ConfigError::Load(_)), "got {err:?}");

    let printed = format!("{:?}", anyhow::Error::from(err));
    assert_eq!(printed.matches("Failed to load config").count(), 1);
    assert_eq!(printed.matches("notanumber").count(), 1);
}

#[test]
fn valid_file_loads() {
    let (_dir, loader) = loader_with_file("loader_test_port = 1234\n");
    for flag in ["", "--report-time"] {
        match loader.load_or_dump_config_for_flag(flag) {
            Ok(LoadOrDump::Loaded(config)) => assert_eq!(config.loader_test_port, 1234),
            other => panic!("flag {flag:?}: expected Loaded, got {other:?}"),
        }
    }
}

#[test]
fn server_settings_invalid_store_is_a_load_error() {
    [
            (
                "state_store = 123",
                "state_store must be sqlite or postgres",
            ),
            ("blob_store = false", "blob_store must be data_disk or s3"),
            (
                "state_store = 'redis'",
                "state_store must be sqlite or postgres",
            ),
            (
                "blob_store = 'memory'",
                "blob_store must be data_disk or s3",
            ),
            (
                "cache.enabled = 'Redis'",
                "cache.enabled must be true or false",
            ),
            ("cache.enabled = 1", "cache.enabled must be true or false"),
            (
                "filesystem_snapshots.enabled = 'Enabled'",
                "filesystem_snapshots.enabled must be true or false",
            ),
            (
                "outbound_sockets.enabled = 'off'",
                "outbound_sockets.enabled must be true or false",
            ),
            (
                "internal_grpc_tls.enabled = 'on'",
                "internal_grpc_tls.enabled must be true or false",
            ),
            (
                "internal_grpc_tls = true",
                "internal_grpc_tls must be a table",
            ),
            ("data_dir = false", "data_dir must be a string"),
            ("cache = true", "cache must be a table"),
            ("cache.retries = false", "cache.retries must be a table"),
            (
                "filesystem_snapshots = true",
                "filesystem_snapshots must be a table",
            ),
            (
                "filesystem_snapshots.enabled = false\nfilesystem_snapshots.storage_retry = false",
                "filesystem_snapshots.storage_retry must be a table",
            ),
            ("outbound_sockets = false", "outbound_sockets must be a table"),
            ("postgres = 5432", "postgres must be a table"),
            ("data_disk = '/state'", "data_disk must be a table"),
            ("object_storage = 's3'", "object_storage must be a table"),
            (
                "object_storage.credentials = 'credentials'",
                "object_storage.credentials must be a table",
            ),
            ("logging = true", "logging must be a table"),
            ("logging.stdout = true", "logging.stdout must be a table"),
            (
                "logging.stdout = [true, false, false, false, false, false, false, false, false, false, false]",
                "logging.stdout must be a table",
            ),
            ("logging.stderr = false", "logging.stderr must be a table"),
            ("logging.file = '/logs'", "logging.file must be a table"),
            (
                "logging.otlp = false\notlp.enabled = false",
                "logging.otlp must be a table",
            ),
            ("otlp = false", "otlp must be a table"),
            ("etcd = 'etcd.internal'", "etcd must be a table"),
            ("etcd.tls = true", "etcd.tls must be a table"),
            ("advanced = true", "advanced must be a table"),
            (
                "advanced.worker_executor = false",
                "advanced.worker_executor must be a table",
            ),
            ("otlp.enabled = 1", "otlp.enabled must be true or false"),
            (
                "number_of_shards = 17",
                "number_of_shards is not a server setting",
            ),
            (
                "retained_periodic_snapshots = 3",
                "retained_periodic_snapshots is not a server setting",
            ),
            (
                "[filesystem_snapshots]\nretained_periodic_snapshots = 3",
                "retained_periodic_snapshots is not a server setting",
            ),
        ]
        .into_iter()
        .for_each(|(contents, accepted)| {
            let (_dir, loader) = loader_with_file(contents);
            [
                loader
                    .for_service("worker_executor")
                    .load()
                    .unwrap_err()
                    .to_string(),
                server_settings::all_in_one(&Figment::from(Toml::string(contents)))
                    .unwrap_err()
                    .to_string(),
            ]
            .into_iter()
            .for_each(|error| assert!(error.contains(accepted), "{error}"));
        });
}

#[test]
fn server_settings_defaults_and_environment_choose_the_storage_root() {
    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        jail.create_file("settings.toml", "")?;
        let loader = ConfigLoader::<LoaderTestConfig>::new(std::path::Path::new("settings.toml"))
            .for_service("worker_executor");
        assert_eq!(
            loader
                .figment()
                .extract_inner::<String>("key_value_storage.config.root_dir")?,
            "/var/lib/golem/kv-store"
        );
        jail.create_file("settings.toml", "data_dir = '/file-state'")?;
        assert_eq!(
            loader
                .figment()
                .extract_inner::<String>("key_value_storage.config.root_dir")?,
            "/file-state/kv-store"
        );
        jail.set_env("GOLEM__DATA_DIR", "/environment-state");
        assert_eq!(
            loader
                .figment()
                .extract_inner::<String>("key_value_storage.config.root_dir")?,
            "/environment-state/kv-store"
        );
        Ok(())
    });
}

#[test]
fn server_settings_advanced_environment_is_scoped_and_keeps_its_path() {
    figment::Jail::expect_with(|jail| {
        jail.create_file(
            "settings.toml",
            "[advanced.worker_executor]\nloader_test_port = 1234",
        )?;
        jail.set_env("GOLEM_ADVANCED__WORKER_EXECUTOR__LOADER_TEST_PORT", "4567");
        let loader =
            || ConfigLoader::<LoaderTestConfig>::new(std::path::Path::new("settings.toml"));
        assert_eq!(
            loader()
                .for_service("worker_executor")
                .load()?
                .loader_test_port,
            4567
        );
        assert_eq!(
            loader()
                .for_service("worker_service")
                .load()?
                .loader_test_port,
            0
        );
        Ok(())
    });
}

#[test]
fn variant_layers_preserve_required_and_fieldless_config_contracts() {
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", content = "config")]
    enum Persistence {
        Sqlite(super::DbSqliteConfig),
        Postgres(super::DbPostgresConfig),
        Nested(super::DbConfig),
        Empty(golem_schema::Empty),
        Disabled,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct Settings {
        persistence: Persistence,
    }

    impl Default for Settings {
        fn default() -> Self {
            Self {
                persistence: Persistence::Sqlite(super::DbSqliteConfig {
                    database: "native-state.db".into(),
                    max_connections: 11,
                    foreign_keys: true,
                }),
            }
        }
    }

    impl SafeDisplay for Settings {
        fn to_safe_string(&self) -> String {
            String::new()
        }
    }

    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let loader = || ConfigLoader::<Settings>::new(std::path::Path::new("settings.toml"));
        jail.create_file("settings.toml", "")?;
        let Persistence::Sqlite(native) = loader().load()?.persistence else {
            panic!("expected native SQLite defaults")
        };
        assert_eq!(native.database, "native-state.db");
        assert_eq!(native.max_connections, 11);

        let postgres = "host = 'state.internal'\nport = 25432\ndatabase = 'tenant_state'\nusername = 'owner'\npassword = ''\nmax_connections = 43\n";
        let native_postgres =
            format!("persistence.type = 'Postgres'\n[persistence.config]\n{postgres}");
        jail.create_file("settings.toml", &native_postgres)?;
        jail.set_env("GOLEM__PERSISTENCE__CONFIG__DATABASE", "selected_postgres");
        let same_variant = loader().figment();
        assert!(
            same_variant
                .find_value("persistence.config.foreign_keys")
                .is_err()
        );
        let Persistence::Postgres(same) = same_variant.extract::<Settings>()?.persistence else {
            panic!("expected PostgreSQL with an upper database override")
        };
        assert_eq!(same.database, "selected_postgres");
        assert_eq!(same.host, "state.internal");
        assert_eq!(same.max_connections, 43);

        [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .try_for_each(
                |(supported_postgres, explicit_database)| -> figment::Result<()> {
                    jail.clear_env();
                    jail.create_file(
                        "settings.toml",
                        &if supported_postgres {
                            format!("state_store = 'postgres'\n[postgres]\n{postgres}")
                        } else {
                            native_postgres.clone()
                        },
                    )?;
                    let prefix = if supported_postgres {
                        "GOLEM_ADVANCED__SHARD_MANAGER__PERSISTENCE"
                    } else {
                        "GOLEM__PERSISTENCE"
                    };
                    jail.set_env(format!("{prefix}__TYPE"), "Sqlite");
                    if explicit_database {
                        jail.set_env(format!("{prefix}__CONFIG__DATABASE"), "selected.db");
                    }
                    let loader = if supported_postgres {
                        loader().for_service("shard_manager")
                    } else {
                        loader()
                    };
                    let figment = loader.figment();
                    assert!(figment.find_value("persistence.config.host").is_err());
                    let Persistence::Sqlite(sqlite) = figment.extract::<Settings>()?.persistence
                    else {
                        panic!("expected restored SQLite defaults")
                    };
                    assert_eq!(
                        sqlite.database,
                        if explicit_database {
                            "selected.db"
                        } else {
                            "native-state.db"
                        }
                    );
                    assert_eq!(sqlite.max_connections, 11);
                    assert!(sqlite.foreign_keys);
                    Ok(())
                },
            )?;

        [
            (false, "Empty"),
            (true, "Empty"),
            (false, "Disabled"),
            (true, "Disabled"),
        ]
        .into_iter()
        .try_for_each(|(from_env, variant)| -> figment::Result<()> {
            jail.clear_env();
            if from_env {
                jail.create_file("settings.toml", &native_postgres)?;
                jail.set_env("GOLEM__PERSISTENCE__TYPE", variant);
            } else {
                jail.create_file("settings.toml", &format!("persistence.type = '{variant}'"))?;
            }
            let figment = loader().figment();
            let selected = figment.extract::<Settings>()?;
            assert!(matches!(
                (variant, selected.persistence),
                ("Empty", Persistence::Empty(_)) | ("Disabled", Persistence::Disabled)
            ));
            assert_eq!(
                figment.find_value("persistence.config").is_ok(),
                variant == "Empty"
            );
            Ok(())
        })?;

        [
            ("persistence.type = 'Postgres'", "host"),
            (
                "persistence.type = 'Postgres'\npersistence.config.host = 'state.internal'",
                "database",
            ),
            (
                "persistence.type = 'Nested'\npersistence.config.type = 'Postgres'",
                "config",
            ),
        ]
        .into_iter()
        .try_for_each(|(contents, missing)| -> figment::Result<()> {
            jail.clear_env();
            jail.create_file("settings.toml", contents)?;
            let diagnostic = loader().load().unwrap_err().to_string();
            assert!(diagnostic.contains(missing), "{diagnostic}");
            assert!(diagnostic.contains("persistence"), "{diagnostic}");
            assert!(diagnostic.contains("settings.toml"), "{diagnostic}");
            Ok(())
        })?;

        jail.clear_env();
        jail.create_file(
                "root.toml",
                "type = 'Postgres'\n[config]\nhost = 'state.internal'\nport = 25432\nusername = 'owner'\npassword = ''\n",
            )?;
        let error = ConfigLoader::<super::DbConfig>::new(std::path::Path::new("root.toml"))
            .load()
            .unwrap_err();
        assert!(matches!(
            error.kind,
            figment::error::Kind::MissingField(ref field) if &**field == "database"
        ));
        Ok(())
    });
}

#[test]
fn server_settings_preserve_typed_error_origins_and_advanced_precedence() {
    #[derive(Debug, Default, Serialize, Deserialize)]
    struct Settings {
        tracing: crate::tracing::TracingConfig,
        issuers: Vec<crate::model::golem_issuer::GolemIssuer>,
    }

    impl SafeDisplay for Settings {
        fn to_safe_string(&self) -> String {
            String::new()
        }
    }

    figment::Jail::expect_with(|jail| {
        jail.clear_env();
        let loader = ConfigLoader::<Settings>::new(std::path::Path::new("settings.toml"))
            .for_service("worker_executor");
        let secrets = r#"
                [postgres]
                password = "postgres-secret-sentinel"
                [object_storage.credentials]
                secret_access_key = "s3-secret-sentinel"
                [advanced.worker_executor.filesystem_snapshots.config]
                repository_key = "repository-secret-sentinel"
            "#;
        jail.create_file(
            "settings.toml",
            &format!("[logging.stdout]\nenabled = 'not-a-boolean'\n{secrets}"),
        )?;
        jail.set_env("GOLEM__LOGGING__STDERR__ENABLED", "true");
        let error = loader.load().unwrap_err();
        assert_eq!(error.path, ["tracing", "stdout", "enabled"]);
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains("logging.stdout.enabled"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("settings.toml"), "{diagnostic}");
        assert!(!diagnostic.contains("environment variable"), "{diagnostic}");
        [
            "postgres-secret-sentinel",
            "s3-secret-sentinel",
            "repository-secret-sentinel",
        ]
        .into_iter()
        .for_each(|secret| assert!(!diagnostic.contains(secret), "{diagnostic}"));

        jail.set_env("GOLEM__LOGGING__STDOUT__ENABLED", "true");
        assert!(loader.load()?.tracing.stdout.enabled);
        jail.set_env("GOLEM__LOGGING__STDOUT__ENABLED", "not-a-boolean");
        let diagnostic = loader.load().unwrap_err().to_string();
        assert!(
            diagnostic.contains("LOGGING.STDOUT.ENABLED"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("environment variable"), "{diagnostic}");
        assert!(!diagnostic.contains("settings.toml"), "{diagnostic}");

        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__TRACING__STDOUT__ENABLED",
            "true",
        );
        let config = loader.load()?;
        assert!(config.tracing.stdout.enabled);
        assert!(config.tracing.stderr.enabled);
        assert!(config.tracing.stdout.json);

        jail.set_env(
            "GOLEM_ADVANCED__WORKER_EXECUTOR__ISSUERS",
            r#"["not-an-issuer"]"#,
        );
        let error = loader.load().unwrap_err();
        assert_eq!(error.path, ["issuers", "0"]);
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains("ADVANCED.WORKER_EXECUTOR.ISSUERS.0"),
            "{diagnostic}"
        );
        assert!(diagnostic.contains("environment variable"), "{diagnostic}");
        Ok(())
    });
}

#[test]
fn missing_file_is_a_load_error() {
    let dir = tempfile::tempdir().unwrap();
    let loader = ConfigLoader::<LoaderTestConfig>::new(&dir.path().join("does-not-exist.toml"));
    let err = loader.load_or_dump_config_for_flag("").unwrap_err();
    assert!(matches!(err, ConfigError::Load(_)), "got {err:?}");
}

#[test]
fn malformed_file_is_a_load_error() {
    let (_dir, loader) = loader_with_file("loader_test_port = = 1\n");
    let err = loader.load_or_dump_config_for_flag("").unwrap_err();
    assert!(matches!(err, ConfigError::Load(_)), "got {err:?}");
}

#[test]
fn dump_flag_dumps_without_loading() {
    let (_dir, loader) = loader_with_file("loader_test_port = \"notanumber\"\n");
    for flag in [
        "--dump-config-default",
        "--dump-config-default-env-var",
        "--dump-config-default-toml",
    ] {
        match loader.load_or_dump_config_for_flag(flag) {
            Ok(LoadOrDump::Dumped) => {}
            other => panic!("flag {flag}: expected Dumped, got {other:?}"),
        }
    }
}

#[test]
fn unknown_dump_flag_is_an_error() {
    let err = dump_config("--dump-config-bogus", &Figment::new(), &[]).unwrap_err();
    match &err {
        ConfigError::UnknownDumpFlag(flag) => assert_eq!(flag, "--dump-config-bogus"),
        other => panic!("expected UnknownDumpFlag, got {other:?}"),
    }
    let message = err.to_string();
    assert!(message.contains("--dump-config-bogus"), "{message}");
    assert!(message.contains("--dump-config-default,"), "{message}");
    assert!(
        message.contains("--dump-config-default-env-var"),
        "{message}"
    );
    assert!(message.contains("--dump-config-default-toml"), "{message}");

    let (_dir, loader) = loader_with_file("loader_test_port = 1234\n");
    let err = loader
        .load_or_dump_config_for_flag("--dump-config-bogus")
        .unwrap_err();
    assert!(
        matches!(err, ConfigError::UnknownDumpFlag(_)),
        "got {err:?}"
    );
}
