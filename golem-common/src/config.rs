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

pub mod byte_size;
pub mod server_settings;

use crate::SafeDisplay;
use crate::model::RetryConfig;
use figment::Figment;
use figment::providers::{Env, Format, Serialized, Toml};
use figment::value::Value;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

const ENV_VAR_PREFIX: &str = "GOLEM__";
const ENV_VAR_NESTED_SEPARATOR: &str = "__";

/// What [`ConfigLoader::load_or_dump_config`] did.
#[derive(Debug)]
pub enum LoadOrDump<T> {
    /// The configuration loaded. The service starts with it.
    Loaded(T),
    /// A `--dump-config-*` flag printed the configuration. The process exits with code 0.
    Dumped,
}

/// A configuration that does not load, or a `--dump-config*` flag that is not known.
///
/// `Load` has the figment error in its message and not as its `source`, so `anyhow` and the
/// `Error:` line of a failed `main` print it once.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to load config: {0}")]
    Load(figment::Error),
    #[error(
        "Unknown dump config parameter: {0}. Use --dump-config-default, \
         --dump-config-default-env-var or --dump-config-default-toml"
    )]
    UnknownDumpFlag(String),
}

pub trait ConfigLoaderConfig: Default + Serialize + Deserialize<'static> + SafeDisplay {}
impl<T: Default + Serialize + Deserialize<'static> + SafeDisplay> ConfigLoaderConfig for T {}

