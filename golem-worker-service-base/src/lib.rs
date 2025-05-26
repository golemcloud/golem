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

use golem_common::golem_version;
use service::worker::WorkerRequestMetadata;

pub mod api;
pub mod app_config;
pub mod gateway_api_definition;
pub mod gateway_api_definition_transformer;
pub mod gateway_api_deployment;
pub mod gateway_binding;
pub mod gateway_execution;
pub mod gateway_middleware;
pub mod gateway_request;
pub mod gateway_rib_compiler;
pub mod gateway_rib_interpreter;
pub mod gateway_security;
pub mod getter;
pub mod grpcapi;
mod headers;
pub mod http_invocation_context;
pub mod metrics;
pub mod path;
pub mod repo;
pub mod service;

#[cfg(test)]
test_r::enable!();

const VERSION: &str = golem_version!();

pub fn empty_worker_metadata() -> WorkerRequestMetadata {
    WorkerRequestMetadata {
        account_id: Some(golem_common::model::AccountId::placeholder()),
        limits: None,
    }
}
