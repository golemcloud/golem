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
use figment::value::{Dict, Map, Tag, Value as RawValue};
use figment::{Figment, Metadata, Profile, Provider};
use serde_json::{Value, json};

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
    service_settings(service, input).unwrap_or_else(|error| {
        Figment::from(SettingsValues {
            metadata: error
                .metadata
                .clone()
                .unwrap_or_else(|| Metadata::named("server settings")),
            data: Err(error),
            sources: input.__metadata_map().unwrap_or_default(),
        })
    })
}

/// Projects the common settings onto the embedded services and retains launcher settings.
pub fn all_in_one(input: &Figment) -> figment::Result<Figment> {
    let root: Value = input.extract()?;
    validate_settings(input, &root, None)?;
    if root.pointer("/advanced/server/data_dir").is_some() {
        return Err(invalid_setting(
            input,
            "advanced.server.data_dir",
            "use supported data_dir instead of advanced.server.data_dir",
        ));
    }
    SERVICES
        .into_iter()
        .chain(["router", "golem_issuer", "ports_file"])
        .try_for_each(|key| {
            if root.get(key).is_some() {
                Err(invalid_setting(
                    input,
                    key,
                    format!("use the advanced channel for unsupported setting {key}"),
                ))
            } else {
                Ok(())
            }
        })?;
    let mut service_input: RawValue = input.extract()?;
    if let RawValue::Dict(_, config) = &mut service_input {
        config.remove("logging");
        config.remove("otlp");
    }
    let service_input = raw_values(input, service_input);
    let projected = SERVICES
        .into_iter()
        .try_fold(Figment::new(), |projected, service| {
            let config = project(service, &service_input);
            config.extract::<RawValue>()?;
            Ok::<_, figment::Error>(projected.merge(relocate(&config, "", service)))
        })?;
    let logging =
        relocate(input, "logging", "logging").merge(relocate(input, "otlp", "logging.otlp"));
    let server = relocate(input, "advanced.server", "");
    let mut root: RawValue = input.extract()?;
    if let RawValue::Dict(_, root) = &mut root {
        [
            "state_store",
            "blob_store",
            "postgres",
            "data_disk",
            "object_storage",
            "cache",
            "logging",
            "otlp",
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
        .merge(logging)
        .merge(raw_values(input, root))
        .merge(server))
}

/// Projects one advanced layer and retains supported settings for the selected variants.
pub fn server_advanced(input: &Figment, lower: &Figment) -> figment::Result<Figment> {
    let value: Value = input.extract()?;
    let lower_executor = project("worker_executor", lower);
    let lower_executor_value: Value = lower_executor.extract()?;
    let lower_value: Value = lower.extract()?;
    let persistence = "/advanced/shard_manager/persistence/type";
    let snapshots = "/advanced/worker_executor/filesystem_snapshots/type";
    let retained = lower_executor_value
        .pointer("/filesystem_snapshots/config/retained_update_snapshots")
        .filter(|_| {
            lower_executor_value
                .pointer("/filesystem_snapshots/type")
                .and_then(Value::as_str)
                == Some("Enabled")
        })
        .map(|_| {
            relocate(
                &lower_executor,
                "filesystem_snapshots.config.retained_update_snapshots",
                "retained_update_snapshots",
            )
        })
        .unwrap_or_else(|| {
            relocate(
                lower,
                "retained_update_snapshots",
                "retained_update_snapshots",
            )
        });
    let inherited = [
        (
            value.pointer(persistence).and_then(Value::as_str) == Some("Etcd")
                && value.pointer(persistence) != lower_value.pointer(persistence),
            relocate(lower, "etcd", "etcd"),
        ),
        (
            value.pointer(snapshots).and_then(Value::as_str) == Some("Enabled"),
            retained,
        ),
    ]
    .into_iter()
    .filter(|(needed, _)| *needed)
    .fold(Figment::new(), |inherited, (_, value)| {
        inherited.merge(value)
    });
    all_in_one(&inherited.merge(input))
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
        "cache": {"enabled": false},
        "postgres": postgres_defaults(),
        "object_storage": {
            "region": "us-east-1", "object_prefix": "",
            "components_bucket": "golem-components",
            "data_bucket": "golem-data",
        },
        "internal_grpc_tls": {"enabled": false},
        "retained_update_snapshots": 2,
        "outbound_sockets": {"enabled": false},
    });
    if let Some(config) = advanced.as_object_mut() {
        common["logging"] = config.remove("tracing").unwrap_or_else(|| json!({}));
        if let Some(logging) = common["logging"].as_object_mut() {
            let otlp = logging.remove("otlp");
            common["otlp"] = otlp.unwrap_or_else(|| json!({}));
        }
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
                    common["outbound_sockets"]["enabled"] =
                        sockets.remove("enabled").unwrap_or(Value::Bool(false));
                }
                let snapshots = config
                    .remove("filesystem_snapshots")
                    .unwrap_or_else(|| json!({}));
                let mut fields = snapshots
                    .get("config")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                fields["enabled"] =
                    json!(snapshots.get("type").and_then(Value::as_str) == Some("Enabled"));
                common["filesystem_snapshots"] = fields;
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
    Figment::from(Serialized::defaults(common))
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
                ("aws_path_style", "path_style"),
                ("components_bucket", "components_bucket"),
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

