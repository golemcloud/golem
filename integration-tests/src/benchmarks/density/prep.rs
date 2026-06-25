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

//! Density-prep tooling (golemcloud/golem#3522).
//!
//! Shared setup every density section depends on: creates one density account,
//! one application, one environment, uploads the section's components once, and
//! deploys the environment pinning every component.
//!
//! The buildspec runs prep exactly once at suite start (against a freshly-wiped
//! cluster) and writes the resulting [`PrepManifest`] to a file that is
//! uploaded to S3 and passed to every per-cell invocation via
//! `--prep-manifest`. The manifest carries the account token and all component
//! IDs so per-cell invocations need no by-name lookup and no re-tokenization —
//! this is also the resume mechanism: a resumed run reloads the same manifest.
//!
//! Idempotency: prep assumes a freshly-wiped cluster. It is not re-run on
//! resume; the manifest is reloaded instead.

use crate::benchmarks::density::DensitySection;
use anyhow::Context;
use chrono::{DateTime, Utc};
use golem_client::api::RegistryServiceClient;
use golem_common::model::account::{AccountCreation, AccountEmail, AccountId};
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::auth::{TokenCreation, TokenSecret};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::{EnvironmentCreation, EnvironmentId, EnvironmentName};
use golem_test_framework::config::dsl_impl::TestUserContext;
use golem_test_framework::config::{BenchmarkTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tracing::info;

/// WASM file name (without `.wasm`) of the agent-counters component, used by
/// the agent-density section for both the shared component and the per-agent
/// distinct uploads.
pub const AGENT_COUNTERS_WASM: &str = "it_agent_counters_release";

/// Number of distinct per-agent components uploaded for agent-density (one
/// component per agent in the per-agent-sharing cells).
///
/// All are byte-identical uploads of the agent-counters WASM under distinct
/// names; the registry mints distinct `component_id`s, producing
/// compiled-component-cache thrash on the executor. The executor's compiled
/// cache defaults to 32 entries, so even a few hundred distinct components
/// thrash it hard — the per-agent ceiling is reached well below this count.
///
/// The first cloud pass caps per-agent-component cells at 2000 concurrent
/// agents/components.
pub const PER_AGENT_COMPONENT_COUNT: u32 = 2000;

/// Registry name of the single shared agent-density component (the
/// shared-component sharing mode, labelled `U`).
pub const UNIFORM_COMPONENT_NAME: &str = "density-counter-uniform";

/// Builds the registry name of the `index`-th (1-based) per-agent distinct
/// component: `density-counter-distinct-0001` ..
/// `density-counter-distinct-2000`.
pub fn distinct_component_name(index: u32) -> String {
    format!("density-counter-distinct-{index:04}")
}

/// Persisted record of a completed density-prep, written by the prep step and
/// reloaded by every per-cell invocation (and on resume).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepManifest {
    pub section: String,
    pub run_id: String,
    /// Token secret for the density account. In cloud mode this is the only
    /// credential a per-cell invocation needs — it authenticates the invoke and
    /// component-read calls. The account id/email are not needed at run time.
    pub token: TokenSecret,
    pub environment_id: EnvironmentId,
    /// The shared component's id (used by shared-component cells). `None` for
    /// sections that have no shared component.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uniform_component_id: Option<ComponentId>,
    /// The per-agent distinct component ids, in upload order (1-based index ->
    /// position). Used by per-agent-sharing cells. Empty for sections without a
    /// per-agent sharing mode.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub distinct_component_ids: Vec<ComponentId>,
}

impl PrepManifest {
    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path.as_ref(), json)
            .with_context(|| format!("writing prep manifest to {:?}", path.as_ref()))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("reading prep manifest from {:?}", path.as_ref()))?;
        let manifest = serde_json::from_str(&json)?;
        Ok(manifest)
    }

    /// Reconstructs a [`TestUserContext`] from the stored token so per-cell
    /// invocations can invoke agents and read components without re-creating
    /// the account. The account id/email are not used by the invoke or
    /// component-read paths in cloud mode (the token authenticates), so
    /// placeholders are used.
    pub fn user_context(
        &self,
        deps: &BenchmarkTestDependencies,
    ) -> TestUserContext<BenchmarkTestDependencies> {
        use std::collections::HashMap as Map;
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
            last_deployments: Arc::new(RwLock::new(Map::new())),
        }
    }
}

