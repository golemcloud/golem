// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

use crate::components::cloud_service::CloudService;
use async_trait::async_trait;
use tracing::Level;

use crate::components::rdb::Rdb;
use crate::components::{wait_for_startup_grpc, ROOT_TOKEN};

pub mod spawned;

#[async_trait]
pub trait ComponentService {
    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }

    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }

    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }

    fn kill(&self);
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "cloud-component-service", timeout).await
}

fn env_vars(
    http_port: u16,
    grpc_port: u16,
    component_compilation_service: Option<(&str, u16)>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    cloud: Arc<dyn CloudService + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();
    let vars: &[(&str, &str)] = &[
        ("ENVIRONMENT", "local"),
        ("GOLEM__ENVIRONMENT", "local"),
        ("GOLEM__WORKSPACE", "it"),
        ("RUST_LOG"                     , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
        ("WASMTIME_BACKTRACE_DETAILS"               , "1"),
        ("RUST_BACKTRACE"               , "1"),
        ("GOLEM__CLOUD_SERVICE__HOST", &cloud.private_host()),
        ("GOLEM__CLOUD_SERVICE__PORT", &cloud.private_grpc_port().to_string()),
        ("GOLEM__COMPONENT_STORE__TYPE", "Local"),
        ("GOLEM__COMPONENT_STORE__CONFIG__OBJECT_PREFIX", ""),
        ("GOLEM__COMPONENT_STORE__CONFIG__ROOT_PATH", "/tmp/ittest-local-object-store/golem-cloud"),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
        ("GOLEM__TRACING__STDOUT__JSON", "true"),
        ("GOLEM__CLOUD_SERVICE__ACCESS_TOKEN"     , ROOT_TOKEN),
    ];

    let mut vars: HashMap<String, String> =
        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())));

    match component_compilation_service {
        Some((host, port)) => {
            vars.insert(
                "GOLEM__COMPILATION__TYPE".to_string(),
                "Enabled".to_string(),
            );
            vars.insert(
                "GOLEM__COMPILATION__CONFIG__HOST".to_string(),
                host.to_string(),
            );
            vars.insert(
                "GOLEM__COMPILATION__CONFIG__PORT".to_string(),
                port.to_string(),
            );
        }
        _ => {
            vars.insert(
                "GOLEM__COMPILATION__TYPE".to_string(),
                "Disabled".to_string(),
            );
        }
    };

    vars.extend(rdb.info().env("component_service").clone());
    vars
}

#[derive(Debug)]
pub enum AddComponentError {
    AlreadyExists,
    Other(String),
}

impl Error for AddComponentError {}

impl Display for AddComponentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AddComponentError::AlreadyExists => write!(f, "Component already exists"),
            AddComponentError::Other(message) => write!(f, "{message}"),
        }
    }
}