struct SettingsValues {
    metadata: Metadata,
    data: figment::Result<Dict>,
    sources: Map<Tag, Metadata>,
}

impl Provider for SettingsValues {
    fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    fn data(&self) -> figment::Result<Map<Profile, Dict>> {
        self.data.clone().map(|dict| Profile::Default.collect(dict))
    }

    fn __metadata_map(&self) -> Option<Map<Tag, Metadata>> {
        Some(self.sources.clone())
    }
}

fn raw_values(input: &Figment, value: RawValue) -> Figment {
    Figment::from(SettingsValues {
        metadata: Metadata::named("server settings"),
        data: Ok(value.into_dict().expect("settings must be a table")),
        sources: input.__metadata_map().unwrap_or_default(),
    })
}

/// Drops replaced variant fields and restores matching defaults before merging.
pub(super) fn merge_variants(lower: Figment, upper: Figment, defaults: &Figment) -> Figment {
    fn reset_configs(lower: &mut RawValue, upper: &RawValue, defaults: Option<&RawValue>) {
        let (RawValue::Dict(_, lower), RawValue::Dict(_, upper)) = (lower, upper) else {
            return;
        };
        let lower_type = lower.get("type").and_then(RawValue::as_str);
        let upper_type = upper.get("type").and_then(RawValue::as_str);
        let selected_type = upper_type.or(lower_type);
        let defaults = defaults
            .and_then(RawValue::as_dict)
            .filter(|defaults| defaults.get("type").and_then(RawValue::as_str) == selected_type);
        if lower_type.is_some() && upper_type.is_some() && lower_type != upper_type {
            lower.remove("config");
            if let Some(config) = defaults.and_then(|defaults| defaults.get("config")) {
                lower.insert("config".into(), config.clone());
            }
        }
        upper.iter().for_each(|(key, value)| {
            if let Some(lower) = lower.get_mut(key) {
                reset_configs(
                    lower,
                    value,
                    defaults.and_then(|defaults| defaults.get(key)),
                );
            }
        });
    }

    let (Ok(mut value), Ok(upper_value)) =
        (lower.extract::<RawValue>(), upper.extract::<RawValue>())
    else {
        return lower.merge(upper);
    };
    let defaults = defaults.extract::<RawValue>().ok();
    reset_configs(&mut value, &upper_value, defaults.as_ref());
    raw_values(&lower, value).merge(upper)
}

