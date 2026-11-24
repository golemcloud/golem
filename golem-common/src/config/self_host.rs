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

use figment::providers::Serialized;
use figment::value::{Dict, Map};
use figment::{Figment, Metadata, Profile, Provider};
use serde_json::{Value, json};

/// Settings retained for recovery records and package downloads.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct InstallationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_location: Option<Box<str>>,
    #[serde(
        with = "humantime_serde::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub recovery_horizon: Option<std::time::Duration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_mirror_url: Option<url::Url>,
}

/// The service names used by the advanced channel.
pub const SERVICES: [&str; 5] = [
    "registry_service",
    "shard_manager",
    "worker_service",
    "worker_executor",
    "component_compilation_service",
];

/// Projects the supported settings and the named advanced section onto one service.
pub fn project(service: &str, input: &Figment) -> Figment {
    Figment::from(ServiceSettings { service, input })
}

/// Projects the common settings onto the embedded services and retains launcher settings.
pub fn server(input: &Figment) -> figment::Result<Figment> {
    let mut root: Value = input.extract()?;
    SERVICES
        .into_iter()
        .chain(["router", "golem_issuer", "ports_file"])
        .try_for_each(|key| {
            if root.get(key).is_some() {
                Err(figment::Error::from(format!(
                    "use the advanced channel for unsupported setting {key}"
                )))
            } else {
                Ok(())
            }
        })?;
    let mut service_input = root.clone();
    if let Some(config) = service_input.as_object_mut() {
        config.remove("logging");
        config.remove("otlp");
    }
    let service_input = Figment::from(Serialized::defaults(service_input));
    let projected = SERVICES
        .into_iter()
        .try_fold(Figment::new(), |projected, service| {
            let config: Value = project(service, &service_input).extract()?;
            Ok::<_, figment::Error>(projected.merge(Serialized::default(service, config)))
        })?;
    let logging = Figment::from(Serialized::defaults(
        root.get("logging").cloned().unwrap_or_else(|| json!({})),
    ));
    let logging = match root.get("otlp") {
        Some(otlp) => logging.merge(Serialized::default("otlp", otlp)),
        None => logging,
    };
    let server = root
        .pointer("/advanced/server")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(root) = root.as_object_mut() {
        [
            "state_store",
            "blob_store",
            "postgres",
            "data_disk",
            "object_storage",
            "cache",
            "logging",
            "otlp",
            "recovery_location",
            "recovery_horizon",
            "package_mirror_url",
            "internal_grpc_tls",
            "etcd",
            "outbound_sockets",
            "filesystem_snapshots",
            "retained_update_snapshots",
            "advanced",
        ]
        .into_iter()
        .for_each(|key| {
            root.remove(key);
        });
    }
    Ok(projected
        .merge(Serialized::default("logging", logging.extract::<Value>()?))
        .merge(Serialized::defaults(root))
        .merge(Serialized::defaults(server)))
}

/// Projects one advanced layer and retains supported settings for the selected variants.
pub fn server_advanced(input: &Figment, lower: &Figment) -> figment::Result<Figment> {
    let value: Value = input.extract()?;
    let lower: Value = lower.extract()?;
    let persistence = "/advanced/shard_manager/persistence/type";
    let snapshots = "/advanced/worker_executor/filesystem_snapshots/type";
    let inherited = [
        (
            "etcd",
            value.pointer(persistence).and_then(Value::as_str) == Some("Etcd")
                && value.pointer(persistence) != lower.pointer(persistence),
        ),
        (
            "retained_update_snapshots",
            value.pointer(snapshots).and_then(Value::as_str) == Some("Enabled"),
        ),
    ]
    .into_iter()
    .filter_map(|(key, needed)| lower.get(key).filter(|_| needed).map(|value| (key, value)))
    .fold(Figment::new(), |inherited, (key, value)| {
        inherited.merge(Serialized::default(key, value))
    });
    server(&inherited.merge(input))
}

