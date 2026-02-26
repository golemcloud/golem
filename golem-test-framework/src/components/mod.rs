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

    let log_level = obj
        .get("level")
        .and_then(|v| v.as_str())
        .and_then(parse_log_level)
        .unwrap_or_else(|| tracing_to_log_level(fallback_level));

    let target = obj
        .get("target")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let message = obj.get("message").and_then(|v| v.as_str()).unwrap_or("");

    let context = extract_span_context(&obj);

    log::log!(target: target, log_level, "{prefix} {message}{context}");
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

fn parse_log_level(s: &str) -> Option<log::Level> {
    let s = s.trim();
    let s = s
        .strip_prefix("Level(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(s);
    match s.to_uppercase().as_str() {
        "TRACE" => Some(log::Level::Trace),
        "DEBUG" => Some(log::Level::Debug),
        "INFO" => Some(log::Level::Info),
        "WARN" => Some(log::Level::Warn),
        "ERROR" => Some(log::Level::Error),
        _ => None,
    }
}

fn tracing_to_log_level(level: Level) -> log::Level {
    match level {
        Level::TRACE => log::Level::Trace,
        Level::DEBUG => log::Level::Debug,
        Level::INFO => log::Level::Info,
        Level::WARN => log::Level::Warn,
        Level::ERROR => log::Level::Error,
    }
}

const RESERVED_KEYS: &[&str] = &["timestamp", "level", "target", "message"];

fn extract_span_context(obj: &serde_json::Value) -> String {
    let Some(map) = obj.as_object() else {
        return String::new();
    };
    let mut pairs: Vec<String> = Vec::new();
    for (k, v) in map.iter() {
        if RESERVED_KEYS.contains(&k.as_str()) {
            continue;
        }
        match v {
            serde_json::Value::String(s) => pairs.push(format!("{k}={s}")),
            serde_json::Value::Object(inner) => {
                for (ik, iv) in inner {
                    match iv {
                        serde_json::Value::String(s) => pairs.push(format!("{ik}={s}")),
                        other => pairs.push(format!("{ik}={other}")),
                    }
                }
            }
            other => pairs.push(format!("{k}={other}")),
        }
    }
    if pairs.is_empty() {
        String::new()
    } else {
        format!(" {}", pairs.join(" "))
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
            .with("GOLEM__TRACING__STDOUT__JSON_FLATTEN", "true".to_string())
            .with(
                "GOLEM__TRACING__STDOUT__JSON_FLATTEN_SPAN",
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
