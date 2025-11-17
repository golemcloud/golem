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

test_r::enable!();

#[cfg(test)]
#[cfg(feature = "export_golem_agentic")]
mod tests {
    use golem_rust::agentic::Multimodal;
    use golem_rust::agentic::{Multimodal, UnstructuredBinary, UnstructuredText};
    use golem_rust::golem_agentic::golem::agent::common::{AgentMode, AgentType};
    use golem_rust::wasm_rpc::golem_rpc_0_2_x::types::Datetime;
    use golem_rust::{agent_definition, agent_implementation, agentic::Agent, Schema};
    use golem_rust_macro::MultimodalSchema;
    use golem_rust_macro::{AllowedLanguages, AllowedMimeTypes, MultimodalSchema};
    use test_r::test;

    #[agent_definition]
    trait Echo {
        fn new(init: UserId) -> Self;
        fn echo_mut(&mut self, message: String) -> String;
        fn echo(&self, message: String) -> String;
        fn get_id(&self) -> String;
        fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()>;
        fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String>;
        fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()>;
        fn echo_option(&self, option: Option<String>) -> Option<String>;
        fn echo_multimodal(&self, input: Multimodal<TextOrImage>) -> Multimodal<TextOrImage>;
        fn echo_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText;
        fn echo_unstructured_text_lc(
            &self,
            input: UnstructuredText<MyLang>,
        ) -> UnstructuredText<MyLang>;

        fn echo_unstructured_binary(
            &self,
            input: UnstructuredBinary<MyMimeType>,
        ) -> UnstructuredBinary<MyMimeType>;
    }

    struct EchoImpl {
        _id: UserId,
    }

    #[derive(AllowedLanguages)]
    enum MyLang {
        #[code("de")]
        German,
        En,
    }

    #[derive(AllowedMimeTypes)]
    enum MyMimeType {
        #[mime_type("text/plain")]
        PlainText,
        #[mime_type("image/png")]
        PngImage,
    }

    #[agent_implementation]
    impl Echo for EchoImpl {
        fn new(id: UserId) -> Self {
            EchoImpl { _id: id }
        }
        fn echo_mut(&mut self, message: String) -> String {
            format!("Echo: {}", message)
        }

        fn echo(&self, message: String) -> String {
            message.to_string()
        }

        fn get_id(&self) -> String {
            self.get_agent_id()
        }

        fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()> {
            result
        }

        fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String> {
            result
        }

        fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()> {
            result
        }

        fn echo_option(&self, option: Option<String>) -> Option<String> {
            option
        }

        fn echo_multimodal(&self, input: Multimodal<TextOrImage>) -> Multimodal<TextOrImage> {
            input
        }

        fn echo_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText {
            input
        }
        fn echo_unstructured_text_lc(
            &self,
            input: UnstructuredText<MyLang>,
        ) -> UnstructuredText<MyLang> {
            input
        }

        fn echo_unstructured_binary(
            &self,
            input: UnstructuredBinary<MyMimeType>,
        ) -> UnstructuredBinary<MyMimeType> {
            input
        }
    }

    // Test: #[agent_definition(mode = "ephemeral")]
    #[agent_definition(mode = "ephemeral")]
    trait EchoEphemeralExplicit {
        fn new(init: UserId) -> Self;
        fn echo(&self, message: String) -> String;
    }

    struct EchoEphemeralExplicitImpl {
        _id: UserId,
    }

    #[agent_implementation]
    impl EchoEphemeralExplicit for EchoEphemeralExplicitImpl {
        fn new(id: UserId) -> Self {
            EchoEphemeralExplicitImpl { _id: id }
        }
        fn echo(&self, message: String) -> String {
            message.to_string()
        }
    }

    // Test: #[agent_definition(ephemeral)]
    #[agent_definition(ephemeral)]
    trait EchoEphemeralShorthand {
        fn new(init: UserId) -> Self;
        fn echo(&self, message: String) -> String;
    }

    struct EchoEphemeralShorthandImpl {
        _id: UserId,
    }

    #[agent_implementation]
    impl EchoEphemeralShorthand for EchoEphemeralShorthandImpl {
        fn new(id: UserId) -> Self {
            EchoEphemeralShorthandImpl { _id: id }
        }
        fn echo(&self, message: String) -> String {
            message.to_string()
        }
    }

    // Test: #[agent_definition(mode = "durable")]
    #[agent_definition(mode = "durable")]
    trait EchoDurableExplicit {
        fn new(init: UserId) -> Self;
        fn echo(&self, message: String) -> String;
    }

    struct EchoDurableExplicitImpl {
        _id: UserId,
    }

    #[agent_implementation]
    impl EchoDurableExplicit for EchoDurableExplicitImpl {
        fn new(id: UserId) -> Self {
            EchoDurableExplicitImpl { _id: id }
        }
        fn echo(&self, message: String) -> String {
            message.to_string()
        }
    }