/// Gives the common defaults and puts unsupported defaults in the named advanced section.
pub fn reference(service: &str, input: &Figment) -> Figment {
    let mut advanced = without_nulls(input.extract().expect("service defaults must serialize"));
    if service == "server" {
        return server_reference(advanced);
    }
    let mut common = json!({
        "state_store": "sqlite",
        "blob_store": "data_disk",
        "data_dir": "/var/lib/golem",
        "cache": {"type": "Disabled", "config": {}},
        "postgres": postgres_defaults(),
        "object_storage": {
            "region": "us-east-1", "object_prefix": "",
            "components_bucket": "golem-components", "artifacts_bucket": "golem-artifacts",
            "data_bucket": "golem-data",
        },
        "internal_grpc_tls": {"enabled": false},
        "retained_update_snapshots": 2,
        "outbound_sockets": false,
    });
    if let Some(config) = advanced.as_object_mut() {
        common["logging"] = config.remove("tracing").unwrap_or_else(|| json!({}));
        if let Some(logging) = common["logging"].as_object_mut() {
            let otlp = logging.remove("otlp");
            common["otlp"] = otlp.unwrap_or_else(|| json!({}));
        }
        config.remove("installation");
        match service {
            "registry_service" => {
                config.remove("db");
            }
            "shard_manager" => {
                config.remove("persistence");
            }
            "worker_service" => {
                config.remove("gateway_session_storage");
            }
            "worker_executor" => {
                ["key_value_storage", "indexed_storage", "scheduler_storage"]
                    .into_iter()
                    .for_each(|key| {
                        config.remove(key);
                    });
                if let Some(sockets) = config
                    .get_mut("agent_sockets")
                    .and_then(Value::as_object_mut)
                {
                    common["outbound_sockets"] =
                        sockets.remove("enabled").unwrap_or(Value::Bool(false));
                }
                common["filesystem_snapshots"] = config
                    .remove("filesystem_snapshots")
                    .unwrap_or_else(|| json!({"type": "Disabled", "config": {}}));
            }
            _ => {}
        }
        config.remove("blob_storage");
    }
    std::iter::once("grpc")
        .chain(grpc_clients(service).iter().copied())
        .for_each(|path| {
            let pointer = format!("/{}", path.replace('.', "/"));
            if let Some(config) = advanced
                .pointer_mut(&pointer)
                .and_then(Value::as_object_mut)
            {
                config.remove("tls");
            }
        });
    common["advanced"] = json!({ service: advanced });
    Figment::from(Serialized::defaults(InstallationConfig::default()))
        .merge(Serialized::defaults(common))
}

fn without_nulls(value: Value) -> Value {
    match value {
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, without_nulls(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(without_nulls).collect()),
        value => value,
    }
}

fn server_reference(mut root: Value) -> Figment {
    let postgres = root.pointer("/registry_service/db").cloned();
    let blob = root.pointer("/registry_service/blob_storage").cloned();
    let reference = SERVICES
        .into_iter()
        .fold(Figment::new(), |config, service| {
            let service_config = root
                .as_object_mut()
                .and_then(|root| root.remove(service))
                .unwrap_or_else(|| json!({}));
            let value: Value = reference(
                service,
                &Figment::from(Serialized::defaults(service_config)),
            )
            .extract()
            .expect("reference settings must serialize");
            config.merge(Serialized::defaults(value))
        });
    let reference = match postgres
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("Postgres"))
    {
        Some(postgres) => reference
            .merge(Serialized::default("state_store", "postgres"))
            .merge(Serialized::default("postgres", &postgres["config"])),
        None => reference,
    };
    let reference = match blob
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("S3"))
    {
        Some(blob) => {
            let config = &blob["config"];
            [
                ("region", "region"),
                ("aws_endpoint_url", "endpoint_url"),
                ("public_endpoint_url", "public_endpoint_url"),
                ("aws_path_style", "path_style"),
                ("components_bucket", "components_bucket"),
                ("artifacts_bucket", "artifacts_bucket"),
                ("custom_data_bucket", "data_bucket"),
                ("object_prefix", "object_prefix"),
            ]
            .into_iter()
            .filter_map(|(from, to)| {
                config
                    .get(from)
                    .filter(|value| !value.is_null())
                    .map(|value| (to, value))
            })
            .fold(
                reference.merge(Serialized::default("blob_store", "s3")),
                |reference, (key, value)| {
                    reference.merge(Serialized::default(&format!("object_storage.{key}"), value))
                },
            )
        }
        None => reference,
    };
    let data_dir = root
        .as_object_mut()
        .and_then(|root| root.remove("data_dir"));
    let logging = root.as_object_mut().and_then(|root| root.remove("logging"));
    let reference = reference.merge(Serialized::default("advanced.server", root));
    let reference = match data_dir {
        Some(data_dir) => reference.merge(Serialized::default("data_dir", data_dir)),
        None => reference,
    };
    match logging {
        Some(mut logging) => {
            let otlp = logging
                .as_object_mut()
                .and_then(|logging| logging.remove("otlp"));
            let reference = reference.merge(Serialized::default("logging", logging));
            match otlp {
                Some(otlp) => reference.merge(Serialized::default("otlp", otlp)),
                None => reference,
            }
        }
        None => reference,
    }
}

