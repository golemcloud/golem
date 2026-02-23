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

use crate::bridge_gen::type_naming::{TypeName, TypeNaming};
use crate::model::agent::test::code_first_snippets_agent_type;
use crate::model::GuestLanguage;

pub(crate) fn test_type_naming<TN: TypeName>(language: GuestLanguage, agent_name: &str) {
    let agent_type = code_first_snippets_agent_type(language, agent_name);
    TypeNaming::<TN>::new(&agent_type).unwrap();

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