/// Supplies a missing variant config as an empty table when deserialization requires it.
pub(super) fn complete_variant_configs<T: super::ConfigLoaderConfig>(input: Figment) -> Figment {
    let missing = input.extract::<T>().err().and_then(|error| {
        error.into_iter().find_map(|error| {
            if !matches!(error.kind, figment::error::Kind::MissingField(ref field) if &**field == "config") {
                return None;
            }
            let path = |field| {
                error.path.iter().map(String::as_str).chain([field]).collect::<Vec<_>>().join(".")
            };
            let config = path("config");
            let tag = path("type");
            (input.find_value(&config).is_err() && input.extract_inner::<String>(&tag).is_ok())
                .then_some((tag, config))
        })
    });
    match missing {
        Some((tag, config)) => {
            let empty = generated(&input, &tag, &config, json!({}));
            complete_variant_configs::<T>(input.merge(empty))
        }
        None => input,
    }
}

fn relocate(input: &Figment, from: &str, to: &str) -> Figment {
    let value = if from.is_empty() {
        input.extract::<RawValue>().ok()
    } else {
        input.find_value(from).ok()
    };
    value.map_or_else(Figment::new, |value| located(input, value, from, to))
}

fn generated(input: &Figment, origin: &str, to: &str, value: Value) -> Figment {
    located(
        input,
        RawValue::serialize(value).expect("generated settings must serialize"),
        origin,
        to,
    )
}

fn located(input: &Figment, value: RawValue, from: &str, to: &str) -> Figment {
    fn location_tag(
        input: &Figment,
        tag: Tag,
        from: &str,
        to: &str,
        sources: &mut Map<Tag, Metadata>,
    ) -> Tag {
        let metadata = input
            .get_metadata(tag)
            .cloned()
            .or_else(|| {
                input
                    .find_value(from)
                    .ok()
                    .and_then(|value| input.get_metadata(value.tag()).cloned())
            })
            .unwrap_or_else(|| Metadata::named("server settings defaults"));
        let original = metadata.clone();
        let source: Box<[Box<str>]> = from
            .split('.')
            .filter(|key| !key.is_empty())
            .map(Into::into)
            .collect();
        let destination: Box<[Box<str>]> = to
            .split('.')
            .filter(|key| !key.is_empty())
            .map(Into::into)
            .collect();
        let metadata = metadata.interpolater(move |profile, path| {
            if path.len() >= destination.len()
                && path
                    .iter()
                    .zip(destination.iter())
                    .all(|(key, expected)| *key == expected.as_ref())
            {
                let path: Vec<&str> = source
                    .iter()
                    .map(AsRef::as_ref)
                    .chain(path[destination.len()..].iter().copied())
                    .collect();
                original.interpolate(profile, &path)
            } else {
                original.interpolate(profile, path)
            }
        });
        let tagged = Figment::from(SettingsValues {
            metadata,
            data: Ok(Dict::from([("value".into(), false.into())])),
            sources: Map::new(),
        });
        let tag = tagged
            .find_value("value")
            .expect("tag allocation must succeed")
            .tag();
        sources.extend(tagged.__metadata_map().unwrap_or_default());
        tag
    }

    fn retag(
        input: &Figment,
        value: RawValue,
        from: &str,
        to: &str,
        sources: &mut Map<Tag, Metadata>,
    ) -> RawValue {
        let tag = location_tag(input, value.tag(), from, to, sources);
        match value {
            RawValue::String(_, value) => RawValue::String(tag, value),
            RawValue::Char(_, value) => RawValue::Char(tag, value),
            RawValue::Bool(_, value) => RawValue::Bool(tag, value),
            RawValue::Num(_, value) => RawValue::Num(tag, value),
            RawValue::Empty(_, value) => RawValue::Empty(tag, value),
            RawValue::Dict(_, fields) => RawValue::Dict(
                tag,
                fields
                    .into_iter()
                    .map(|(key, value)| (key, retag(input, value, from, to, sources)))
                    .collect(),
            ),
            RawValue::Array(_, values) => RawValue::Array(
                tag,
                values
                    .into_iter()
                    .map(|value| retag(input, value, from, to, sources))
                    .collect(),
            ),
        }
    }
    let mut sources = Map::new();
    let value = retag(input, value, from, to, &mut sources);
    let mut source_parent = from;
    let mut destination_parent = to;
    let value = to
        .split('.')
        .filter(|key| !key.is_empty())
        .rev()
        .fold(value, |value, key| {
            source_parent = source_parent
                .rsplit_once('.')
                .map_or(source_parent, |(parent, _)| parent);
            destination_parent = destination_parent
                .rsplit_once('.')
                .map_or("", |(parent, _)| parent);
            let tag = location_tag(
                input,
                Tag::Default,
                source_parent,
                destination_parent,
                &mut sources,
            );
            RawValue::Dict(tag, Dict::from([(key.into(), value)]))
        });
    Figment::from(SettingsValues {
        metadata: Metadata::named("server settings"),
        data: Ok(value
            .into_dict()
            .expect("projected settings must be a table")),
        sources,
    })
}

