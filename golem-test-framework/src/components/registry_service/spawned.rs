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

use super::{wait_for_startup, RegistryService};
use crate::components::component_compilation_service::ComponentCompilationService;
use crate::components::rdb::Rdb;
use crate::components::{new_reqwest_client, ChildProcessLogger};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tracing::info;
use tracing::Level;
use uuid::uuid;

pub struct SpawnedRegistryService {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    base_http_client: OnceCell<reqwest::Client>,
    admin_account_id: AccountId,
    admin_account_email: String,
    admin_account_token: TokenSecret,
    default_plan_id: PlanId,
    low_fuel_plan_id: PlanId,
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
        let admin_account_email = "admin@golem.cloud".to_string();
        let admin_account_token =
            TokenSecret::trusted("lDL3DP2d7I3EbgfgJ9YEjVdEXNETpPkGYwyb36jgs28".to_string());
        let default_plan_id = PlanId(uuid!("8e3e354a-e45e-4e30-bae4-27c30c74d9ee"));
        let low_fuel_plan_id = PlanId(uuid!("301fd75c-dcc5-48e3-967e-e7c33df52493"));

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
                    &admin_plan_id,
                    &admin_account_id,
                    &admin_account_email,
                    &admin_account_token,
                    &default_plan_id,
                    &low_fuel_plan_id,
                    otlp,
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

        wait_for_startup("localhost", grpc_port, http_port, Duration::from_secs(90)).await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            base_http_client: OnceCell::new(),
            admin_account_id,
            admin_account_email,
            admin_account_token,
            default_plan_id,
            low_fuel_plan_id,
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
    fn admin_account_email(&self) -> String {
        self.admin_account_email.clone()
    }
    fn admin_account_token(&self) -> TokenSecret {
        self.admin_account_token.clone()
    }

    fn default_plan(&self) -> PlanId {
        self.default_plan_id
    }
    fn low_fuel_plan(&self) -> PlanId {
        self.low_fuel_plan_id
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
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
