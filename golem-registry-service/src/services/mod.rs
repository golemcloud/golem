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

pub mod account;
pub mod account_usage;
pub mod application;
pub mod auth;
pub mod component;
pub mod component_compilation;
pub mod component_object_store;
pub mod component_resolver;
pub mod component_transformer_plugin_caller;
pub mod deployment;
pub mod domain_registration;
pub mod environment;
pub mod environment_plugin_grant;
pub mod environment_share;
pub mod http_api_deployment;
pub mod oauth2;
pub mod oauth2_github_client;
pub mod plan;
pub mod plugin_registration;
pub mod reports;
pub mod security_scheme;
pub mod token;

/// Run CPU-heavy work on the global Rayon pool, returning a Future
pub async fn run_cpu_bound_work<F, R>(f: F) -> R
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();

    rayon::spawn(move || {
        let result = f();
        // ignore error if receiver was dropped
        let _ = tx.send(result);
    });

    rx.await.expect("Rayon task panicked or channel closed")
}
