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
use golem_cli::bridge_gen::rust::RustTypeName;
use golem_cli::model::GuestLanguage;
use test_r::test;

pub(crate) fn test_type_naming<TN: TypeName>(language: GuestLanguage, agent_name: &str) {
    let agent_type = code_first_snippets_agent_type(language, agent_name);
    TypeNaming::<TN>::new(&agent_type, false).unwrap();
}

#[test]
fn test_type_naming_reserved_names_are_not_used() {
    let agent_type = code_first_snippets_agent_type(GuestLanguage::Rust, "FooAgent");

    let without_reserved = TypeNaming::<RustTypeName>::new(&agent_type, false).unwrap();
    let without_reserved_names = without_reserved
        .types()
        .map(|(_, name)| name.to_string())
        .collect::<Vec<_>>();
    assert!(without_reserved_names
        .iter()
        .any(|name| name == "AllPrimitives"));

    let with_reserved = TypeNaming::<RustTypeName>::new_with_reserved_names(
        &agent_type,
        false,
        vec![RustTypeName::from("AllPrimitives")],
    )
    .unwrap();
    let with_reserved_names = with_reserved
        .types()
        .map(|(_, name)| name.to_string())
        .collect::<Vec<_>>();
    assert!(with_reserved_names
        .iter()
        .all(|name| name != "AllPrimitives"));
}