pub type ConfigExample<T> = (&'static str, T);

pub trait HasConfigExamples<T> {
    fn examples() -> Vec<ConfigExample<T>>;
}

pub struct ConfigLoader<T: ConfigLoaderConfig> {
    pub config_file_name: PathBuf,
    pub make_examples: Option<fn() -> Vec<ConfigExample<T>>>,
    service: Option<&'static str>,
    config_type: std::marker::PhantomData<T>,
}

impl<T: ConfigLoaderConfig> ConfigLoader<T> {
    pub fn new(config_file_name: &Path) -> ConfigLoader<T> {
        let config_file_name = std::env::current_dir()
            .expect("Failed to get current directory")
            .join(config_file_name);
        Self {
            config_file_name,
            make_examples: None,
            service: None,
            config_type: std::marker::PhantomData,
        }
    }

    pub fn new_with_examples(config_file_name: &Path) -> ConfigLoader<T>
    where
        T: HasConfigExamples<T>,
    {
        let config_file_name = std::env::current_dir()
            .expect("Failed to get current directory")
            .join(config_file_name);
        Self {
            config_file_name,
            make_examples: Some(T::examples),
            service: None,
            config_type: std::marker::PhantomData,
        }
    }

    pub fn for_service(mut self, service: &'static str) -> Self {
        self.service = Some(service);
        self
    }

    pub fn default_figment(&self) -> Figment {
        Figment::from(Serialized::defaults(T::default()))
    }

    fn settings_defaults(&self) -> Figment {
        let defaults = self.default_figment();
        match self.service {
            Some(service) => server_settings::reference(service, &defaults),
            None => defaults,
        }
    }

    pub fn example_figments(&self) -> Vec<(&'static str, Figment)> {
        self.make_examples
            .map_or(Vec::new(), |make| make())
            .into_iter()
            .map(|(name, example)| {
                let value = match self.service {
                    Some(service) => Serialized::default(&format!("advanced.{service}"), example),
                    None => Serialized::defaults(example),
                };
                (name, Figment::from(value))
            })
            .collect()
    }

    pub fn figment(&self) -> Figment {
        let defaults = self.settings_defaults();
        let input = [
            Figment::from(Toml::file_exact(self.config_file_name.clone())),
            Figment::from(env_config_provider()),
        ]
        .into_iter()
        .fold(defaults.clone(), |lower, upper| {
            server_settings::merge_variants(lower, upper, &defaults)
        });
        let settings = match self.service {
            Some(service) => server_settings::project(service, &input),
            None => input,
        };
        let defaults = self.default_figment();
        server_settings::complete_variant_configs::<T>(server_settings::merge_variants(
            defaults.clone(),
            settings,
            &defaults,
        ))
    }

    pub fn load(&self) -> figment::Result<T> {
        self.figment().extract::<T>()
    }

    /// Reads the first command line argument. A `--dump-config-*` flag prints the default
    /// configuration and the examples and returns [`LoadOrDump::Dumped`] without loading
    /// anything. An unknown `--dump-config*` flag is an error. Otherwise it loads the
    /// configuration. It prints nothing on a failure: the caller returns the error, and the
    /// process exits with code 1.
    pub fn load_or_dump_config(&self) -> Result<LoadOrDump<T>, ConfigError> {
        let flag = std::env::args().nth(1).unwrap_or_default();
        self.load_or_dump_config_for_flag(&flag)
    }

    fn load_or_dump_config_for_flag(&self, flag: &str) -> Result<LoadOrDump<T>, ConfigError> {
        if dump_config(flag, &self.settings_defaults(), &self.example_figments())? {
            Ok(LoadOrDump::Dumped)
        } else {
            self.load()
                .map(LoadOrDump::Loaded)
                .map_err(ConfigError::Load)
        }
    }
}

/// Prints the default config and the examples in the format selected by a `--dump-config*`
/// flag: `--dump-config-default`, `--dump-config-default-env-var` or
/// `--dump-config-default-toml`. Returns `Ok(false)` when `flag` is not a dump flag, and
/// [`ConfigError::UnknownDumpFlag`] for an unknown `--dump-config*` flag, before it prints anything.
pub fn dump_config(
    flag: &str,
    default: &Figment,
    examples: &[(&str, Figment)],
) -> Result<bool, ConfigError> {
    let dump = match flag {
        "--dump-config-default" => dump::print(),
        "--dump-config-default-env-var" => dump::env_var(),
        "--dump-config-default-toml" => dump::toml(),
        other if other.starts_with("--dump-config") => {
            return Err(ConfigError::UnknownDumpFlag(other.to_string()));
        }
        _ => return Ok(false),
    };

    let extract =
        |figment: &Figment| -> Value { figment.extract().expect("Failed to extract config") };

    let dump_figment = |header: &str, is_example: bool, figment: &Figment| match &dump {
        dump::Dump::RootValue(dump) => dump.root_value(header, is_example, &extract(figment)),
        dump::Dump::Visitor(dump) => {
            dump.begin(header, is_example);
            visit_dump(
                figment,
                dump.as_ref(),
                &mut Vec::new(),
                "",
                &extract(figment),
            )
        }
    };

    dump_figment("Generated from default config", false, default);
    for (name, example) in examples {
        dump_figment(
            &format!("Generated from example config: {name}"),
            true,
            example,
        );
    }

    Ok(true)
}

fn visit_dump<'a>(
    figment: &Figment,
    dump: &dyn dump::VisitorDump,
    path: &mut Vec<&'a str>,
    name: &'a str,
    value: &'a Value,
) {
    match value {
        Value::Dict(_, dict) => {
            if !name.is_empty() {
                path.push(name);
            }

            for (name, value) in dict.iter().filter(|(_, v)| v.as_dict().is_none()) {
                visit_dump(figment, dump, path, name, value)
            }
            for (name, value) in dict.iter().filter(|(_, v)| v.as_dict().is_some()) {
                visit_dump(figment, dump, path, name, value)
            }

            if !name.is_empty() {
                path.pop();
            }
        }
        value => dump.value(path, name, figment.get_metadata(value.tag()), value),
    }
}

pub(crate) mod dump {
    use crate::config::{ENV_VAR_NESTED_SEPARATOR, ENV_VAR_PREFIX};
    use figment::Metadata;
    use figment::value::Value;

    pub fn print() -> Dump {
        Dump::Visitor(Box::new(Print))
    }

    pub fn env_var() -> Dump {
        Dump::Visitor(Box::new(EnvVar))
    }

    pub fn toml() -> Dump {
        Dump::RootValue(Box::new(Toml))
    }

    pub enum Dump {
        RootValue(Box<dyn RootValueDump>),
        Visitor(Box<dyn VisitorDump>),
    }

    pub trait RootValueDump {
        fn root_value(&self, header: &str, is_example: bool, value: &Value);
    }

    pub trait VisitorDump {
        fn begin(&self, header: &str, is_example: bool);
        fn value(&self, path: &[&str], name: &str, metadata: Option<&Metadata>, value: &Value);
    }

