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
    use golem_rust::agentic::{create_webhook, Principal};
    use golem_rust::agentic::{
        AgentTypeName, Multimodal, MultimodalAdvanced, MultimodalCustom, Schema,
        UnstructuredBinary, UnstructuredText,
    };
    use golem_rust::golem_agentic::golem::agent::common::{AgentMode, AgentType};
    use golem_rust::golem_ai::golem::llm::llm::Config;
    use golem_rust::golem_wasm::golem_rpc_0_2_x::types::Datetime;
    use golem_rust::{agent_definition, agent_implementation, agentic::BaseAgent, Schema};
    use golem_rust::{AllowedLanguages, AllowedMimeTypes, MultimodalSchema};
    use golem_rust_macro::{description, endpoint, prompt};
    use std::fmt::Debug;

    use test_r::test;

    #[agent_definition]
    trait SampleAgent: BaseAgent {
        fn new(init: String) -> Self;
        fn num(&self, i: String) -> u32;
    }

    struct SampleAgentImpl {
        _id: String,
    }

    #[agent_implementation]
    impl SampleAgent for SampleAgentImpl {
        fn new(init: String) -> Self {
            SampleAgentImpl { _id: init }
        }

        fn num(&self, _i: String) -> u32 {
            42
        }
    }

    struct AgentWithTypeParameterImpl<T> {
        _request_id: T,
    }

    #[agent_definition]
    trait AgentWithTypeParameter<T: Schema + Clone + Debug> {
        fn new(init: String) -> Self;
        fn num(&self, i: String) -> u32;
    }

    #[agent_implementation]
    impl AgentWithTypeParameter<String> for AgentWithTypeParameterImpl<String> {
        fn new(init: String) -> Self {
            AgentWithTypeParameterImpl { _request_id: init }
        }

        fn num(&self, _i: String) -> u32 {
            1
        }
    }

    struct AgentWithStaticMethodsImpl;

    #[agent_definition]
    trait AgentWithStaticMethods {
        fn new(init: UserId) -> Self;

        #[allow(unused)]
        fn foo(&self, param: String) -> String;

        #[allow(unused)]
        fn bar(&self, param: String) -> String;
        fn baz(&self, param: String) -> String;
    }

    #[agent_implementation]
    impl AgentWithStaticMethods for AgentWithStaticMethodsImpl {
        fn new(_init: UserId) -> Self {
            AgentWithStaticMethodsImpl
        }

        fn foo(&self, param: String) -> String {
            param
        }

        fn bar(&self, param: String) -> String {
            self.foo(param)
        }

        fn baz(&self, param: String) -> String {
            param
        }
    }

    #[agent_definition]
    trait AgentWithOnlyConstructor {
        fn new(init: UserId) -> Self;
    }

    struct AgentWithOnlyConstructorImpl;
    #[agent_implementation]
    impl AgentWithOnlyConstructor for AgentWithOnlyConstructorImpl {
        fn new(_init: UserId) -> Self {
            AgentWithOnlyConstructorImpl
        }
    }

    #[agent_definition]
    trait AgentWithOnlyStaticMethods {
        fn new(init: UserId) -> Self;

        #[allow(unused)]
        fn foo(&self) -> String;

        #[allow(unused)]
        fn bar(&self, param: String) -> String;
    }

    struct AgentWithOnlyStaticMethodsImpl;

    #[agent_implementation]
    impl AgentWithOnlyStaticMethods for AgentWithOnlyStaticMethodsImpl {
        fn new(_init: UserId) -> Self {
            AgentWithOnlyStaticMethodsImpl
        }
        fn foo(&self) -> String {
            self.bar("foo".to_string())
        }
        fn bar(&self, param: String) -> String {
            param
        }
    }

    #[agent_definition]
    trait FooAgent {
        fn new(init: UserId) -> Self;

        #[allow(unused)]
        fn foo(&self) -> String;
        fn bar(&self) -> String;
    }

    struct FooImpl;

    #[agent_implementation]
    impl FooAgent for FooImpl {
        // Use `FooImpl` instead of `Self` should work
        fn new(_init: UserId) -> FooImpl {
            FooImpl
        }
        fn foo(&self) -> String {
            "foo".to_string()
        }
        fn bar(&self) -> String {
            "bar".to_string()
        }
    }

    #[agent_definition]
    trait Echo {
        fn new(init: UserId, llm_config: Config) -> Self;

        fn echo_mut(&mut self, message: String) -> String;
        fn echo(&self, message: String) -> String;
        fn get_id(&self) -> String;
        fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()>;
        fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String>;
        fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()>;
        fn echo_option(&self, option: Option<String>) -> Option<String>;
        fn echo_multimodal_advanced(
            &self,
            input: MultimodalAdvanced<TextOrImage>,
        ) -> MultimodalAdvanced<TextOrImage>;
        fn echo_multimodal(&self, input: Multimodal) -> Multimodal;
        fn echo_multimodal_custom(
            &self,
            input: MultimodalCustom<TextOrImage>,
        ) -> MultimodalCustom<TextOrImage>;

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
        _llm_config: Config,
    }

    #[derive(Debug, Clone, AllowedLanguages)]
    enum MyLang {
        #[code("de")]
        German,
        En,
    }

    #[derive(Debug, Clone, AllowedMimeTypes)]
    enum MyMimeType {
        #[mime_type("text/plain")]
        PlainText,
        #[mime_type("image/png")]
        PngImage,
    }

    #[agent_implementation]
    impl Echo for EchoImpl {
        fn new(id: UserId, llm_config: Config) -> Self {
            EchoImpl {
                _id: id,
                _llm_config: llm_config,
            }
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

        fn echo_multimodal_advanced(
            &self,
            input: MultimodalAdvanced<TextOrImage>,
        ) -> MultimodalAdvanced<TextOrImage> {
            input
        }

        fn echo_multimodal(&self, input: Multimodal) -> Multimodal {
            input
        }

        fn echo_multimodal_custom(
            &self,
            input: MultimodalCustom<TextOrImage>,
        ) -> MultimodalCustom<TextOrImage> {
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

        async fn load_snapshot(&mut self, _bytes: Vec<u8>) -> Result<(), String> {
            Ok(())
        }

        async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
            Ok(vec![])
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
        async fn new(init: UserId, llm_config: Config) -> Self;
        async fn echo_mut(&mut self, message: String) -> String;
        async fn echo(&self, message: String) -> String;
        async fn get_id(&self) -> String;
        async fn echo_result(&self, result: Result<(), ()>) -> Result<(), ()>;
        async fn echo_result_err(&self, result: Result<(), String>) -> Result<(), String>;
        async fn echo_result_ok(&self, result: Result<String, ()>) -> Result<String, ()>;
        async fn echo_option(&self, option: Option<String>) -> Option<String>;
        async fn echo_multimodal_custom(
            &self,
            input: MultimodalAdvanced<TextOrImage>,
        ) -> MultimodalAdvanced<TextOrImage>;
        async fn echo_multimodal(&self, input: Multimodal) -> Multimodal;
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
        llm_config: Config,
    }

    #[agent_implementation]
    impl EchoAsync for EchoAsyncImpl {
        async fn new(id: UserId, llm_config: Config) -> Self {
            EchoAsyncImpl { id, llm_config }
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

        async fn echo_multimodal_custom(
            &self,
            input: MultimodalAdvanced<TextOrImage>,
        ) -> MultimodalAdvanced<TextOrImage> {
            input
        }

        async fn echo_multimodal(&self, input: Multimodal) -> Multimodal {
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
            let client = EchoClient::get(self.id.clone(), self.llm_config.clone());
            client.echo(string).await
        }

        fn rpc_call_trigger(&self, string: String) {
            let client = EchoClient::get(self.id.clone(), self.llm_config.clone());
            client.trigger_echo(string);
        }

        fn rpc_call_schedule(&self, string: String) {
            let client = EchoClient::get(self.id.clone(), self.llm_config.clone());
            client.schedule_echo(
                string,
                Datetime {
                    seconds: 1,
                    nanoseconds: 1,
                },
            );
        }
    }

    #[agent_definition]
    trait AgentWithReservedMethods {
        fn new(init: String) -> Self;

        fn get(&self) -> String;

        async fn remote_get(&mut self) -> String;
    }

    struct AgentWithGetFunctionImpl {}

    #[agent_implementation]
    impl AgentWithReservedMethods for AgentWithGetFunctionImpl {
        fn new(_init: String) -> Self {
            AgentWithGetFunctionImpl {}
        }

        fn get(&self) -> String {
            "foo".to_string()
        }

        async fn remote_get(&mut self) -> String {
            let client = AgentWithReservedMethodsClient::get_("increment".to_string());

            client.get().await
        }
    }

    #[derive(Schema, MultimodalSchema)]
    enum TextOrImage {
        Text(String),
        Image(Vec<u8>),
    }

    #[derive(Schema, Clone)]
    struct UserId {
        id: String,
    }

    #[allow(clippy::assertions_on_constants)]
    #[test] // only to verify that the agent compiles correctly
    fn test_agent_compilation() {
        assert!(true);
    }

    #[agent_definition]
    #[description("a descriptive agent")]
    pub trait DescriptiveAgent {
        #[description("new constructor")]
        #[prompt("new prompt")]
        fn new(name: String) -> Self;

        #[description("increment description")]
        #[prompt("increment prompt")]
        fn increment(&mut self) -> u32;
    }

    struct DescriptiveAgentImpl {}

    impl DescriptiveAgent for DescriptiveAgentImpl {
        fn new(_name: String) -> Self {
            DescriptiveAgentImpl {}
        }

        fn increment(&mut self) -> u32 {
            1
        }
    }

    #[test]
    fn test_agent_description_and_prompts() {
        DescriptiveAgentImpl::__register_agent_type();

        let agent_name = AgentTypeName("DescriptiveAgent".to_string());

        let agent =
            golem_rust::agentic::get_agent_type_by_name(&agent_name).expect("Agent type not found");

        let increment_method = agent.methods.first().expect("increment method not found");

        assert_eq!(agent.description, "a descriptive agent");

        assert_eq!(agent.constructor.description, "new constructor");

        assert_eq!(
            agent.constructor.prompt_hint,
            Some("new prompt".to_string())
        );

        assert_eq!(increment_method.description, "increment description");

        assert_eq!(
            increment_method.prompt_hint,
            Some("increment prompt".to_string())
        );
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

    #[agent_definition(
        mount = "/chats/{agent-type}/{foo}/{bar}",
        webhook_suffix = "/{agent-type}/events/{foo}/{bar}",
        auth = true,
        phantom_agent = true,
        cors = ["https://app.acme.com", "https://staging.acme.com"],
    )]
    trait ComplexHttpAgent {
        fn new(foo: String, bar: String) -> Self;

        #[endpoint(
        get = "/path-and-header/{resource_id}",
        headers("X-Request-ID" = "request_id")
        )]
        fn path_and_header(&self, resource_id: String, request_id: String) -> String;

        #[endpoint(get = "/greet?l={location}&n={name}")]
        fn greet1(&self, location: String, name: String) -> String;

        #[endpoint(get = "/greet?l={location}&n={name}")]
        #[endpoint(get = "/greet?lx={location}&nm={name}", cors = ["*"], auth = true, headers("X-Foo" = "location", "X-Bar" = "name"))]
        fn greet2(&self, location: String, name: String) -> String;

        #[endpoint(get = "/greet/{name}/{*file_path}")]
        fn greet3(&self, name: String, file_path: String) -> String;

        #[endpoint(get = "/")]
        fn empty_path(&self);
    }

    struct AgentWithHttpMountImpl {}

    #[agent_implementation]
    impl ComplexHttpAgent for AgentWithHttpMountImpl {
        fn new(_foo: String, _bar: String) -> Self {
            AgentWithHttpMountImpl {}
        }

        fn path_and_header(&self, _resource_id: String, _request_id: String) -> String {
            "foo".to_string()
        }

        fn greet1(&self, _location: String, _name: String) -> String {
            "foo".to_string()
        }

        fn greet2(&self, _location: String, _name: String) -> String {
            "bar".to_string()
        }

        fn greet3(&self, _name: String, _file_path: String) -> String {
            "baz".to_string()
        }

        fn empty_path(&self) {}
    }

    #[test]
    fn test_agent_with_http() {
        use golem_rust::agentic::get_all_agent_types;

        let agent_types = get_all_agent_types();

        let agent = agent_types
            .iter()
            .find(|a| a.type_name == "ComplexHttpAgent")
            .expect("ComplexHttpAgent not found");

        assert!(
            agent.http_mount.is_some(),
            "HTTP mount details should be set"
        );

        assert_eq!(
            agent.http_mount.as_ref().map(|hm| hm.phantom_agent),
            Some(true),
            "Agent phantom property should be set"
        );

        assert!(
            !agent.methods.is_empty(),
            "Agent should have methods defined"
        );

        assert!(
            agent.methods.iter().all(|m| !m.http_endpoint.is_empty()),
            "All methods should have HTTP endpoint details"
        );

        assert!(agent.methods.iter().all(|m| !m.http_endpoint.is_empty()),)
    }

    #[agent_definition(mount = "/chats/{agent-type}")]
    trait SimpleHttpAgent {
        fn new() -> Self;

        #[endpoint(get = "/green/{name}")]
        fn greet(&self, name: String) -> String;
    }

    struct SimpleHttpAgentImpl;

    #[agent_implementation]
    impl SimpleHttpAgent for SimpleHttpAgentImpl {
        fn new() -> Self {
            SimpleHttpAgentImpl
        }

        fn greet(&self, name: String) -> String {
            format!("Hello, {}!", name)
        }
    }

    // Auto Injected Principal Compilation Tests
    // Principal last parameter
    #[agent_definition]
    pub trait AgentWithPrincipalAutoInjection1 {
        fn new(name: String, principal: Principal) -> Self;

        fn foo(&self, name: String, principal: Principal) -> String;
    }

    pub struct AgentWithPrincipalAutoInjection1Impl;

    #[agent_implementation]
    impl AgentWithPrincipalAutoInjection1 for AgentWithPrincipalAutoInjection1Impl {
        fn new(_name: String, _principal: Principal) -> Self {
            Self
        }

        fn foo(&self, name: String, _principal: Principal) -> String {
            name
        }
    }

    // Auto Injected Principal Compilation Tests
    #[agent_definition]
    pub trait AgentWithPrincipalAutoInjection2 {
        fn new(name: String, text1: u64, principal: Principal, text: String) -> Self;

        fn foo(&self, name: String, num: u64, principal: Principal, text: String) -> String;
    }

    pub struct AgentWithPrincipalAutoInjection2Impl;

    #[agent_implementation]
    impl AgentWithPrincipalAutoInjection2 for AgentWithPrincipalAutoInjection2Impl {
        fn new(_name: String, _text1: u64, _principal: Principal, _text: String) -> Self {
            Self
        }

        fn foo(&self, name: String, _num: u64, _principal: Principal, _text: String) -> String {
            name
        }
    }

    #[agent_definition]
    pub trait AgentWithPrincipalAutoInjection3 {
        fn new(name: String, text1: u64, principal: Principal, text: Option<String>) -> Self;

        fn foo(
            &self,
            name: String,
            text1: u64,
            principal: Principal,
            text: Option<String>,
        ) -> String;

        fn foo_without_principal_1(&self, name: String, num: u64) -> String;
        fn foo_without_principal_2(&self, name: String, num: u64, text: String) -> String;
        fn foo_without_principal_3(&self, name: String, num: u64, text: Option<String>) -> String;
        fn foo_without_principal_4(
            &self,
            name: String,
            num: u64,
            text1: Option<String>,
            text2: Option<String>,
        ) -> String;
        fn foo_without_principal_5(&self, name: String, num: u64, text: Option<String>) -> String;
    }

    pub struct AgentWithPrincipalAutoInjection5Impl;

    #[agent_implementation]
    impl AgentWithPrincipalAutoInjection3 for AgentWithPrincipalAutoInjection5Impl {
        fn new(_name: String, _text1: u64, _principal: Principal, _text: Option<String>) -> Self {
            Self
        }

        fn foo(
            &self,
            name: String,
            _text1: u64,
            _principal: Principal,
            _text: Option<String>,
        ) -> String {
            name
        }

        fn foo_without_principal_1(&self, name: String, _num: u64) -> String {
            name
        }

        fn foo_without_principal_2(&self, name: String, _num: u64, _text: String) -> String {
            name
        }

        fn foo_without_principal_3(
            &self,
            name: String,
            _num: u64,
            _text: Option<String>,
        ) -> String {
            name
        }

        fn foo_without_principal_4(
            &self,
            name: String,
            _num: u64,
            _text1: Option<String>,
            _text2: Option<String>,
        ) -> String {
            name
        }

        fn foo_without_principal_5(
            &self,
            name: String,
            _num: u64,
            _text: Option<String>,
        ) -> String {
            name
        }
    }

    #[agent_definition]
    pub trait RemoteAgentWithPrincipal {
        fn new(name: String, principal: Principal) -> Self;
        async fn foo(&self, name: String) -> String;
    }

    pub struct RemoteAgentWithPrincipalImpl;

    #[agent_implementation]
    impl RemoteAgentWithPrincipal for RemoteAgentWithPrincipalImpl {
        fn new(_name: String, _principal: Principal) -> Self {
            Self
        }

        async fn foo(&self, name: String) -> String {
            AgentWithPrincipalAutoInjection1Client::get(name.clone())
                .foo(name.clone())
                .await;

            AgentWithPrincipalAutoInjection2Client::get(name.clone(), 1, "required".into())
                .foo(name.clone(), 1, "required".into())
                .await;

            AgentWithPrincipalAutoInjection3Client::get(
                name.clone(),
                1,
                Some("not-undefined".into()),
            )
            .foo_without_principal_1("name".into(), 1)
            .await;

            "finished".into()
        }
    }

    #[agent_definition]
    pub trait WebhookAgent {
        fn new(name: String) -> Self;

        async fn create_webhook_and_trigger(&self) -> String;
    }

    fn webhook_placeholder(_url: &str) -> String {
        "webhook triggered".to_string()
    }

    pub struct WebhookAgentImpl;

    #[agent_implementation]
    impl WebhookAgent for WebhookAgentImpl {
        fn new(_name: String) -> Self {
            Self
        }

        async fn create_webhook_and_trigger(&self) -> String {
            let webhook = create_webhook();

            webhook_placeholder(webhook.url());

            let request = webhook.await;

            request.json().unwrap()
        }
    }
}
