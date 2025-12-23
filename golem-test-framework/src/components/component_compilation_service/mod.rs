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

use crate::components::{wait_for_startup_grpc, EnvVarBuilder};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use tracing::Level;

pub mod provided;
pub mod spawned;

#[async_trait]
pub trait ComponentCompilationService: Send + Sync {
    fn grpc_host(&self) -> String;
    fn grpc_port(&self) -> u16;

    async fn kill(&self);
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(
        host,
        grpc_port,
        "golem-component-compilation-service",
        timeout,
    )
    .await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    verbosity: Level,
    enable_fs_cache: bool,
    otlp: bool,
) -> HashMap<String, String> {
    EnvVarBuilder::golem_service(verbosity)
        .with_str("GOLEM__COMPILED_COMPONENT_SERVICE__TYPE", "Enabled")
        .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
        .with_str("GOLEM__REGISTRY_SERVICE__TYPE", "Dynamic")
        .with("GOLEM__ENGINE__ENABLE_FS_CACHE", "true".to_string())
        .with(
            "GOLEM__ENGINE__ENABLE_FS_CACHE",
            enable_fs_cache.to_string(),
        )
        .with("GOLEM__GRPC__PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_optional_otlp("component_compilation_service", otlp)
        .build()
}
