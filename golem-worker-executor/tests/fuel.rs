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

use async_trait::async_trait;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentTypeName, RegisteredAgentType};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::AgentId;
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::custom_api::CompiledRoutes;
use golem_service_base::mcp::CompiledMcp;
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment};
use golem_service_base::model::component::Component;
use golem_service_base::model::environment::EnvironmentState;
use golem_service_base::model::{AccountResourceLimits, ResourceLimits as ServiceResourceLimits};
use golem_worker_executor::services::resource_limits::{ResourceLimits, ResourceLimitsGrpc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use test_r::test;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A registry mock whose batch response can be updated between calls.
/// `get_resource_limits` always returns the initial limits (used at
/// `initialize_account` time). `batch_update_fuel_usage` returns whatever
/// limits have been set via `set_batch_response`, allowing tests to simulate
/// the server confirming a new balance after consumption.
struct MockRegistry {
    initial_limits: ServiceResourceLimits,
    batch_response: Mutex<AccountResourceLimits>,
}

impl MockRegistry {
    fn new(available_fuel: u64, max_memory: u64) -> Arc<Self> {
        Arc::new(Self {
            initial_limits: ServiceResourceLimits {
                available_fuel,
                max_memory_per_worker: max_memory,
            },
            batch_response: Mutex::new(AccountResourceLimits(HashMap::new())),
        })
    }

    /// Configure what the server returns for a given account on the next
    /// `batch_update_fuel_usage` call.
    fn set_batch_response(&self, id: AccountId, available_fuel: u64, max_memory: u64) {
        let mut map = HashMap::new();
        map.insert(
            id,
            ServiceResourceLimits {
                available_fuel,
                max_memory_per_worker: max_memory,
            },
        );
        *self.batch_response.lock().unwrap() = AccountResourceLimits(map);
    }
}

#[async_trait]
impl RegistryService for MockRegistry {
    async fn authenticate_token(&self, _: &TokenSecret) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_auth_details_for_environment(
        &self,
        _: EnvironmentId,
        _: bool,
        _: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_resource_limits(
        &self,
        _: AccountId,
    ) -> Result<ServiceResourceLimits, RegistryServiceError> {
        Ok(self.initial_limits.clone())
    }
    async fn update_worker_limit(
        &self,
        _: AccountId,
        _: &AgentId,
        _: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }
    async fn update_worker_connection_limit(
        &self,
        _: AccountId,
        _: &AgentId,
        _: bool,
    ) -> Result<(), RegistryServiceError> {
        unimplemented!()
    }
    async fn batch_update_fuel_usage(
        &self,
        _: HashMap<AccountId, i64>,
    ) -> Result<AccountResourceLimits, RegistryServiceError> {
        Ok(self.batch_response.lock().unwrap().clone())
    }
    async fn download_component(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<u8>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_component_metadata(
        &self,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_deployed_component_metadata(
        &self,
        _: ComponentId,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_deployed_component_revisions(
        &self,
        _: ComponentId,
    ) -> Result<Vec<Component>, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_component(
        &self,
        _: AccountId,
        _: ApplicationId,
        _: EnvironmentId,
        _: &str,
    ) -> Result<Component, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_all_agent_types(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
    ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_agent_type(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_latest_agent_type_by_names(
        &self,
        _: &AccountId,
        _: &ApplicationName,
        _: &EnvironmentName,
        _: &AgentTypeName,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_agent_type_at_deployment(
        &self,
        _: &AccountId,
        _: &ApplicationName,
        _: &EnvironmentName,
        _: &AgentTypeName,
        _: DeploymentRevision,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn resolve_agent_type_by_names(
        &self,
        _: &ApplicationName,
        _: &EnvironmentName,
        _: &AgentTypeName,
        _: Option<DeploymentRevision>,
        _: Option<&str>,
        _: &AuthCtx,
    ) -> Result<RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_routes_for_domain(
        &self,
        _: &Domain,
    ) -> Result<CompiledRoutes, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_active_compiled_mcps_for_domain(
        &self,
        _: &Domain,
    ) -> Result<CompiledMcp, RegistryServiceError> {
        unimplemented!()
    }
    async fn get_current_environment_state(
        &self,
        _: EnvironmentId,
    ) -> Result<EnvironmentState, RegistryServiceError> {
        unimplemented!()
    }
}

const BATCH_INTERVAL: Duration = Duration::from_millis(50);
const IDLE_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
/// Wait long enough for at least two full batch cycles to complete.
const BATCH_WAIT: Duration = Duration::from_millis(300);
/// Wait long enough for the idle refresh threshold to elapse and a batch to fire.
const IDLE_REFRESH_WAIT: Duration = Duration::from_millis(400);

fn account_id() -> AccountId {
    AccountId(Uuid::new_v4())
}

/// Creates a `ResourceLimitsGrpc` with short intervals so the background task
/// fires quickly. Returns the service and a shutdown token to cancel it after
/// the test.
fn make_svc(registry: Arc<MockRegistry>) -> (Arc<ResourceLimitsGrpc>, CancellationToken) {
    let token = CancellationToken::new();
    let svc = ResourceLimitsGrpc::new(
        registry,
        BATCH_INTERVAL,
        IDLE_REFRESH_INTERVAL,
        token.clone(),
    );
    (svc, token)
}

#[test]
async fn borrow_fails_after_batch_confirms_account_exhausted() {
    // The server returns available_fuel=0 after the batch cycle.
    // After waiting for a batch to fire, further borrows must fail.
    let registry = MockRegistry::new(10_000, 512);
    let id = account_id();
    registry.set_batch_response(id, 0, 512);

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.borrow_fuel(1_000);
    sleep(BATCH_WAIT).await;

    assert!(
        !entry.borrow_fuel(1),
        "borrow must fail after server confirms exhaustion"
    );

    token.cancel();
}

#[test]
async fn borrow_succeeds_after_batch_confirms_remaining_budget() {
    // The server confirms the account still has budget. Borrowing continues.
    let registry = MockRegistry::new(10_000, 512);
    let id = account_id();
    registry.set_batch_response(id, 8_000, 512);

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.borrow_fuel(2_000);
    sleep(BATCH_WAIT).await;

    assert!(
        entry.borrow_fuel(1_000),
        "borrow must succeed when server confirms remaining budget"
    );

    token.cancel();
}

#[test]
async fn return_fuel_reduces_consumption_reported_to_server() {
    // Borrowing 5_000 then returning 4_000 unused means only 1_000 was consumed.
    // The server reflects this by confirming a higher remaining balance.
    // After the batch, borrowing a large amount must succeed.
    let registry = MockRegistry::new(10_000, 512);
    let id = account_id();
    // Server sees net consumption ≈ 1_000 → returns 9_000 remaining
    registry.set_batch_response(id, 9_000, 512);

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.borrow_fuel(5_000);
    entry.return_fuel(4_000); // return unused portion
    sleep(BATCH_WAIT).await;

    // After batch: server-confirmed 9_000 available → large borrow must succeed
    assert!(entry.borrow_fuel(5_000));

    token.cancel();
}

#[test]
async fn same_account_returns_shared_entry() {
    let registry = MockRegistry::new(10_000, 512);
    let (svc, token) = make_svc(registry);
    let id = account_id();

    let entry1 = svc.initialize_account(id).await.unwrap();
    let entry2 = svc.initialize_account(id).await.unwrap();

    assert!(Arc::ptr_eq(&entry1, &entry2));
    token.cancel();
}

#[test]
async fn different_accounts_have_independent_entries() {
    let registry = MockRegistry::new(10_000, 512);
    let (svc, token) = make_svc(registry);

    let entry_a = svc.initialize_account(account_id()).await.unwrap();
    let entry_b = svc.initialize_account(account_id()).await.unwrap();

    assert!(!Arc::ptr_eq(&entry_a, &entry_b));
    token.cancel();
}

#[test]
async fn two_workers_joint_consumption_exhausts_shared_account() {
    // Both workers consume from the shared entry. The server sees the combined
    // delta and confirms the account is exhausted.
    let registry = MockRegistry::new(10_000, 512);
    let id = account_id();
    registry.set_batch_response(id, 0, 512); // joint consumption exhausted the account

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.borrow_fuel(5_000); // worker A
    entry.borrow_fuel(5_000); // worker B
    sleep(BATCH_WAIT).await;

    assert!(
        !entry.borrow_fuel(1),
        "both workers' consumption must jointly exhaust the account"
    );

    token.cancel();
}

#[test]
async fn exhausting_one_account_does_not_affect_another() {
    let registry = MockRegistry::new(10_000, 512);
    let id_a = account_id();
    let id_b = account_id();
    // Only account A is exhausted by the server
    registry.set_batch_response(id_a, 0, 512);

    let (svc, token) = make_svc(registry);
    let entry_a = svc.initialize_account(id_a).await.unwrap();
    let entry_b = svc.initialize_account(id_b).await.unwrap();

    entry_a.borrow_fuel(1_000);
    sleep(BATCH_WAIT).await;

    assert!(!entry_a.borrow_fuel(1), "account A must be exhausted");
    assert!(entry_b.borrow_fuel(1_000), "account B must be unaffected");

    token.cancel();
}

#[test]
async fn idle_account_picks_up_plan_upgrade_after_refresh_interval() {
    // An idle account (no borrows) with a low initial limit eventually sees a
    // plan upgrade when the idle refresh batch fires. After the upgrade the
    // account can borrow amounts beyond the original limit.
    let registry = MockRegistry::new(1_000, 512);
    let id = account_id();
    // Plan upgrade: server now grants 100_000
    registry.set_batch_response(id, 100_000, 512);

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    // Wait for both the idle refresh threshold (100ms) and a batch cycle (50ms)
    sleep(IDLE_REFRESH_WAIT).await;

    // The upgraded limit must now be in effect
    assert!(
        entry.borrow_fuel(50_000),
        "borrow must succeed after idle refresh picks up plan upgrade"
    );

    token.cancel();
}

#[test]
async fn max_memory_limit_reflects_server_value_after_batch() {
    let registry = MockRegistry::new(10_000, 512);
    let id = account_id();
    registry.set_batch_response(id, 9_000, 2048);

    let (svc, token) = make_svc(registry);
    let entry = svc.initialize_account(id).await.unwrap();

    entry.borrow_fuel(1_000);
    sleep(BATCH_WAIT).await;

    assert_eq!(entry.max_memory_limit(), 2048);
    token.cancel();
}

#[test]
async fn initial_max_memory_limit_reflects_registry_value() {
    let registry = MockRegistry::new(10_000, 4096);
    let (svc, token) = make_svc(registry);

    let entry = svc.initialize_account(account_id()).await.unwrap();
    assert_eq!(entry.max_memory_limit(), 4096);

    token.cancel();
}
