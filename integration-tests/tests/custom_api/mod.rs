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

mod agent_http_principal_ts;
mod agent_http_routes_rust;
mod agent_http_routes_ts;
mod http_test_context;
mod mcp;
mod openapi_generation;

use golem_test_framework::config::EnvBasedTestDependencies;
use test_r::inherit_test_dep;

inherit_test_dep!(EnvBasedTestDependencies);

fn assert_json_content_type(response: &reqwest::Response) {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected Content-Type: application/json, got: {content_type:?}"
    );
}
