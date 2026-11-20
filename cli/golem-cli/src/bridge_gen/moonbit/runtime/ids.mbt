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

///|
/// The ID of an agent: an opaque UUID that Golem issues. `Show` prints the
/// lowercase hyphenated UUID, which is also its JSON form.
pub(all) struct AgentId {
  uuid : Uuid
} derive(Debug, Eq)

///|
pub extend AgentId with Debug::{to_repr}

///|
pub extend AgentId with Eq::{equal, not_equal}

///|
pub extend AgentId with Show::{to_string, output}

///|
pub impl Show for AgentId with fn output(self, logger) {
  self.uuid.output(logger)
}

///|
/// Parses the text form of an agent ID, a hyphenated UUID.
pub fn AgentId::parse(value : String) -> AgentId raise BridgeError {
  { uuid: Uuid::parse(value) }
}

///|
fn AgentId::from_json(j : Json) -> AgentId raise {
  { uuid: Uuid::parse_canonical(expect_string(j)) }
}