fn grpc_clients(service: &str) -> &'static [&'static str] {
    match service {
        "registry_service" => &["component_compilation.config"],
        "shard_manager" => &["registry_service", "worker_executors"],
        "worker_service" => &["registry_service", "worker_executor", "shard_manager"],
        "worker_executor" => &["registry_service", "shard_manager", "public_worker_api"],
        "component_compilation_service" => &["compile_worker"],
        _ => &[],
    }
}

struct ServiceSettings<'a> {
    service: &'a str,
    input: &'a Figment,
}

impl Provider for ServiceSettings<'_> {
    fn metadata(&self) -> Metadata {
        Metadata::named("self-host settings")
    }

    fn data(&self) -> figment::Result<Map<Profile, Dict>> {
        let value: Value = self.input.extract()?;
        [
            ("state_store", ["sqlite", "postgres"]),
            ("blob_store", ["data_disk", "s3"]),
        ]
        .into_iter()
        .try_for_each(|(key, allowed)| match value.get(key) {
            Some(value) if !value.as_str().is_some_and(|value| allowed.contains(&value)) => Err(
                figment::Error::from(format!("{key} must be {} or {}", allowed[0], allowed[1])),
            ),
            _ => Ok(()),
        })?;
        if let Some(cache) = value.pointer("/cache/type")
            && !cache
                .as_str()
                .is_some_and(|kind| ["Disabled", "Redis"].contains(&kind))
        {
            return Err(figment::Error::from("cache.type must be Disabled or Redis"));
        }
        ["number_of_shards", "retained_periodic_snapshots"]
            .into_iter()
            .try_for_each(|key| {
                if value.get(key).is_some() {
                    Err(figment::Error::from(format!(
                        "{key} is not a self-host setting"
                    )))
                } else {
                    Ok(())
                }
            })?;
        if value
            .pointer("/filesystem_snapshots/config/retained_periodic_snapshots")
            .is_some()
        {
            return Err(figment::Error::from(
                "retained_periodic_snapshots is not a self-host setting",
            ));
        }
        let installation: InstallationConfig = self.input.extract()?;
        let projected =
            storage(self.service, &value)?.merge(Serialized::default("installation", installation));
        let projected = [("logging", "tracing"), ("otlp", "tracing.otlp")]
            .into_iter()
            .fold(projected, |projected, (from, to)| match value.get(from) {
                Some(value) => projected.merge(Serialized::default(to, value)),
                None => projected,
            });
        let projected = if self.service == "worker_executor" {
            [
                ("outbound_sockets", "agent_sockets.enabled"),
                ("filesystem_snapshots", "filesystem_snapshots"),
            ]
            .into_iter()
            .fold(projected, |projected, (from, to)| match value.get(from) {
                Some(value) => projected.merge(Serialized::default(to, value)),
                None => projected,
            })
        } else {
            projected
        };
        let projected = transport(self.service, &value, projected)?.merge(Serialized::defaults(
            value
                .pointer(&format!("/advanced/{}", self.service))
                .cloned()
                .unwrap_or_else(|| json!({})),
        ));
        let projected = if self.service == "worker_executor"
            && (value.get("retained_update_snapshots").is_some()
                || projected
                    .extract_inner::<String>("filesystem_snapshots.type")
                    .as_deref()
                    == Ok("Enabled"))
        {
            projected.merge(Serialized::default(
                "filesystem_snapshots.config.retained_update_snapshots",
                value
                    .get("retained_update_snapshots")
                    .cloned()
                    .unwrap_or_else(|| json!(2)),
            ))
        } else {
            projected
        };
        Serialized::defaults(projected.extract::<Value>()?).data()
    }
}

