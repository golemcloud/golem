// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_api_grpc::proto::grpc::health::v1::health_check_response::ServingStatus;
use golem_api_grpc::proto::grpc::health::v1::HealthCheckRequest;
use golem_client::api::HealthCheckClient;
use golem_client::Security;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::Child;
use std::str::FromStr;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info, trace};
use tracing::{error, warn, Level};
use url::Url;

pub mod blob_storage;
pub mod component_compilation_service;
mod docker;
mod dynamic_span;
pub mod rdb;
pub mod redis;
pub mod redis_monitor;
pub mod registry_service;
pub mod service;
pub mod shard_manager;
pub mod worker_executor;
pub mod worker_executor_cluster;
pub mod worker_service;

pub struct ChildProcessLogger {
    _out_handle: JoinHandle<()>,
    _err_handle: JoinHandle<()>,
}

impl ChildProcessLogger {
    pub fn log_child_process(
        prefix: &str,
        out_level: Level,
        err_level: Level,
        child: &mut Child,
    ) -> Self {
        let stdout = child
            .stdout
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stdout"));

        let stderr = child
            .stderr
            .take()
            .unwrap_or_else(|| panic!("Can't get {prefix} stderr"));

        let prefix_clone = prefix.to_string();
        let stdout_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => relay_line(&prefix_clone, &line, out_level),
                    Err(e) => {
                        warn!("{} failed to read stdout: {e}", prefix_clone);
                        break;
                    }
                }
            }
        });

        let prefix_clone = prefix.to_string();
        let stderr_handle = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                match line {
                    Ok(line) => relay_line(&prefix_clone, &line, err_level),
                    Err(e) => {
                        warn!("{} failed to read stderr: {e}", prefix_clone);
                        break;
                    }
                }
            }
        });

        Self {
            _out_handle: stdout_handle,
            _err_handle: stderr_handle,
        }
    }
}

fn relay_line(prefix: &str, line: &str, fallback_level: Level) {
    let Ok(obj) = serde_json::from_str::<serde_json::Value>(line) else {
        emit_at_level(fallback_level, prefix, line);
        return;
    };

    let level = obj
        .get("level")
        .and_then(|v| v.as_str())
        .and_then(parse_tracing_level)
        .unwrap_or(fallback_level);

    let target = obj
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("child_process");

    let file = obj.get("filename").and_then(|v| v.as_str());

    let line = obj
        .get("line_number")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let message = obj
        .get("fields")
        .and_then(|f| f.get("message"))
        .and_then(|v| v.as_str())
        .or_else(|| obj.get("message").and_then(|v| v.as_str()))
        .unwrap_or("");

    let span_infos = parse_span_infos(&obj);
    // Create and enter each span before creating the next, so each becomes
    // a child of the previous one (proper nesting).
    let _entered: Vec<tracing::span::EnteredSpan> = span_infos
        .iter()
        .map(|(name, fields)| dynamic_span::make_span(prefix, name, fields).entered())
        .collect();

    let event_fields =
        format_kv_fields(obj.get("fields").and_then(|f| f.as_object()), &["message"]);

    let msg = if event_fields.is_empty() {
        format!("{prefix} {message}")
    } else {
        format!("{prefix} {message} {event_fields}")
    };
    dynamic_span::dispatch_event(target, level, &msg, file, line);
}

fn parse_span_infos(obj: &serde_json::Value) -> Vec<(String, Vec<(String, String)>)> {
    obj.get("spans")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|span_obj| {
                    let name = span_obj.get("name")?.as_str()?.to_string();
                    let fields = parse_kv_fields(span_obj.as_object(), &["name"]);
                    Some((name, fields))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_kv_fields(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    skip: &[&str],
) -> Vec<(String, String)> {
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter()
        .filter(|(k, _)| !skip.contains(&k.as_str()))
        .map(|(k, v)| {
            let val = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), val)
        })
        .collect()
}

