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
/// Identity of a deployed agent, as returned by the create-agent and
/// invoke-agent endpoints.
pub(all) struct AgentId {
  component_id : String
  agent_id : String
} derive(Debug, Eq)

///|
fn AgentId::from_json(j : Json) -> AgentId raise {
  let m = expect_object(j)
  {
    component_id: expect_string(get_field(m, "componentId")),
    agent_id: expect_string(get_field(m, "agentId")),
  }
}
