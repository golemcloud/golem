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

use golem_cli::bridge_gen::parameter_naming::ParameterNaming;
use test_r::test;

#[test]
fn parameter_naming_returns_preferred_name_when_available() {
    let mut naming = ParameterNaming::new();
    assert_eq!(naming.fresh("__args"), "__args");
}

#[test]
fn parameter_naming_generates_deterministic_suffixes_for_collisions() {
    let mut naming = ParameterNaming::new();
    naming.reserve("__args");
    assert_eq!(naming.fresh("__args"), "__args1");
    assert_eq!(naming.fresh("__args"), "__args2");
}

#[test]
fn parameter_naming_skips_already_used_suffixes() {
    let mut naming = ParameterNaming::new();
    naming.reserve_many(vec!["__args", "__args1", "__args2"]);
    assert_eq!(naming.fresh("__args"), "__args3");
}
