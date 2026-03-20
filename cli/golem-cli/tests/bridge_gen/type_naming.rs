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

pub use golem_cli::bridge_gen::type_naming::*;

use crate::bridge_gen::fixtures::code_first_snippets_agent_type;
use crate::bridge_gen::type_naming::{TypeName, TypeNaming};
use golem_cli::model::GuestLanguage;

pub(crate) fn test_type_naming<TN: TypeName>(language: GuestLanguage, agent_name: &str) {
    let agent_type = code_first_snippets_agent_type(language, agent_name);
    TypeNaming::<TN>::new(&agent_type, false).unwrap();
}
