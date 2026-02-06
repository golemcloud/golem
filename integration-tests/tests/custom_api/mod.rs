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

mod agent_http_routes_ts;
mod agent_http_routes_rust;
// mod echo_agent;
// mod shopping_cart;

use golem_test_framework::config::EnvBasedTestDependencies;
use test_r::inherit_test_dep;

inherit_test_dep!(EnvBasedTestDependencies);