    struct Print;

    impl VisitorDump for Print {
        fn begin(&self, header: &str, is_example: bool) {
            if is_example {
                println!();
            }
            println!(":: {header}\n")
        }

        fn value(&self, path: &[&str], name: &str, _metadata: Option<&Metadata>, value: &Value) {
            if !path.is_empty() {
                for elem in path {
                    print!("{elem}.");
                }
            }
            if !name.is_empty() {
                print!("{name}");
            }

            print!(
                ": {}",
                serde_json::to_string(value).expect("Failed to pretty print value")
            );

            println!()
        }
    }

    struct EnvVar;

    impl VisitorDump for EnvVar {
        fn begin(&self, header: &str, is_example: bool) {
            if is_example {
                println!();
            }
            println!("### {header}\n")
        }

        fn value(&self, path: &[&str], name: &str, _metadata: Option<&Metadata>, value: &Value) {
            let is_empty_value = value.to_empty().is_some();

            if is_empty_value {
                print!("#");
            }

            let path = if path.first() == Some(&"advanced") {
                print!("GOLEM_ADVANCED__");
                &path[1..]
            } else {
                print!("{ENV_VAR_PREFIX}");
                path
            };
            path.iter().for_each(|elem| {
                print!("{}{}", elem.to_uppercase(), ENV_VAR_NESTED_SEPARATOR);
            });
            if !name.is_empty() {
                print!("{}", name.to_uppercase());
            }

            if !is_empty_value {
                println!(
                    "={}",
                    serde_json::to_string(value).expect("Failed to pretty print value")
                );
            } else {
                println!("=");
            }
        }
    }

    pub struct Toml;

    impl RootValueDump for Toml {
        fn root_value(&self, header: &str, is_example: bool, value: &Value) {
            if is_example {
                println!();
            }
            println!("## {header}");

            let mut config_as_toml_str =
                toml::to_string(value).expect("Failed to serialize as TOML");

            if is_example {
                config_as_toml_str = config_as_toml_str
                    .lines()
                    .map(|l| {
                        if l.is_empty() {
                            "#".to_string()
                        } else {
                            format!("# {l}")
                        }
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            }

            println!("{}", config_as_toml_str.trim_end_matches('\n'));
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub database: usize,
    pub tracing: bool,
    pub pool_size: usize,
    pub retries: RetryConfig,
    pub key_prefix: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub tls: bool,
}

impl RedisConfig {
    pub fn url(&self) -> Url {
        let scheme = if self.tls { "rediss" } else { "redis" };
        Url::parse(&format!(
            "{}://{}:{}/{}",
            scheme, self.host, self.port, self.database
        ))
        .expect("Failed to parse Redis URL")
    }
}

impl SafeDisplay for RedisConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "database: {}", self.database);
        let _ = writeln!(&mut result, "tracing: {}", self.tracing);
        let _ = writeln!(&mut result, "pool size: {}", self.pool_size);
        let _ = writeln!(&mut result, "tls: {}", self.tls);
        if !self.key_prefix.is_empty() {
            let _ = writeln!(&mut result, "key prefix: {}", self.key_prefix);
        }
        if self.username.is_some() {
            let _ = writeln!(&mut result, "username: ****");
        }
        if self.password.is_some() {
            let _ = writeln!(&mut result, "password: ****");
        }
        result
    }
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6380,
            database: 0,
            tracing: false,
            pool_size: 8,
            retries: RetryConfig::default(),
            key_prefix: "".to_string(),
            username: None,
            password: None,
            tls: false,
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::max_attempts_5()
    }
}

impl RetryConfig {
    pub fn max_attempts_3() -> RetryConfig {
        Self {
            max_attempts: 3,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(1),
            multiplier: 3.0,
            max_jitter_factor: Some(0.15),
        }
    }

    pub fn max_attempts_5() -> RetryConfig {
        Self {
            max_attempts: 5,
            min_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            multiplier: 2.0,
            max_jitter_factor: Some(0.15),
        }
    }

    pub fn no_retries() -> RetryConfig {
        Self {
            max_attempts: 0,
            min_delay: Duration::from_millis(0),
            max_delay: Duration::from_millis(0),
            multiplier: 1.0,
            max_jitter_factor: None,
        }
    }
}

