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

//! One-time setup shared by every chaos scenario (GOL-363).
//!
//! Same idea as [density prep](crate::benchmarks::density::prep): create one
//! account, application and environment, upload the components once, and write a
//! manifest that every later invocation reloads. The manifest is also the resume
//! mechanism — a re-triggered run on the same commit reloads it instead of
//! rebuilding the world.
//!
//! Chaos needs two components at once, which no density section does: the
//! counters component (durable, ephemeral and scheduled streams) and the promise
//! component. They go into separate environments because both export agent types
//! that must resolve uniquely within an environment.

use anyhow::Context;
use chrono::{DateTime, Utc};
use golem_client::api::{RegistryServiceClient, ResourcesClient};
use golem_common::model::account::{AccountCreation, AccountEmail, AccountId};
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::auth::{TokenCreation, TokenSecret};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::{EnvironmentCreation, EnvironmentId, EnvironmentName};
use golem_common::model::quota::{
    EnforcementAction, ResourceDefinitionCreation, ResourceLimit, ResourceName, ResourceRateLimit,
    TimePeriod,
};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

/// WASM file name (without `.wasm`) of the counters component. Carries the
/// `Counter`, `EphemeralCounter`, `ScheduleEmitter` and `ScheduleCounter` agent
/// types the mixed workload drives.
pub const COUNTERS_WASM: &str = "it_agent_counters_release";
/// WASM file name of the Rust promise component.
pub const PROMISE_WASM: &str = "golem_it_promise_agent_rust_release";

/// Resource the quota stream reserves against. Must match `CHAOS_QUOTA_RESOURCE`
/// in the counters component — the agent names it when it takes its token.
pub const QUOTA_RESOURCE: &str = "chaos-quota";

/// Registry name of the counters component.
pub const COUNTERS_COMPONENT_NAME: &str = "chaos-counters";
/// Registry name of the promise component. Must match the name the WASM's
/// package declares, as the promise density prep does.
pub const PROMISE_COMPONENT_NAME: &str = "golem-it:promise-agent-rust";

/// Persisted record of a completed chaos prep.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChaosPrepManifest {
    pub run_id: String,
    /// The chaos account's token. In cloud mode this is the only credential a
    /// scenario invocation needs.
    pub token: TokenSecret,
    /// Environment holding the counters component.
    pub environment_id: EnvironmentId,
    pub counters_component_id: ComponentId,
    /// Environment holding the promise component. Separate because both
    /// components export agent types that must resolve uniquely per environment.
    pub promise_environment_id: EnvironmentId,
    pub promise_component_id: ComponentId,
}

