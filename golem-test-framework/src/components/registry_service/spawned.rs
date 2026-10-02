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

use super::{RegistryService, wait_for_startup};
use crate::components::ChildProcessLogger;
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::new_reqwest_client_with_tracing;
use crate::components::rdb::Rdb;
use async_trait::async_trait;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::Level;
use tracing::info;
use uuid::uuid;

pub struct SpawnedRegistryService {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    admin_account_id: AccountId,
    admin_account_email: AccountEmail,
    admin_account_token: TokenSecret,
    builtin_plugin_owner_account_id: AccountId,
    default_plan_id: PlanId,
    low_fuel_plan_id: PlanId,
    low_disk_space_plan_id: PlanId,
    low_http_calls_plan_id: PlanId,
    low_rpc_calls_plan_id: PlanId,
    base_http_client: OnceCell<reqwest_middleware::ClientWithMiddleware>,
}

impl SpawnedRegistryService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        rdb: &Arc<dyn Rdb>,
        component_compilation_service: Option<&Arc<dyn ComponentCompilationService>>,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
        otlp: bool,
    ) -> Self {
        info!("Starting golem-registry-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled golem-registry-service at {executable:?}");
        }

        let admin_plan_id = PlanId(uuid!("157dc684-00eb-496d-941c-da8fd1d15c63"));
        let admin_account_id = AccountId(uuid!("e71a6160-4144-4720-9e34-e5943458d129"));
        let admin_account_email = AccountEmail::new("admin@golem.cloud");
        let admin_account_token =
            TokenSecret::trusted("lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28".to_string());

        let builtin_plugin_owner_account_id =
            AccountId(uuid!("b7d3b9fb-ca74-4f75-8d20-c2ce03f5871d"));

        let default_plan_id = PlanId(uuid!("8e3e354a-e45e-4e30-bae4-27c30c74d9ee"));
        let low_fuel_plan_id = PlanId(uuid!("301fd75c-dcc5-48e3-967e-e7c33df52493"));
        let low_disk_space_plan_id = PlanId(uuid!("a2f3b4c5-d6e7-8901-abcd-ef0123456789"));
        let low_http_calls_plan_id = PlanId(uuid!("b3c4d5e6-f7a8-9012-bcde-f01234567890"));
        let low_rpc_calls_plan_id = PlanId(uuid!("c4d5e6f7-a8b9-0123-cdef-012345678901"));

        let repository_root = working_directory
            .parent()
            .expect("registry service working directory is inside the repository");
        let builtin_artifact_cache_dir =
            repository_root.join("target/integration-builtin-artifacts");
        prepopulate_builtin_artifact_cache(repository_root, &builtin_artifact_cache_dir);

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(
                super::env_vars(
                    http_port,
                    grpc_port,
                    rdb,
                    false,
                    component_compilation_service,
                    verbosity,
                    admin_plan_id,
                    admin_account_id,
                    &admin_account_email,
                    &admin_account_token,
                    builtin_plugin_owner_account_id,
                    default_plan_id,
                    low_fuel_plan_id,
                    low_disk_space_plan_id,
                    low_http_calls_plan_id,
                    low_rpc_calls_plan_id,
                    otlp,
                    &builtin_artifact_cache_dir,
                )
                .await,
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-component-service");

        let logger = ChildProcessLogger::log_child_process(
            "[registry-service]",
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup(
            "localhost",
            grpc_port,
            http_port,
            Duration::from_secs(90),
            Some(&mut child),
        )
        .await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            admin_account_id,
            admin_account_email,
            admin_account_token,
            builtin_plugin_owner_account_id,
            default_plan_id,
            low_fuel_plan_id,
            low_disk_space_plan_id,
            low_http_calls_plan_id,
            low_rpc_calls_plan_id,
            base_http_client: OnceCell::new(),
        }
    }
}

fn prepopulate_builtin_artifact_cache(repository_root: &Path, cache_dir: &Path) {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(repository_root.join("builtin-artifacts.lock.json"))
            .expect("failed to read builtin-artifacts.lock.json"),
    )
    .expect("failed to parse builtin-artifacts.lock.json");
    let artifacts = manifest["artifacts"]
        .as_object()
        .expect("builtin-artifacts.lock.json must contain an artifacts object");
    let local_artifacts = [
        ("filesystem_tools", "builtin-tools/filesystem-tools.wasm"),
        ("javascript_tools", "builtin-tools/javascript-tools.wasm"),
        ("otlp_exporter", "plugins/otlp-exporter.wasm"),
        ("typescript_tools", "builtin-tools/typescript-tools.wasm"),
        ("web_fetch", "builtin-tools/web-fetch.wasm"),
    ];

    std::fs::create_dir_all(cache_dir).expect("failed to create built-in artifact test cache");
    for (artifact_id, relative_path) in local_artifacts {
        let source = repository_root.join(relative_path);
        if !source.is_file() {
            continue;
        }
        let expected = artifacts[artifact_id]["sha256"]
            .as_str()
            .expect("default built-in artifacts must have a SHA-256");
        let destination = cache_dir.join(format!("{expected}.wasm"));
        if destination.is_file() {
            continue;
        }

        let bytes = std::fs::read(&source)
            .unwrap_or_else(|error| panic!("failed to read '{}': {error}", source.display()));
        let actual = hex::encode(Sha256::digest(&bytes));
        assert_eq!(
            actual,
            expected,
            "locally built artifact '{}' does not match builtin-artifacts.lock.json",
            source.display()
        );
        let mut temporary = tempfile::NamedTempFile::new_in(cache_dir)
            .expect("failed to create temporary built-in artifact cache file");
        temporary
            .write_all(&bytes)
            .expect("failed to write temporary built-in artifact cache file");
        match temporary.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => panic!(
                "failed to prepopulate built-in artifact '{}': {}",
                destination.display(),
                error.error
            ),
        }
    }
}

#[async_trait]
impl RegistryService for SpawnedRegistryService {
    fn http_host(&self) -> String {
        "localhost".to_string()
    }
    fn http_port(&self) -> u16 {
        self.http_port
    }

    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }
    fn grpc_port(&self) -> u16 {
        self.grpc_port
    }

    fn admin_account_id(&self) -> AccountId {
        self.admin_account_id
    }
    fn admin_account_email(&self) -> AccountEmail {
        self.admin_account_email.clone()
    }
    fn admin_account_token(&self) -> TokenSecret {
        self.admin_account_token.clone()
    }

    fn builtin_plugin_owner_account_id(&self) -> AccountId {
        self.builtin_plugin_owner_account_id
    }

    fn default_plan(&self) -> PlanId {
        self.default_plan_id
    }
    fn low_fuel_plan(&self) -> PlanId {
        self.low_fuel_plan_id
    }

    fn low_disk_space_plan(&self) -> PlanId {
        self.low_disk_space_plan_id
    }

    fn low_http_calls_plan(&self) -> PlanId {
        self.low_http_calls_plan_id
    }

    fn low_rpc_calls_plan(&self) -> PlanId {
        self.low_rpc_calls_plan_id
    }

    async fn base_http_client(&self) -> reqwest_middleware::ClientWithMiddleware {
        self.base_http_client
            .get_or_init(|| async { new_reqwest_client_with_tracing() })
            .await
            .clone()
    }

    async fn kill(&self) {
        info!("Stopping golem-registry-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedRegistryService {
    fn drop(&mut self) {
        info!("Stopping golem-registry-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
