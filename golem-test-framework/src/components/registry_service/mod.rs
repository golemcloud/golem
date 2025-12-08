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

pub mod provided;
pub mod spawned;

use super::component_compilation_service::ComponentCompilationService;
use super::rdb::Rdb;
use super::{wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder};
use async_trait::async_trait;
use golem_client::api::RegistryServiceClientLive;
use golem_client::{Context, Security};
use golem_common::model::account::AccountId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::plan::PlanId;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;
use url::Url;

#[async_trait]
pub trait RegistryService: Send + Sync {
    fn http_host(&self) -> String;
    fn http_port(&self) -> u16;

    fn grpc_host(&self) -> String;
    fn grpc_port(&self) -> u16;

    fn admin_account_id(&self) -> AccountId;
    fn admin_account_email(&self) -> String;
    fn admin_account_token(&self) -> TokenSecret;

    fn default_plan(&self) -> PlanId;
    fn low_fuel_plan(&self) -> PlanId;

    async fn kill(&self);

    async fn base_http_client(&self) -> reqwest::Client;

    async fn client(&self, token: &TokenSecret) -> RegistryServiceClientLive {
        let url = format!("http://{}:{}", self.http_host(), self.http_port());
        RegistryServiceClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.secret().to_string()),
            },
        }
    }
}

async fn wait_for_startup(host: &str, grpc_port: u16, http_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "golem-registry-service", timeout).await;
    wait_for_startup_http(host, http_port, "golem-registry-service", timeout).await;
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    rdb: &Arc<dyn Rdb>,
    rdb_private_connection: bool,
    component_compilation_service: Option<&Arc<dyn ComponentCompilationService>>,
    verbosity: Level,
    admin_plan_id: &PlanId,
    admin_account_id: &AccountId,
    admin_account_email: &str,
    admin_token: &TokenSecret,
    default_plan_id: &PlanId,
    low_fuel_plan_id: &PlanId,
    otlp: bool,
) -> HashMap<String, String> {
    let builder = EnvVarBuilder::golem_service(verbosity)
        .with_str("GOLEM__BLOB_STORAGE__TYPE", "LocalFileSystem")
        .with_str(
            "GOLEM__BLOB_STORAGE__CONFIG__ROOT",
            "/tmp/ittest-local-object-store/golem",
        )
        .with("GOLEM__LOGIN__TYPE", "Disabled".to_string());

    // component compilation
    let builder = match component_compilation_service {
        Some(component_compilation_service) => builder
            .with("GOLEM__COMPONENT_COMPILATION__TYPE", "Enabled".to_string())
            .with(
                "GOLEM__COMPONENT_COMPILATION__CONFIG__HOST",
                component_compilation_service.grpc_host(),
            )
            .with(
                "GOLEM__COMPONENT_COMPILATION__CONFIG__PORT",
                component_compilation_service.grpc_port().to_string(),
            ),
        _ => builder.with_str("GOLEM__COMPONENT_COMPILATION__TYPE", "Disabled"),
    };

    builder
        // users
        .with(
            "GOLEM__INITIAL_ACCOUNTS__ROOT__ID",
            admin_account_id.to_string(),
        )
        .with(
            "GOLEM__INITIAL_ACCOUNTS__ROOT__EMAIL",
            admin_account_email.to_string(),
        )
        .with(
            "GOLEM__INITIAL_ACCOUNTS__ROOT__PLAN_ID",
            admin_plan_id.to_string(),
        )
        .with(
            "GOLEM__INITIAL_ACCOUNTS__ROOT__TOKEN",
            admin_token.secret().to_string(),
        )
        // plans
        .with(
            "GOLEM__INITIAL_PLANS__DEFAULT__PLAN_ID",
            default_plan_id.to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__DEFAULT__WORKER_LIMIT",
            "100000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__PLAN_ID",
            admin_plan_id.to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__PLAN_NAME",
            "unlimited".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__APP_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__COMPONENT_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__ENV_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__MAX_MEMORY_PER_WORKER",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__MONTHLY_GAS_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__MONTHLY_UPLOAD_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__STORAGE_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__WORKER_CONNECTION_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__UNLIMITED__WORKER_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__PLAN_ID",
            low_fuel_plan_id.to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__PLAN_NAME",
            "low_fuel".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__APP_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__COMPONENT_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__ENV_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__MAX_MEMORY_PER_WORKER",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__MONTHLY_GAS_LIMIT",
            "1".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__MONTHLY_UPLOAD_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__STORAGE_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__WORKER_CONNECTION_LIMIT",
            "10000000000000000".to_string(),
        )
        .with(
            "GOLEM__INITIAL_PLANS__LOW_FUEL__WORKER_LIMIT",
            "10000000000000000".to_string(),
        )
        //
        .with("GOLEM__GRPC_PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with_all(rdb.info().env("golem_registry", rdb_private_connection))
        .with_optional_otlp("registry_service", otlp)
        .build()
}
