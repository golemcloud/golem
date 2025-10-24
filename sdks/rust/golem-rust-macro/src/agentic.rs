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

use proc_macro::TokenStream;

pub fn agent_definition_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    item // TODO: implement agent definition processing
}

pub fn agent_implementation_impl(_attrs: TokenStream, item: TokenStream) -> TokenStream {
    item // TODO: implement agent implementation processing
}

pub fn derive_agent_arg(input: TokenStream) -> TokenStream {
    input // TODO: implement AgentArg derive macro
}