fn format_kv_fields(
    map: Option<&serde_json::Map<String, serde_json::Value>>,
    skip: &[&str],
) -> String {
    let pairs = parse_kv_fields(map, skip);
    if pairs.is_empty() {
        String::new()
    } else {
        pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

fn emit_at_level(level: Level, prefix: &str, line: &str) {
    match level {
        Level::TRACE => trace!("{prefix} {line}"),
        Level::DEBUG => debug!("{prefix} {line}"),
        Level::INFO => info!("{prefix} {line}"),
        Level::WARN => warn!("{prefix} {line}"),
        Level::ERROR => error!("{prefix} {line}"),
    }
}

fn parse_tracing_level(s: &str) -> Option<Level> {
    let s = s.trim();
    let s = s
        .strip_prefix("Level(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(s);
    match s.to_uppercase().as_str() {
        "TRACE" => Some(Level::TRACE),
        "DEBUG" => Some(Level::DEBUG),
        "INFO" => Some(Level::INFO),
        "WARN" => Some(Level::WARN),
        "ERROR" => Some(Level::ERROR),
        _ => None,
    }
}

pub async fn wait_for_startup_grpc(host: &str, grpc_port: u16, name: &str, timeout: Duration) {
    info!(
        "Waiting for {name} (GRPC) start on host {host}:{grpc_port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let success =
            match golem_api_grpc::proto::grpc::health::v1::health_client::HealthClient::connect(
                format!("http://{host}:{grpc_port}"),
            )
            .await
            {
                Ok(mut client) => match client
                    .check(HealthCheckRequest {
                        service: "".to_string(),
                    })
                    .await
                {
                    Ok(response) => response.into_inner().status == ServingStatus::Serving as i32,
                    Err(err) => {
                        debug!("Health request for {name} returned with an error: {err:?}");
                        false
                    }
                },
                Err(_) => false,
            };
        if success {
            break;
        } else {
            if start.elapsed() > timeout {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

pub async fn wait_for_startup_http(host: &str, http_port: u16, name: &str, timeout: Duration) {
    info!(
        "Waiting for {name} (HTTP) start on host {host}:{http_port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    loop {
        let client = golem_client::api::HealthCheckClientLive {
            context: golem_client::Context {
                client: new_reqwest_client(),
                base_url: Url::from_str(&format!("http://{host}:{http_port}"))
                    .expect("Can't parse HTTP URL for health check"),
                security_token: Security::Empty,
            },
        };

        let success = match client.healthcheck().await {
            Ok(_) => true,
            Err(err) => {
                debug!("Health request for {name} returned with an error: {err:?}");
                false
            }
        };
        if success {
            break;
        } else {
            if start.elapsed() > timeout {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

pub async fn wait_for_startup_http_any_response(
    host: &str,
    http_port: u16,
    name: &str,
    timeout: Duration,
) {
    info!(
        "Waiting for {name} (HTTP) start on host {host}:{http_port}, timeout: {}s",
        timeout.as_secs()
    );
    let start = Instant::now();
    let client = reqwest::Client::new();
    let url = reqwest::Url::parse(&format!("http://{host}:{http_port}")).unwrap();
    loop {
        let success = match client.get(url.clone()).send().await {
            Ok(_) => true,
            Err(err) => {
                debug!("request for {name} resulted in an error: {err:?}");
                false
            }
        };

        if success {
            break;
        } else {
            if start.elapsed() > timeout {
                panic!("Failed to verify that {name} is running");
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

struct EnvVarBuilder {
    env_vars: HashMap<String, String>,
}

impl EnvVarBuilder {
    fn default() -> Self {
        Self {
            env_vars: HashMap::new(),
        }
    }

    fn golem_service(verbosity: Level) -> Self {
        Self::default()
            .with_rust_log_with_dep_defaults(verbosity)
            .with_rust_back_log()
            .with_tracing_from_env()
            .with("GOLEM__TRACING__STDOUT__ANSI", "false".to_string())
            .with("GOLEM__TRACING__STDOUT__ENABLED", "true".to_string())
            .with("GOLEM__TRACING__STDOUT__JSON", "true".to_string())
            .with("GOLEM__TRACING__STDOUT__JSON_FLATTEN", "false".to_string())
            .with(
                "GOLEM__TRACING__STDOUT__JSON_FLATTEN_SPAN",
                "false".to_string(),
            )
            .with(
                "GOLEM__TRACING__STDOUT__JSON_SOURCE_LOCATION",
                "true".to_string(),
            )
    }

    fn with(mut self, name: &str, value: String) -> Self {
        self.env_vars.insert(name.to_string(), value);
        self
    }

    fn with_str(self, name: &str, value: &str) -> Self {
        self.with(name, value.to_string())
    }

    fn with_all(mut self, env_vars: HashMap<String, String>) -> Self {
        self.env_vars.extend(env_vars);
        self
    }

    fn with_rust_log_with_dep_defaults(self, verbosity: Level) -> Self {
        let rust_log_level_str = verbosity.as_str().to_lowercase();
        self.with(
            "RUST_LOG",
            format!(
                "{rust_log_level_str},\
                cranelift_codegen=warn,\
                wasmtime_cranelift=warn,\
                wasmtime_jit=warn,\
                h2=warn,\
                hyper=warn,\
                tower=warn,\
                fred=warn,\
                golem_client=warn"
            ),
        )
    }

    fn with_rust_back_log(self) -> Self {
        self.with_str("RUST_BACKLOG", "1")
    }

    fn with_tracing_from_env(mut self) -> Self {
        for (name, value) in
            std::env::vars().filter(|(name, _value)| name.starts_with("GOLEM__TRACING_"))
        {
            self.env_vars.insert(name, value);
        }
        self
    }

    fn with_optional_otlp(mut self, service_name: &str, enabled: bool) -> Self {
        if enabled {
            self.env_vars.insert(
                "GOLEM__TRACING__OTLP__ENABLED".to_string(),
                "true".to_string(),
            );
            self.env_vars.insert(
                "GOLEM__TRACING__OTLP__HOST".to_string(),
                "localhost".to_string(),
            );
            self.env_vars
                .insert("GOLEM__TRACING__OTLP__PORT".to_string(), "4318".to_string());
            self.env_vars.insert(
                "GOLEM__TRACING__OTLP__SERVICE_NAME".to_string(),
                service_name.to_string(),
            );
        }
        self
    }

    fn build(self) -> HashMap<String, String> {
        self.env_vars
    }
}

fn new_reqwest_client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("Failed to build reqwest client")
}
