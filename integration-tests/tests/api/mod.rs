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

mod account;
mod application;
mod auth;
mod component;
mod deployment;
mod domain_registration;
mod environment;
mod environment_plugin_grants;
mod environment_share;
mod http_api_deployment;
mod plugin_registration;
mod reports;
mod security_schemes;
use golem_test_framework::config::EnvBasedTestDependencies;
use test_r::inherit_test_dep;

inherit_test_dep!(EnvBasedTestDependencies);
