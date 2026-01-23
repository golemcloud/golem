// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::bridge_gen::type_naming::TypeNaming;
use crate::model::agent::test::code_first_snippets_agent_type;
use golem_templates::model::GuestLanguage;
use golem_wasm::analysis::AnalysedType;
use test_r::test;

#[test]
fn test_type_naming_rust_foo() {
    test_type_naming(GuestLanguage::Rust, "FooAgent");
}

#[test]
fn test_type_naming_rust_bar() {
    test_type_naming(GuestLanguage::Rust, "BarAgent");
}

#[test]
fn test_type_naming_ts_foo() {
    test_type_naming(GuestLanguage::TypeScript, "FooAgent");
}

#[test]
fn test_type_naming_ts_bar() {
    test_type_naming(GuestLanguage::TypeScript, "BarAgent");
}

fn test_type_naming(language: GuestLanguage, agent_name: &str) {
    let agent_type = code_first_snippets_agent_type(language, agent_name);
    TypeNaming::new(&agent_type, should_be_named);

    /*
    println!("Collected anonymous types:");
    for (typ, locations) in &type_naming.anonymous_type_locations {
        println!("- {:?}", typ);
        for location in locations {
            println!("  - {}", location);
        }
    }
    println!("Collected named types:");
    for (name, types) in &type_naming.named_type_locations {
        println!("- {}", name);
        for (typ, locations) in types {
            println!("  - {:?}", typ);
            for location in locations {
                println!("    - {}", location);
            }
        }
    }
    println!("Final named types:");
    for (typ, name) in type_naming.types {
        println!("- {}: {:?}", name, typ);
    }
    */
}

fn should_be_named(typ: &AnalysedType) -> bool {
    match typ {
        AnalysedType::Variant(_)
        | AnalysedType::Result(_)
        | AnalysedType::Option(_)
        | AnalysedType::Enum(_)
        | AnalysedType::Flags(_)
        | AnalysedType::Record(_)
        | AnalysedType::Tuple(_)
        | AnalysedType::List(_)
        | AnalysedType::Handle(_) => true,
        AnalysedType::Str(_)
        | AnalysedType::Chr(_)
        | AnalysedType::F64(_)
        | AnalysedType::F32(_)
        | AnalysedType::U64(_)
        | AnalysedType::S64(_)
        | AnalysedType::U32(_)
        | AnalysedType::S32(_)
        | AnalysedType::U16(_)
        | AnalysedType::S16(_)
        | AnalysedType::U8(_)
        | AnalysedType::S8(_)
        | AnalysedType::Bool(_) => false,
    }
}
