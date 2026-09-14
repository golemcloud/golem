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

pub mod fixtures;
pub mod moonbit;
pub mod parameter_naming;
pub mod rust;
// The Rust compile checks are the bulk of the bridge_gen suite; the it-cli `bridge_gen_rust`
// CI shard runs only them and the `bridge_gen` shard skips them.
test_r::tag_suite!(rust, bridge_gen_rust);
pub mod scala;
// Scala compile checks reference the in-tree SDK and share its sbt target directories.
test_r::sequential_suite!(scala);
pub mod schema_graph_literals;
#[path = "../../src/bridge_gen/schema_graph_test_fixture.rs"]
#[allow(dead_code)]
pub mod schema_graph_test_fixture;
pub mod type_naming;
pub mod typescript;