fn invalid_setting(input: &Figment, path: &str, message: impl Into<String>) -> figment::Error {
    let mut error = figment::Error::from(message.into());
    error.metadata = input
        .find_value(path)
        .ok()
        .and_then(|value| input.get_metadata(value.tag()).cloned());
    error.path = path.split('.').map(str::to_string).collect();
    error.profile = Some(Profile::Default);
    error
}

fn validate_settings(input: &Figment, value: &Value, service: Option<&str>) -> figment::Result<()> {
    [
        "/postgres",
        "/data_disk",
        "/object_storage",
        "/object_storage/credentials",
        "/cache",
        "/cache/retries",
        "/logging",
        "/logging/stdout",
        "/logging/stderr",
        "/logging/file",
        "/logging/otlp",
        "/otlp",
        "/internal_grpc_tls",
        "/etcd",
        "/etcd/tls",
        "/outbound_sockets",
        "/filesystem_snapshots",
        "/filesystem_snapshots/storage_retry",
        "/advanced",
    ]
    .into_iter()
    .chain(
        [
            "/advanced/server",
            "/advanced/registry_service",
            "/advanced/shard_manager",
            "/advanced/worker_service",
            "/advanced/worker_executor",
            "/advanced/component_compilation_service",
        ]
        .into_iter()
        .filter(|pointer| {
            service.is_none_or(|service| pointer.strip_prefix("/advanced/") == Some(service))
        }),
    )
    .try_for_each(|pointer| match value.pointer(pointer) {
        Some(value) if !value.is_object() => Err(invalid_setting(
            input,
            &pointer[1..].replace('/', "."),
            format!("{} must be a table", pointer[1..].replace('/', ".")),
        )),
        _ => Ok(()),
    })?;
    if value
        .get("data_dir")
        .is_some_and(|value| !value.is_string())
    {
        return Err(invalid_setting(
            input,
            "data_dir",
            "data_dir must be a string",
        ));
    }
    [
        ("state_store", ["sqlite", "postgres"]),
        ("blob_store", ["data_disk", "s3"]),
    ]
    .into_iter()
    .try_for_each(|(key, allowed)| match value.get(key) {
        Some(value) if !value.as_str().is_some_and(|value| allowed.contains(&value)) => {
            Err(invalid_setting(
                input,
                key,
                format!("{key} must be {} or {}", allowed[0], allowed[1]),
            ))
        }
        _ => Ok(()),
    })?;
    [
        ("cache.enabled", "/cache/enabled"),
        (
            "filesystem_snapshots.enabled",
            "/filesystem_snapshots/enabled",
        ),
        ("internal_grpc_tls.enabled", "/internal_grpc_tls/enabled"),
        ("otlp.enabled", "/otlp/enabled"),
        ("outbound_sockets.enabled", "/outbound_sockets/enabled"),
    ]
    .into_iter()
    .try_for_each(|(key, pointer)| match value.pointer(pointer) {
        Some(value) if !value.is_boolean() => Err(invalid_setting(
            input,
            key,
            format!("{key} must be true or false"),
        )),
        _ => Ok(()),
    })
}