pub fn env_config_provider() -> Figment {
    Figment::from(Env::prefixed(ENV_VAR_PREFIX).split(ENV_VAR_NESTED_SEPARATOR)).merge(
        Env::prefixed("GOLEM_ADVANCED__")
            .split(ENV_VAR_NESTED_SEPARATOR)
            .map(|key| format!("advanced.{key}").into()),
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum DbConfig {
    Postgres(DbPostgresConfig),
    Sqlite(DbSqliteConfig),
}

impl SafeDisplay for DbConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            DbConfig::Postgres(postgres) => {
                let _ = writeln!(&mut result, "postgres:");
                let _ = writeln!(&mut result, "{}", postgres.to_safe_string_indented());
            }
            DbConfig::Sqlite(sqlite) => {
                let _ = writeln!(&mut result, "sqlite:");
                let _ = writeln!(&mut result, "{}", sqlite.to_safe_string_indented());
            }
        }
        result
    }
}

impl Default for DbConfig {
    fn default() -> Self {
        DbConfig::Sqlite(DbSqliteConfig::default())
    }
}

impl DbConfig {
    pub fn postgres_example() -> Self {
        Self::Postgres(DbPostgresConfig {
            host: "localhost".to_string(),
            database: "postgres".to_string(),
            username: "postgres".to_string(),
            password: "postgres".to_string(),
            port: 5432,
            max_connections: 10,
            schema: None,
            acquire_timeout: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DbSqliteConfig {
    pub database: String,
    pub max_connections: u32,
    pub foreign_keys: bool,
}

impl DbSqliteConfig {
    pub fn connect_options(&self) -> sqlx::sqlite::SqliteConnectOptions {
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&self.database)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .create_if_missing(true)
            .foreign_keys(self.foreign_keys)
    }
}

impl SafeDisplay for DbSqliteConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "database: {}", self.database);
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        result
    }
}

impl Default for DbSqliteConfig {
    fn default() -> Self {
        Self {
            database: "golem_service.db".to_string(),
            max_connections: 10,
            foreign_keys: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DbPostgresConfig {
    pub host: String,
    pub database: String,
    pub username: String,
    pub password: String,
    pub port: u16,
    pub max_connections: u32,
    pub schema: Option<String>,
    /// How long a caller waits for a free pooled connection before giving up.
    ///
    /// `None` leaves sqlx's own default (30s) in place. Deliberately not shortened here: this
    /// struct is shared by every Postgres pool in the workspace, and a pool whose caller does not
    /// retry is better off waiting out a load spike than failing N times sooner.
    ///
    /// Shortening it is worthwhile only where something above the pool retries, because a retry
    /// loop cannot make progress while a single attempt is still parked - see the key-value
    /// storage pool, which picks its own shorter fallback for exactly that reason.
    #[serde(default, with = "humantime_serde")]
    pub acquire_timeout: Option<Duration>,
}

impl DbPostgresConfig {
    pub fn connect_options(&self) -> sqlx::postgres::PgConnectOptions {
        sqlx::postgres::PgConnectOptions::new()
            .host(&self.host)
            .port(self.port)
            .database(&self.database)
            .username(&self.username)
            .password(&self.password)
    }
}

impl SafeDisplay for DbPostgresConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        let _ = writeln!(&mut result, "host: {}", self.host);
        let _ = writeln!(&mut result, "port: {}", self.port);
        let _ = writeln!(&mut result, "database: {}", self.database);
        let _ = writeln!(&mut result, "username: ****");
        let _ = writeln!(&mut result, "password: ****");
        let _ = writeln!(&mut result, "max connections: {}", self.max_connections);
        if let Some(schema) = &self.schema {
            let _ = writeln!(&mut result, "schema: {schema}");
        }
        result
    }
}

#[cfg(test)]
mod tests {
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
            let loader =
                ConfigLoader::<LoaderTestConfig>::new(std::path::Path::new("settings.toml"))
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
            let Persistence::Postgres(same) = same_variant.extract::<Settings>()?.persistence
            else {
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
                        let Persistence::Sqlite(sqlite) =
                            figment.extract::<Settings>()?.persistence
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
}
