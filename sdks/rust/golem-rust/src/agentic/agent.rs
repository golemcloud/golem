// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::golem_agentic::exports::golem::agent::guest::{AgentType, DataValue};

// A simple Agent that every agent abstraction has to extend
// This is auto implemented when using `agent_implementation` attribute.
// Implementation detail: Once the agent_impl trait has an instance of `Agent`,
// it's internal functionalities can be used to further implement the real component
//
// We never want to directly implement this trait
// Example usage:
//
// ```
//  [agent_definition]
//  trait WeatherAgent: Agent {
//    fn get_weather(&self, location: String) -> String;
//  }
// ```
//
//  ```
//  struct MyWeatherAgent;
//
//  #[agent_implementation]
//  impl WeatherAgent for MyWeatherAgent {fn get_weather(&self, location: String) -> String } }
//  ```
// There is no need to implement `Agent` anywhere, as it is automatically implemented by the `[agent_implementation]` attribute.
pub trait Agent {
    fn get_id(&self) -> String;
    fn invoke(&self, method_name: String, input: DataValue) -> DataValue;
    fn get_definition(&self) -> AgentType;
}