fn service_settings(service: &str, input: &Figment) -> figment::Result<Figment> {
    let value: Value = input.extract()?;
    validate_settings(input, &value, Some(service))?;
    ["number_of_shards", "retained_periodic_snapshots"]
        .into_iter()
        .try_for_each(|key| {
            if value.get(key).is_some() {
                Err(invalid_setting(
                    input,
                    key,
                    format!("{key} is not a server setting"),
                ))
            } else {
                Ok(())
            }
        })?;
    if value
        .pointer("/filesystem_snapshots/retained_periodic_snapshots")
        .is_some()
    {
        return Err(invalid_setting(
            input,
            "filesystem_snapshots.retained_periodic_snapshots",
            "retained_periodic_snapshots is not a server setting",
        ));
    }
    let projected = storage(service, input, &value)?;
    let projected = [("logging", "tracing"), ("otlp", "tracing.otlp")]
        .into_iter()
        .fold(projected, |projected, (from, to)| {
            projected.merge(relocate(input, from, to))
        });
    let projected = if service == "worker_executor" {
        let projected = projected.merge(relocate(
            input,
            "outbound_sockets.enabled",
            "agent_sockets.enabled",
        ));
        match value.get("filesystem_snapshots") {
            Some(snapshots) => {
                let enabled = snapshots.get("enabled").and_then(Value::as_bool);
                let settings = enabled.map_or_else(Figment::new, |enabled| {
                    generated(
                        input,
                        "filesystem_snapshots",
                        "filesystem_snapshots",
                        json!({
                            "type": if enabled { "Enabled" } else { "Disabled" },
                            "config": {},
                        }),
                    )
                });
                let settings = if enabled != Some(false) {
                    let mut fields: RawValue = input.find_value("filesystem_snapshots")?;
                    if let RawValue::Dict(_, fields) = &mut fields {
                        fields.remove("enabled");
                    }
                    settings.merge(located(
                        input,
                        fields,
                        "filesystem_snapshots",
                        "filesystem_snapshots.config",
                    ))
                } else {
                    settings
                };
                projected.merge(settings)
            }
            None => projected,
        }
    } else {
        projected
    };
    let advanced = relocate(input, &format!("advanced.{service}"), "");
    let projected = merge_variants(projected, advanced.clone(), &Figment::new());
    let projected = transport(service, input, &value, projected);
    let projected = if service == "worker_executor"
        && (value.get("retained_update_snapshots").is_some()
            || projected
                .clone()
                .merge(&advanced)
                .extract_inner::<String>("filesystem_snapshots.type")
                .as_deref()
                == Ok("Enabled"))
    {
        let path = "filesystem_snapshots.config.retained_update_snapshots";
        projected.merge(if value.get("retained_update_snapshots").is_some() {
            relocate(input, "retained_update_snapshots", path)
        } else {
            generated(input, "filesystem_snapshots", path, json!(2))
        })
    } else {
        projected
    };
    Ok(merge_variants(projected, advanced, &Figment::new()))
}

