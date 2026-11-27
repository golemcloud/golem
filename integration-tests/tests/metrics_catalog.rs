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

//! Checks that what the services export is in the metric catalog of
//! `golem_common::metrics::catalog`. The clippy rules on registration cover the metrics that
//! Golem declares; the scrape covers the families that a library adds.

use crate::Tracing;
use anyhow::Context;
use golem_common::metrics::catalog;
use golem_common::{agent_key, data_value};
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::{TestDsl, TestDslExtended};
use test_r::{inherit_test_dep, test, timeout};

inherit_test_dep!(Tracing);
inherit_test_dep!(EnvBasedTestDependencies);

#[test]
#[tracing::instrument]
#[timeout("4m")]
async fn every_scraped_metric_family_is_in_the_catalog(
    deps: &EnvBasedTestDependencies,
    _tracing: &Tracing,
) -> anyhow::Result<()> {
    // A short workload: a deployment, an agent creation, invocations over the HTTP API and one
    // agent-to-agent RPC.
    let user = deps.user().await?;
    let (_, env) = user.app_and_env().await?;
    let component = user
        .component(&env.id, "golem_it_agent_rpc_rust_release")
        .name("golem-it:agent-rpc-rust")
        .unique()
        .store()
        .await?;
    let parent = agent_key!("RustParent", "metrics-catalog-scrape");
    user.start_agent(&component.id, parent.clone()).await?;
    user.invoke_and_await_agent(&component, &parent, "spawn_child", data_value!("scrape"))
        .await?;

    let worker_service = deps.worker_service();
    let registry_service = deps.registry_service();
    let shard_manager = deps.shard_manager();
    let compilation_service = deps.component_compilation_service();
    let mut endpoints = vec![
        (
            "worker service",
            format!(
                "http://{}:{}/metrics",
                worker_service.http_host(),
                worker_service.http_port()
            ),
        ),
        (
            "registry service",
            format!(
                "http://{}:{}/metrics",
                registry_service.http_host(),
                registry_service.http_port()
            ),
        ),
        (
            "shard manager",
            format!(
                "http://{}:{}/metrics",
                shard_manager.grpc_host(),
                shard_manager.http_port()
            ),
        ),
        (
            "compilation service",
            format!(
                "http://{}:{}/metrics",
                compilation_service.grpc_host(),
                compilation_service.http_port()
            ),
        ),
    ];
    for executor in deps.worker_executor_cluster().to_vec() {
        let (url, _) = executor
            .metrics_endpoint()
            .context("the scrape needs the HTTP port of every executor")?;
        endpoints.push(("worker executor", url));
    }

    let client = reqwest::Client::new();
    let mut problems = Vec::new();
    for (service, url) in &endpoints {
        let body = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        anyhow::ensure!(
            body.lines()
                .any(|line| line.starts_with("golem_build_info{")
                    || line.starts_with("golem_executor_build_info{")),
            "{service} at {url} did not serve a Golem metrics response:\n{body}"
        );
        for line in body.lines() {
            let Some(declaration) = line.strip_prefix("# TYPE ") else {
                continue;
            };
            let (name, kind) = declaration
                .split_once(' ')
                .with_context(|| format!("invalid TYPE line from {service}: {line}"))?;
            match catalog::find(name) {
                Some(def) if def.metric_type.as_str() == kind => {}
                Some(def) => problems.push(format!(
                    "{service}: {name} is a {kind}, the catalog declares a {}",
                    def.metric_type.as_str()
                )),
                None if catalog::find_group(name).is_some() => {}
                None => problems.push(format!("{service}: {name} is not in the catalog")),
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    Ok(())
}
