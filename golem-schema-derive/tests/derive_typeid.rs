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
//
//! Tests for the canonical dotted TypeId format (§4.20).
#![allow(dead_code)]

use golem_common::schema::{FromSchema, IntoSchema, TypeId};
use test_r::test;

test_r::enable!();

mod nested {
    use golem_common::schema::{FromSchema, IntoSchema};
    #[derive(IntoSchema, FromSchema)]
    pub struct Profile {
        pub name: String,
    }
}

#[derive(IntoSchema, FromSchema)]
struct Container<Inner> {
    items: Vec<Inner>,
}

#[derive(IntoSchema, FromSchema)]
#[schema(named = "shared.Container")]
struct NamedContainer<Inner> {
    items: Vec<Inner>,
}

#[test]
fn nested_struct_yields_dotted_type_id() {
    let id = nested::Profile::type_id();
    assert!(
        !id.as_str().contains("::"),
        "type id should not contain `::`, got `{id}`"
    );
    assert!(id.as_str().ends_with(".nested.Profile"), "got `{id}`");
}

#[test]
fn generic_container_appends_normalized_args() {
    let id = Container::<u32>::type_id();
    assert!(id.as_str().contains('<'));
    assert!(id.as_str().contains('>'));
    assert!(!id.as_str().contains("::"));
    assert!(id.as_str().contains("<u32>"), "got `{id}`");
}

#[test]
fn generic_instantiations_do_not_collide() {
    let a: TypeId = Container::<u32>::type_id();
    let b: TypeId = Container::<String>::type_id();
    assert_ne!(a, b, "distinct instantiations must have distinct ids");
    assert!(a.as_str().contains("<u32>"));
    // String maps to its dotted form
    assert!(
        b.as_str().contains("<alloc.string.String>"),
        "expected String arg in `{b}`"
    );
}

#[test]
fn named_generic_auto_suffixes_args() {
    let a = NamedContainer::<u32>::type_id();
    let b = NamedContainer::<String>::type_id();
    assert_eq!(a, TypeId::new("shared.Container<u32>"));
    assert_eq!(b, TypeId::new("shared.Container<alloc.string.String>"));
}