    // Test: #[agent_definition] (default, should be durable)
    #[agent_definition]
    trait EchoDurableDefault {
        fn new(init: UserId) -> Self;
        fn echo(&self, message: String) -> String;
    }

    struct EchoDurableDefaultImpl {
        _id: UserId,
    }

    #[agent_implementation]
    impl EchoDurableDefault for EchoDurableDefaultImpl {
        fn new(id: UserId) -> Self {
            EchoDurableDefaultImpl { _id: id }
        }
        fn echo(&self, message: String) -> String {
            message.to_string()
        }
    }

    #[agent_definition]
    trait EchoAsync {
        async fn new(init: UserId) -> Self;
        async fn echo_mut(&mut self, message: String) -> String;
        async fn echo(&self, message: String) -> String;
        async fn get_id(&self) -> String;
        async fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()>;
        async fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String>;
        async fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()>;
        async fn echo_option(&self, option: Option<String>) -> Option<String>;
        async fn echo_multimodal(&self, input: Multimodal<TextOrImage>) -> Multimodal<TextOrImage>;
        async fn echo_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText;
        async fn echo_unstructured_text_lc(
            &self,
            input: UnstructuredText<MyLang>,
        ) -> UnstructuredText<MyLang>;
        async fn rpc_call(&self, string: String) -> String;
        fn rpc_call_trigger(&self, string: String);
        fn rpc_call_schedule(&self, string: String);
    }

    struct EchoAsyncImpl {
        id: UserId,
    }

    #[agent_implementation]
    impl EchoAsync for EchoAsyncImpl {
        async fn new(id: UserId) -> Self {
            EchoAsyncImpl { id }
        }
        async fn echo_mut(&mut self, message: String) -> String {
            format!("Echo: {}", message)
        }

        async fn echo(&self, message: String) -> String {
            message.to_string()
        }

        async fn get_id(&self) -> String {
            self.get_agent_id()
        }

        async fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()> {
            result
        }

        async fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String> {
            result
        }

        async fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()> {
            result
        }

        async fn echo_option(&self, option: Option<String>) -> Option<String> {
            option
        }

        async fn echo_multimodal(&self, input: Multimodal<TextOrImage>) -> Multimodal<TextOrImage> {
            input
        }

        async fn echo_unstructured_text(&self, input: UnstructuredText) -> UnstructuredText {
            input
        }

        async fn echo_unstructured_text_lc(
            &self,
            input: UnstructuredText<MyLang>,
        ) -> UnstructuredText<MyLang> {
            input
        }

        async fn rpc_call(&self, string: String) -> String {
            let client = EchoClient::get(self.id.clone());
            client.echo(string).await
        }

        fn rpc_call_trigger(&self, string: String) {
            let client = EchoClient::get(self.id.clone());
            client.trigger_echo(string);
        }

        fn rpc_call_schedule(&self, string: String) {
            let client = EchoClient::get(self.id.clone());
            client.schedule_echo(
                string,
                Datetime {
                    seconds: 1,
                    nanoseconds: 1,
                },
            );
        }
    }

    #[derive(MultimodalSchema)]
    enum TextOrImage {
        Text(String),
        Image(Vec<u8>),
    }

    #[derive(Schema, Clone)]
    struct UserId {
        id: String,
    }

    #[test] // only to verify that the agent compiles correctly
    fn test_agent_compilation() {
        assert!(true);
    }

    #[test]
    fn test_agent_modes() {
        use golem_rust::agentic::get_all_agent_types;

        let agent_types = get_all_agent_types();

        // Helper to find an agent type by name
        let find_agent = |name: &str| -> Option<AgentType> {
            agent_types.iter().find(|a| a.type_name == name).cloned()
        };

        // Test: default (no attribute) should be Durable
        if let Some(agent) = find_agent("EchoDurableDefault") {
            assert_eq!(
                &agent.mode,
                &AgentMode::Durable,
                "EchoDurableDefault should be Durable"
            );
        }

        // Test: mode = "durable" should be Durable
        if let Some(agent) = find_agent("EchoDurableExplicit") {
            assert_eq!(
                &agent.mode,
                &AgentMode::Durable,
                "EchoDurableExplicit should be Durable"
            );
        }

        // Test: ephemeral shorthand should be Ephemeral
        if let Some(agent) = find_agent("EchoEphemeralShorthand") {
            assert_eq!(
                &agent.mode,
                &AgentMode::Ephemeral,
                "EchoEphemeralShorthand should be Ephemeral"
            );
        }

        // Test: mode = "ephemeral" should be Ephemeral
        if let Some(agent) = find_agent("EchoEphemeralExplicit") {
            assert_eq!(
                &agent.mode,
                &AgentMode::Ephemeral,
                "EchoEphemeralExplicit should be Ephemeral"
            );
        }
    }
}
