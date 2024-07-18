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

use async_trait::async_trait;
use golem_test_framework::components::component_compilation_service::ComponentCompilationServiceEnvVars;
use std::sync::Arc;

use tracing::Level;

use crate::components::{CloudEnvVars, ROOT_TOKEN};

#[async_trait]
impl ComponentCompilationServiceEnvVars for CloudEnvVars {
    async fn env_vars(
        &self,
        http_port: u16,
        grpc_port: u16,
        component_service: Arc<
            dyn golem_test_framework::components::component_service::ComponentService
                + Send
                + Sync
                + 'static,
        >,
        verbosity: Level,
    ) -> HashMap<String, String> {
        let log_level = verbosity.as_str().to_lowercase();

        let vars: &[(&str, &str)] = &[
            ("ENVIRONMENT", "dev"),
            ("RUST_LOG", &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("WASMTIME_BACKTRACE_DETAILS", "1"),
            ("RUST_BACKTRACE", "1"),
            ("GOLEM__COMPONENT_SERVICE__HOST", &component_service.private_host()),
            ("GOLEM__COMPONENT_SERVICE__PORT", &component_service.private_grpc_port().to_string()),
            ("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem"),
            ("GOLEM__BLOB_STORAGE__CONFIG__ROOT", "/tmp/ittest-local-object-store/golem-cloud"),
            ("GOLEM__TRACING__STDOUT__JSON", "true"),
            ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
            ("GOLEM__HTTP_PORT", &http_port.to_string()),
            ("GOLEM__COMPONENT_SERVICE__ACCESS_TOKEN"     , ROOT_TOKEN),
        ];

        HashMap::from_iter(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }
}
