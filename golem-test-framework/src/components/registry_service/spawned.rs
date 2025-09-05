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

use super::RegistryService;
use crate::components::blob_storage::BlobStorageInfo;
use crate::components::rdb::DbInfo;
use crate::components::{new_reqwest_client, wait_for_startup_http};
use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::account::PlanId;
use golem_common::model::auth::{AccountRole, TokenSecret};
use golem_registry_service::config::{
    AccountsConfig, PlansConfig, PrecreatedAccount, PrecreatedPlan, RegistryServiceConfig,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::OnceCell;
use tokio::task::JoinSet;
use tracing::info;
use uuid::uuid;

const ADMIN_ACCOUNT_NAME: &str = "Admin";
const ADMIN_ACCOUNT_EMAIL: &str = "admin@golem.cloud";

pub struct SpawnedRegistyService {
    join_set: Option<JoinSet<anyhow::Result<()>>>,
    run_details: golem_registry_service::RunDetails,
    base_http_client: OnceCell<reqwest::Client>,
    admin_account_id: AccountId,
    admin_account_email: String,
    admin_account_token: TokenSecret,
}

impl SpawnedRegistyService {
    pub async fn new(
        db_info: &DbInfo,
        blob_storage_info: &BlobStorageInfo,
    ) -> anyhow::Result<Self> {
        info!("Starting golem-registry-service process");

        let mut join_set = JoinSet::new();

        let admin_plan_id = PlanId::new_v4();
        let admin_account_id = AccountId::new_v4();
        let admin_account_token = TokenSecret::new_v4();

        let config = make_config(
            db_info,
            blob_storage_info,
            admin_plan_id,
            admin_account_id.clone(),
            admin_account_token.clone(),
        );

        let prometheus_registry = prometheus::Registry::new();

        let service =
            golem_registry_service::RegistryService::new(config, prometheus_registry).await?;

        let run_details = service.start(&mut join_set).await?;

        wait_for_startup_http(
            "localhost",
            run_details.http_port,
            "registry-service",
            Duration::from_secs(10),
        )
        .await;

        Ok(Self {
            run_details,
            join_set: Some(join_set),
            base_http_client: OnceCell::new(),
            admin_account_id,
            admin_account_email: ADMIN_ACCOUNT_EMAIL.to_string(),
            admin_account_token,
        })
    }
}

#[async_trait]
impl RegistryService for SpawnedRegistyService {
    fn http_host(&self) -> String {
        "localhost".to_string()
    }

    fn http_port(&self) -> u16 {
        self.run_details.http_port
    }

    fn grpc_host(&self) -> String {
        "localhost".to_string()
    }

    fn gprc_port(&self) -> u16 {
        // self.run_details.grpc_port
        todo!()
    }

    fn admin_account_id(&self) -> AccountId {
        self.admin_account_id.clone()
    }

    fn admin_account_email(&self) -> String {
        self.admin_account_email.clone()
    }

    fn admin_account_token(&self) -> TokenSecret {
        self.admin_account_token.clone()
    }

    async fn kill(&mut self) {
        if let Some(mut join_set) = self.join_set.take() {
            join_set.abort_all();
            join_set.join_all().await;
        };
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }
}

fn make_config(
    db_info: &DbInfo,
    blob_storage_info: &BlobStorageInfo,
    admin_plan_id: PlanId,
    admin_account_id: AccountId,
    admin_token: TokenSecret,
) -> RegistryServiceConfig {
    RegistryServiceConfig {
        db: db_info.config("golem_component", false),
        blob_storage: blob_storage_info.config(),
        grpc_port: 0,
        http_port: 0,
        plans: PlansConfig {
            plans: HashMap::from_iter([
                (
                    "unlimited".to_string(),
                    PrecreatedPlan {
                        plan_id: admin_plan_id.0,
                        app_limit: i64::MAX,
                        env_limit: i64::MAX,
                        component_limit: i64::MAX,
                        worker_limit: i64::MAX,
                        storage_limit: i64::MAX,
                        monthly_gas_limit: i64::MAX,
                        monthly_upload_limit: i64::MAX,
                    },
                ),
                (
                    "default".to_string(),
                    PrecreatedPlan {
                        plan_id: uuid!("157dc684-00eb-496d-941c-da8fd1d15c63"),
                        app_limit: 10,
                        env_limit: 40,
                        component_limit: 100,
                        worker_limit: 10000,
                        storage_limit: 500000000,
                        monthly_gas_limit: 1000000000000,
                        monthly_upload_limit: 1000000000,
                    },
                ),
            ]),
        },
        accounts: AccountsConfig {
            accounts: HashMap::from_iter([(
                "admin".to_string(),
                PrecreatedAccount {
                    id: admin_account_id.0,
                    name: ADMIN_ACCOUNT_NAME.to_string(),
                    email: ADMIN_ACCOUNT_EMAIL.to_string(),
                    token: admin_token.0,
                    plan_id: admin_plan_id.0,
                    role: AccountRole::Admin,
                },
            )]),
        },
        ..Default::default()
    }
}