fn transport(service: &str, input: &Value, projected: Figment) -> figment::Result<Figment> {
    let projected = match (service, input.pointer("/etcd/tls")) {
        ("shard_manager", Some(tls)) => {
            projected.merge(Serialized::default("persistence.config.tls", tls))
        }
        _ => projected,
    };
    let Some(tls) = input.get("internal_grpc_tls") else {
        return Ok(projected);
    };
    let enabled = match tls.get("enabled") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| figment::Error::from("internal_grpc_tls.enabled must be a boolean"))?,
    };
    let setting = |field: &str| tls.get(field).cloned().unwrap_or(Value::Null);
    let server = if enabled {
        json!({"type": "Enabled", "config": {
            "server_cert_file": setting("cert_file"),
            "server_key_file": setting("key_file"),
            "client_ca_cert_file": setting("ca_cert_file"),
        }})
    } else {
        json!({"type": "Disabled", "config": {}})
    };
    let client = if enabled {
        json!({"type": "Enabled", "config": {
            "client_cert_file": setting("cert_file"),
            "client_key_file": setting("key_file"),
            "server_ca_cert_file": setting("ca_cert_file"),
            "server_domain_name": setting("server_name"),
        }})
    } else {
        json!({"type": "Disabled", "config": {}})
    };
    let projected = grpc_clients(service).iter().fold(
        projected.merge(Serialized::default("grpc.tls", server)),
        |projected, path| projected.merge(Serialized::default(&format!("{path}.tls"), &client)),
    );
    Ok(projected)
}

fn postgres_defaults() -> Value {
    json!({
        "host": "localhost", "port": 5432, "database": "golem",
        "username": "golem", "password": "", "max_connections": 10,
    })
}