fn transport(service: &str, input: &Figment, value: &Value, projected: Figment) -> Figment {
    let projected = if service == "shard_manager" {
        projected.merge(relocate(input, "etcd.tls", "persistence.config.tls"))
    } else {
        projected
    };
    let Some(tls) = value.get("internal_grpc_tls") else {
        return projected;
    };
    let enabled = tls.get("enabled").and_then(Value::as_bool) == Some(true);
    let tls_config = |path: &str, fields: &[(&str, &str)]| {
        let defaults = generated(
            input,
            "internal_grpc_tls",
            path,
            json!({"type": if enabled { "Enabled" } else { "Disabled" }, "config": {}}),
        );
        if enabled {
            fields.iter().fold(defaults, |config, (from, to)| {
                config
                    .merge(generated(
                        input,
                        "internal_grpc_tls",
                        &format!("{path}.config.{to}"),
                        Value::Null,
                    ))
                    .merge(relocate(
                        input,
                        &format!("internal_grpc_tls.{from}"),
                        &format!("{path}.config.{to}"),
                    ))
            })
        } else {
            defaults
        }
    };
    grpc_clients(service).iter().fold(
        projected.merge(tls_config(
            "grpc.tls",
            &[
                ("cert_file", "server_cert_file"),
                ("key_file", "server_key_file"),
                ("ca_cert_file", "client_ca_cert_file"),
            ],
        )),
        |projected, path| {
            projected.merge(tls_config(
                &format!("{path}.tls"),
                &[
                    ("cert_file", "client_cert_file"),
                    ("key_file", "client_key_file"),
                    ("ca_cert_file", "server_ca_cert_file"),
                    ("server_name", "server_domain_name"),
                ],
            ))
        },
    )
}

fn postgres_defaults() -> Value {
    json!({
        "host": "localhost", "port": 5432, "database": "golem",
        "username": "golem", "password": "", "max_connections": 10,
    })
}

