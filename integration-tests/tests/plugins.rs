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

// TODO: Reenable when oplog processors are converted to agents

// use golem_client::api::RegistryServiceClient;
// use golem_common::model::auth::EnvironmentRole;
// use golem_common::model::base64::Base64;
// use golem_common::model::environment_plugin_grant::EnvironmentPluginGrantCreation;
// use golem_common::model::plugin_registration::{
//     OplogProcessorPluginSpec, PluginRegistrationCreation,
//     PluginSpecDto,
// };
// use golem_common::model::ScanCursor;
// use golem_common::{agent_id, data_value};
// use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
// use golem_test_framework::dsl::{TestDsl, TestDslExtended, WorkerInvocationResultOps};
// use golem_wasm::Value;
// use pretty_assertions::assert_eq;
// use test_r::{inherit_test_dep, test};
//
// inherit_test_dep!(EnvBasedTestDependencies);
//
// #[test]
// #[tracing::instrument]
// async fn oplog_processor(deps: &EnvBasedTestDependencies) -> anyhow::Result<()> {
//     ...
// }
//
// #[test]
// #[tracing::instrument]
// async fn oplog_processor_in_different_env_after_unregistering(
//     deps: &EnvBasedTestDependencies,
// ) -> anyhow::Result<()> {
//     ...
// }