fn storage(service: &str, input: &Value) -> figment::Result<Figment> {
    let stores: &[(&str, &str, &str)] = match service {
        "registry_service" => &[("db", "golem_registry_service", "registry.db")],
        "shard_manager" => &[("persistence", "golem_shard_manager", "shard_manager.db")],
        "worker_service" => &[(
            "gateway_session_storage",
            "golem_worker_service_session_store",
            "gateway-sessions.db",
        )],
        "worker_executor" => &[
            ("key_value_storage", "golem_worker_executor", "kv-store"),
            (
                "indexed_storage",
                "golem_worker_executor_indexed",
                "indexed.db",
            ),
            (
                "scheduler_storage",
                "golem_worker_executor_scheduler",
                "scheduler.db",
            ),
        ],
        _ => &[],
    };
    let root = input
        .get("data_dir")
        .and_then(Value::as_str)
        .unwrap_or("/var/lib/golem");
    let state_store = input
        .get("state_store")
        .and_then(Value::as_str)
        .or_else(|| {
            (input.pointer("/cache/type").and_then(Value::as_str) == Some("Redis"))
                .then_some("sqlite")
        });
    let projected = stores.iter().try_fold(Figment::new(), |config, (path, schema, file)| {
        let mut fields = match state_store {
            None => return Ok(config),
            Some("postgres") => {
                let fields = Figment::from(Serialized::defaults(postgres_defaults()))
                .merge(Serialized::defaults(input.get("postgres").cloned().unwrap_or_else(|| json!({}))))
                .merge(Serialized::default("schema", schema));
                json!({"type": "Postgres", "config": fields.extract::<Value>()?})
            }
            Some("sqlite") => match *path {
                "key_value_storage" => json!({"type": "MultiSqlite", "config": {
                    "root_dir": format!("{root}/{file}"), "max_connections": 4, "foreign_keys": false,
                }}),
                "indexed_storage" => json!({"type": "KVStoreMultiSqlite", "config": {}}),
                _ => json!({"type": "Sqlite", "config": {
                    "database": format!("{root}/{file}"), "max_connections": 4,
                    "foreign_keys": (["registry_service", "shard_manager"].contains(&service)),
                }}),
            },
            Some(_) => return Err(figment::Error::from("state_store must be sqlite or postgres")),
        };
        if service == "worker_service" {
            fields["config"]["pending_login_expiration"] = json!("1h");
            fields["config"]["cleanup_interval"] = json!("5m");
        }
        Ok(config.merge(Serialized::default(path, fields)))
    })?;
    let projected = match input.pointer("/cache/type").and_then(Value::as_str) {
        None | Some("Disabled") => projected,
        Some("Redis") => {
            let redis = Figment::from(Serialized::defaults(super::RedisConfig::default())).merge(
                Serialized::defaults(
                    input
                        .pointer("/cache/config")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                ),
            );
            let redis = redis.extract::<Value>()?;
            match service {
                "worker_executor" => {
                    let persistent = projected.extract_inner::<Value>("key_value_storage")?;
                    let projected = if state_store == Some("sqlite") {
                        projected.merge(Serialized::default("indexed_storage", &persistent))
                    } else {
                        projected
                    };
                    projected.merge(Serialized::default("key_value_storage", json!({
                        "type": "NamespaceRouted", "config": {"cache": {"type": "Redis", "config": redis}, "persistent": persistent},
                    })))
                }
                "worker_service" => projected.merge(Serialized::default("gateway_session_storage", json!({
                    "type": "Redis", "config": Figment::from(Serialized::defaults(redis))
                        .merge(Serialized::default("pending_login_expiration", "1h")).extract::<Value>()?,
                }))),
                _ => projected,
            }
        }
        Some(_) => return Err(figment::Error::from("cache.type must be Disabled or Redis")),
    };
    if ![
        "registry_service",
        "worker_service",
        "worker_executor",
        "component_compilation_service",
    ]
    .contains(&service)
    {
        return Ok(projected);
    }
    let blob = match input.get("blob_store").and_then(Value::as_str) {
        None => return Ok(projected),
        Some("data_disk") => {
            json!({"type": "LocalFileSystem", "config": {"root": input.pointer("/data_disk/blob_root").cloned().unwrap_or_else(|| json!(format!("{root}/blobs")))}})
        }
        Some("s3") => {
            let object_storage = input
                .get("object_storage")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let data_bucket = object_storage
                .get("data_bucket")
                .cloned()
                .unwrap_or_else(|| json!("golem-data"));
            let fields = Figment::from(Serialized::defaults(json!({
                "region": "us-east-1", "object_prefix": "",
                "retries": {"max_attempts": 3, "min_delay": "100ms", "max_delay": "1s", "multiplier": 3.0, "max_jitter_factor": 0.15},
                "namespace_prefixes": true,
                "components_bucket": "golem-components", "artifacts_bucket": "golem-artifacts",
                "custom_data_bucket": data_bucket,
                "oplog_payload_bucket": data_bucket,
                "compressed_oplog_buckets": [data_bucket],
                "compilation_cache_bucket": data_bucket,
                "initial_agent_files_bucket": data_bucket,
                "filesystem_snapshots_bucket": data_bucket,
            })));
            let fields = [
                ("endpoint_url", "aws_endpoint_url"),
                ("public_endpoint_url", "public_endpoint_url"),
                ("region", "region"),
                ("path_style", "aws_path_style"),
                ("object_prefix", "object_prefix"),
                ("credentials", "aws_credentials"),
                ("components_bucket", "components_bucket"),
                ("artifacts_bucket", "artifacts_bucket"),
            ]
            .into_iter()
            .fold(fields, |fields, (from, to)| {
                match object_storage.get(from) {
                    Some(value) => fields.merge(Serialized::default(to, value)),
                    None => fields,
                }
            });
            json!({"type": "S3", "config": fields.extract::<Value>()?})
        }
        Some(_) => return Err(figment::Error::from("blob_store must be data_disk or s3")),
    };
    Ok(projected.merge(Serialized::default("blob_storage", blob)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::providers::{Format, Toml};
    use test_r::test;

    #[test]
    fn sqlite_with_a_redis_cache_has_independent_indexed_storage() {
        let input = Figment::from(Toml::string(
            r#"
            state_store = "sqlite"
            data_dir = "/state"
            [cache]
            type = "Redis"
            "#,
        ));
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
            public_endpoint_url = "https://downloads.example.com"
            components_bucket = "components"
            artifacts_bucket = "artifacts"
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
                    .extract_inner::<String>("blob_storage.config.public_endpoint_url")
                    .unwrap(),
                "https://downloads.example.com"
            );
            assert_eq!(
                config
                    .extract_inner::<String>("blob_storage.config.components_bucket")
                    .unwrap(),
                "components"
            );
            assert_eq!(
                config
                    .extract_inner::<String>("blob_storage.config.artifacts_bucket")
                    .unwrap(),
                "artifacts"
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
    }

    #[test]
    fn snapshot_retention_follows_the_executor_variant() {
        [
            (json!({"filesystem_snapshots": {"type": "Enabled"}}), Some(2)),
            (json!({"filesystem_snapshots": {"type": "Disabled"}}), None),
            (json!({"retained_update_snapshots": 7}), Some(7)),
            (json!({"advanced": {"worker_executor": {"filesystem_snapshots": {"type": "Enabled"}}}}), Some(2)),
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
}