/// Runs density-prep for `section`, returning the manifest. Assumes a freshly
/// wiped cluster.
pub async fn run_prep(
    deps: &BenchmarkTestDependencies,
    section: DensitySection,
) -> anyhow::Result<PrepManifest> {
    let run_id = deps
        .run_id()
        .map(|id| id.to_string())
        .unwrap_or_else(|| "local".to_string());
    let prefix = deps.bench_name_prefix().unwrap_or_default();

    let admin_client = deps
        .registry_service()
        .client(&deps.registry_service().admin_account_token())
        .await;

    // 1. Account: density-bench-{section}, run-id prefixed for traceability.
    let account_base = format!("{prefix}density-bench-{section}");
    info!("Density-prep: creating account {account_base}");
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

    // Build a user context owning this account so we can use the high-level DSL
    // for component upload and deployment.
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

    // 2. Application (one app holds all the density environments).
    info!("Density-prep: creating application {account_base}-app");
    let app = user_client
        .create_application(
            &account.id.0,
            &ApplicationCreation {
                name: ApplicationName(format!("{account_base}-app")),
            },
        )
        .await
        .map_err(|e| anyhow::anyhow!("create_application failed: {e:?}"))?;

    // 3. Shared environment holding the single shared component.
    info!("Density-prep: creating shared environment {account_base}-env");
    let shared_env = create_env(&user_client, &app.id.0, &format!("{account_base}-env")).await?;

    // 4. Upload the shared component into the shared environment and deploy it.
    //    The per-agent distinct components each get their own environment (one
    //    component per env) so their identical agent type names do not collide —
    //    the deployment requires each agent type name to resolve to a single
    //    component within an environment.
    let (uniform_component_id, distinct_component_ids) =
        upload_components(&user, &user_client, &app.id.0, &shared_env.id, section).await?;

    let manifest = PrepManifest {
        section: section.as_str().to_string(),
        run_id,
        token: manifest_token,
        environment_id: shared_env.id,
        uniform_component_id,
        distinct_component_ids,
    };

    info!(
        "Density-prep complete for section {section}: shared_env={}, uniform={:?}, distinct={}",
        manifest.environment_id.0,
        manifest.uniform_component_id.as_ref().map(|c| c.0),
        manifest.distinct_component_ids.len()
    );

    Ok(manifest)
}

/// Creates one environment under `app_id` with checks disabled.
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

/// Uploads the section's components, returning `(shared_component_id,
/// per_agent_component_ids)`.
///
/// The shared component is uploaded into `shared_env` and that env is deployed.
/// Each per-agent distinct component gets its own freshly-created environment
/// under `app_id` (one component per env), and each such env is deployed.
async fn upload_components(
    user: &TestUserContext<BenchmarkTestDependencies>,
    client: &golem_client::api::RegistryServiceClientLive,
    app_id: &uuid::Uuid,
    shared_env: &EnvironmentId,
    section: DensitySection,
) -> anyhow::Result<(Option<ComponentId>, Vec<ComponentId>)> {
    match section {
        DensitySection::Agent => {
            // The single shared component used by shared-component cells.
            info!("Density-prep: uploading shared component {UNIFORM_COMPONENT_NAME}");
            let uniform = user
                .component(shared_env, AGENT_COUNTERS_WASM)
                .name(UNIFORM_COMPONENT_NAME)
                .store()
                .await
                .context("uploading shared component")?;
            user.deploy_environment(*shared_env)
                .await
                .context("deploying shared environment")?;

            // Each per-agent distinct component goes into its own environment so
            // the identical agent type names exported by the byte-identical WASM
            // do not collide at deploy time. This also models distinct users
            // each uploading their own component.
            info!(
                "Density-prep: uploading {PER_AGENT_COMPONENT_COUNT} per-agent distinct components, one per environment"
            );
            let mut distinct = Vec::with_capacity(PER_AGENT_COMPONENT_COUNT as usize);
            for i in 1..=PER_AGENT_COMPONENT_COUNT {
                let name = distinct_component_name(i);
                let env = create_env(client, app_id, &format!("{name}-env"))
                    .await
                    .with_context(|| format!("creating environment for {name}"))?;
                let component = user
                    .component(&env.id, AGENT_COUNTERS_WASM)
                    .name(&name)
                    .store()
                    .await
                    .with_context(|| format!("uploading per-agent component {name}"))?;
                user.deploy_environment(env.id)
                    .await
                    .with_context(|| format!("deploying environment for {name}"))?;
                distinct.push(component.id);
                if i % 200 == 0 {
                    info!(
                        "Density-prep: uploaded {i}/{PER_AGENT_COMPONENT_COUNT} per-agent components"
                    );
                }
            }

            Ok((Some(uniform.id), distinct))
        }
        DensitySection::Schedule | DensitySection::Promise => {
            // Schedule- and promise-density components are added when those
            // sections are implemented (golemcloud/golem#3524, #3525).
            anyhow::bail!(
                "density-prep for section {section} is not implemented yet (agent-density only in v1)"
            )
        }
    }
}