impl ChaosPrepManifest {
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), json)
            .with_context(|| format!("writing chaos prep manifest to {:?}", path.as_ref()))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading chaos prep manifest from {:?}", path.as_ref()))?;
        Ok(serde_json::from_str(&json)?)
    }

    /// Rebuilds a user context from the stored token, so a scenario invocation
    /// needs no account lookup. The account id and email are not used by the
    /// invoke or component-read paths in cloud mode — the token authenticates —
    /// so placeholders stand in, matching what density prep does.
    pub fn user_context(
        &self,
        deps: &BenchmarkTestDependencies,
    ) -> TestUserContext<BenchmarkTestDependencies> {
        use std::sync::{Arc, RwLock};

        TestUserContext {
            deps: deps.clone(),
            account_id: AccountId(uuid::Uuid::nil()),
            account_email: AccountEmail::new(""),
            token: self.token.clone(),
            auto_deploy_enabled: false,
            name_cache: Arc::new(
                golem_test_framework::config::dsl_impl::NameResolutionCache::new(),
            ),
            last_deployments: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

/// Runs chaos prep, returning the manifest. Assumes a freshly wiped cluster.
pub async fn run_prep(deps: &BenchmarkTestDependencies) -> anyhow::Result<ChaosPrepManifest> {
    let run_id = deps
        .run_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "local".to_string());
    let prefix = deps.bench_name_prefix().unwrap_or_default();

    let admin_client = deps
        .registry_service()
        .client(&deps.registry_service().admin_account_token())
        .await;

    let account_base = format!("{prefix}chaos-bench");
    info!("Chaos-prep: creating account {account_base}");
    let account = admin_client
        .create_account(&AccountCreation {
            email: AccountEmail::new(format!("{account_base}@golem.cloud")),
            name: account_base.clone(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("create_account failed: {e:?}"))?;

    let token = admin_client
        .create_token(
            &account.id.0,
            &TokenCreation {
                expires_at: DateTime::<Utc>::MAX_UTC,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("create_token failed: {e:?}"))?;

    let manifest_token = token.secret.clone();
    let user = TestUserContext {
        deps: deps.clone(),
        account_id: account.id,
        account_email: account.email.clone(),
        token: token.secret,
        auto_deploy_enabled: false,
        name_cache: std::sync::Arc::new(
            golem_test_framework::config::dsl_impl::NameResolutionCache::new(),
        ),
        last_deployments: std::sync::Arc::new(std::sync::RwLock::new(HashMap::new())),
    };
    let user_client = deps.registry_service().client(&manifest_token).await;

    info!("Chaos-prep: creating application {account_base}-app");
    let app = user_client
        .create_application(
            &account.id.0,
            &ApplicationCreation {
                name: ApplicationName(format!("{account_base}-app")),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("create_application failed: {e:?}"))?;

    let counters_env = create_env(&user_client, &app.id.0, &format!("{account_base}-env")).await?;
    info!("Chaos-prep: uploading counters component {COUNTERS_COMPONENT_NAME}");
    let counters = user
        .component(&counters_env.id, COUNTERS_WASM)
        .name(COUNTERS_COMPONENT_NAME)
        .store()
        .await
        .context("uploading counters component")?;
    user.deploy_environment(counters_env.id)
        .await
        .context("deploying counters environment")?;

    // The quota stream needs a resource to hold a lease against, and the
    // registry is the only place to declare one: chaos-prep uploads WASM
    // directly rather than deploying a `golem.yaml`, so the manifest's
    // `resourceDefaults` never come into play.
    //
    // The limit is set far above what the workload can consume on purpose. This
    // stream exists to keep a *lease* alive — the executor renews it against the
    // shard-manager every 10s, which is the traffic the partition is meant to
    // cut. Rate-limiting the workload as well would confound the two: a refused
    // reservation would no longer distinguish "the lease was lost" from "the
    // quota was legitimately exhausted".
    let resources_client = deps
        .registry_service()
        .resources_client(&manifest_token)
        .await;
    info!("Chaos-prep: declaring quota resource {QUOTA_RESOURCE} on the counters environment");
    resources_client
        .create_resource(
            &counters_env.id.0,
            &ResourceDefinitionCreation {
                name: ResourceName(QUOTA_RESOURCE.to_string()),
                limit: ResourceLimit::Rate(ResourceRateLimit {
                    value: 1_000_000,
                    period: TimePeriod::Second,
                    max: 1_000_000,
                }),
                // Reject rather than throttle or terminate: a refused
                // reservation is an observation the driver can record and carry
                // on from. Throttling would silently stretch latency instead,
                // and terminating would take the agent out entirely.
                enforcement_action: EnforcementAction::Reject,
                unit: "operation".to_string(),
                units: "operations".to_string(),
            },
        )
        .await
        // Name the endpoint. The first failure here was a bare
        // `Decode("EOF while parsing a value")` from a client pointed at the
        // wrong base URL, which said nothing about where it had been looking.
        .map_err(|e| {
            anyhow::anyhow!(
                "create_resource({QUOTA_RESOURCE}) on environment {} failed: {e:?}",
                counters_env.id.0
            )
        })?;

    let promise_env = create_env(
        &user_client,
        &app.id.0,
        &format!("{account_base}-promise-env"),
    )
    .await?;
    info!("Chaos-prep: uploading promise component {PROMISE_COMPONENT_NAME}");
    let promise = user
        .component(&promise_env.id, PROMISE_WASM)
        .name(PROMISE_COMPONENT_NAME)
        .store()
        .await
        .context("uploading promise component")?;
    user.deploy_environment(promise_env.id)
        .await
        .context("deploying promise environment")?;

    let manifest = ChaosPrepManifest {
        run_id,
        token: manifest_token,
        environment_id: counters_env.id,
        counters_component_id: counters.id,
        promise_environment_id: promise_env.id,
        promise_component_id: promise.id,
    };

    info!(
        "Chaos-prep complete: counters_env={}, counters={}, promise_env={}, promise={}",
        manifest.environment_id.0,
        manifest.counters_component_id.0,
        manifest.promise_environment_id.0,
        manifest.promise_component_id.0
    );

    Ok(manifest)
}

async fn create_env(
    client: &golem_client::api::RegistryServiceClientLive,
    app_id: &uuid::Uuid,
    name: &str,
) -> anyhow::Result<golem_common::model::environment::Environment> {
    client
        .create_environment(
            app_id,
            &EnvironmentCreation {
                name: EnvironmentName(name.to_string()),
                compatibility_check: false,
                version_check: false,
                security_overrides: false,
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("create_environment failed: {e:?}"))
}
