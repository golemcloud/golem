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

    use golem_rust::{agent_definition, agent_implementation, agentic::Agent, Schema};

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
    }

    struct EchoImpl {
        _id: UserId,
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
        async fn rpc_call(&self, string: String) -> String;
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

        async fn rpc_call(&self, string: String) -> String {
            let client = EchoClient::get(self.id.clone());
            client.echo(string).await
        }
    }

    #[derive(Schema, Clone)]
    struct UserId {
        id: String,
    }

    #[test] // only to verify that the agent compiles correctly
    fn test_agent_compilation() {
        assert!(true);
    }
}