fn storage(service: &str, input: &Figment, value: &Value) -> figment::Result<Figment> {
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
    let root = value
        .get("data_dir")
        .and_then(Value::as_str)
        .unwrap_or("/var/lib/golem");
    let state_store = value
        .get("state_store")
        .and_then(Value::as_str)
        .or_else(|| {
            (value.pointer("/cache/enabled").and_then(Value::as_bool) == Some(true))
                .then_some("sqlite")
        });
    let projected = stores.iter().try_fold(Figment::new(), |config, (path, schema, file)| {
        let fields = match state_store {
            None => return Ok(config),
            Some("postgres") => {
                generated(input, "postgres", path, json!({"type": "Postgres", "config": postgres_defaults()}))
                    .merge(relocate(input, "postgres", &format!("{path}.config")))
                    .merge(generated(input, "postgres", &format!("{path}.config.schema"), json!(schema)))
            }
            Some("sqlite") => generated(input, "data_dir", path, match *path {
                "key_value_storage" => json!({"type": "MultiSqlite", "config": {
                    "root_dir": format!("{root}/{file}"), "max_connections": 4, "foreign_keys": false,
                }}),
                "indexed_storage" => json!({"type": "KVStoreMultiSqlite", "config": {}}),
                _ => json!({"type": "Sqlite", "config": {
                    "database": format!("{root}/{file}"), "max_connections": 4,
                    "foreign_keys": (["registry_service", "shard_manager"].contains(&service)),
                }}),
            }),
            Some(_) => return Err(invalid_setting(input, "state_store", "state_store must be sqlite or postgres")),
        };
        let fields = if service == "worker_service" {
            let origin = if state_store == Some("postgres") { "postgres" } else { "data_dir" };
            fields.merge(generated(input, origin, &format!("{path}.config.pending_login_expiration"), json!("1h")))
                .merge(generated(input, origin, &format!("{path}.config.cleanup_interval"), json!("5m")))
        } else { fields };
        Ok(config.merge(fields))
    })?;
    let projected = match value.pointer("/cache/enabled").and_then(Value::as_bool) {
        None | Some(false) => projected,
        Some(true) => {
            let mut fields = input.find_value("cache")?;
            if let RawValue::Dict(_, fields) = &mut fields {
                fields.remove("enabled");
            }
            let redis = json!(super::RedisConfig::default());
            match service {
                "worker_executor" => {
                    let persistent = relocate(
                        &projected,
                        "key_value_storage",
                        "key_value_storage.config.persistent",
                    );
                    let projected = if state_store == Some("sqlite") {
                        let indexed = relocate(&projected, "key_value_storage", "indexed_storage");
                        projected.merge(indexed)
                    } else {
                        projected
                    };
                    projected.merge(generated(input, "cache", "key_value_storage", json!({
                        "type": "NamespaceRouted", "config": {"cache": {"type": "Redis", "config": {}}},
                    })))
                    .merge(generated(input, "cache", "key_value_storage.config.cache.config", redis))
                    .merge(located(input, fields, "cache", "key_value_storage.config.cache.config"))
                    .merge(persistent)
                }
                "worker_service" => projected
                    .merge(generated(
                        input,
                        "cache",
                        "gateway_session_storage",
                        json!({
                            "type": "Redis", "config": {},
                        }),
                    ))
                    .merge(generated(
                        input,
                        "cache",
                        "gateway_session_storage.config",
                        redis,
                    ))
                    .merge(located(
                        input,
                        fields,
                        "cache",
                        "gateway_session_storage.config",
                    ))
                    .merge(generated(
                        input,
                        "cache",
                        "gateway_session_storage.config.pending_login_expiration",
                        json!("1h"),
                    )),
                _ => projected,
            }
        }
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
    let blob_store = value
        .get("blob_store")
        .and_then(Value::as_str)
        .or_else(|| value.get("data_disk").map(|_| "data_disk"));
    let blob = match blob_store {
        None => return Ok(projected),
        Some("data_disk") => generated(
            input,
            "data_dir",
            "blob_storage",
            json!({"type": "LocalFileSystem", "config": {"root": format!("{root}/blobs")}}),
        )
        .merge(relocate(
            input,
            "data_disk.blob_root",
            "blob_storage.config.root",
        )),
        Some("s3") => {
            let fields = generated(
                input,
                "object_storage",
                "blob_storage",
                json!({"type": "S3", "config": {
                    "region": "us-east-1", "object_prefix": "",
                    "retries": {"max_attempts": 3, "min_delay": "100ms", "max_delay": "1s", "multiplier": 3.0, "max_jitter_factor": 0.15},
                    "namespace_prefixes": true,
                    "components_bucket": "golem-components",
                    "custom_data_bucket": "golem-data",
                    "oplog_payload_bucket": "golem-data",
                    "compressed_oplog_buckets": ["golem-data"],
                    "compilation_cache_bucket": "golem-data",
                    "initial_agent_files_bucket": "golem-data",
                    "filesystem_snapshots_bucket": "golem-data",
                }}),
            );
            let fields = [
                ("endpoint_url", "aws_endpoint_url"),
                ("region", "region"),
                ("path_style", "aws_path_style"),
                ("object_prefix", "object_prefix"),
                ("credentials", "aws_credentials"),
                ("components_bucket", "components_bucket"),
            ]
            .into_iter()
            .fold(fields, |fields, (from, to)| {
                fields.merge(relocate(
                    input,
                    &format!("object_storage.{from}"),
                    &format!("blob_storage.config.{to}"),
                ))
            });
            let fields = [
                "custom_data_bucket",
                "oplog_payload_bucket",
                "compilation_cache_bucket",
                "initial_agent_files_bucket",
                "filesystem_snapshots_bucket",
            ]
            .into_iter()
            .fold(fields, |fields, to| {
                fields.merge(relocate(
                    input,
                    "object_storage.data_bucket",
                    &format!("blob_storage.config.{to}"),
                ))
            });
            match input.find_value("object_storage.data_bucket") {
                Ok(bucket) => fields.merge(located(
                    input,
                    RawValue::Array(bucket.tag(), vec![bucket]),
                    "object_storage.data_bucket",
                    "blob_storage.config.compressed_oplog_buckets",
                )),
                Err(_) => fields,
            }
        }
        Some(_) => {
            return Err(invalid_setting(
                input,
                "blob_store",
                "blob_store must be data_disk or s3",
            ));
        }
    };
    Ok(projected.merge(blob))
}

#[cfg(test)]
mod tests;
