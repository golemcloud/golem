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

#[cfg(test)]
#[cfg(feature = "export_golem_agentic")]
mod tests {
    use golem_rust::agentic::MultiModal;
    use golem_rust::{agent_definition, agentic::Agent, MultiModalSchema};

    #[agent_definition]
    trait Echo {
        fn new(init: String) -> Self;
        fn process_multimodal_input(&self, input: MultiModal<TextOrImage>);
    }

    #[derive(MultiModalSchema)]
    enum TextOrImage {
        Text(String),
        Image(Vec<u8>),
    }

    #[test] // only to verify that the agent compiles correctly
    fn test_agent_compilation() {
        assert!(true);
    }
}
