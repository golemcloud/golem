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
#![allow(dead_code)]

use golem_common::schema::{
    DiscriminatorRule, IntoSchema, SchemaBuilder, SchemaType, try_into_schema_graph,
};
use test_r::test;

test_r::enable!();

#[derive(IntoSchema)]
#[schema(union)]
enum Resource {
    #[schema(prefix = "ssh://")]
    Ssh(String),
    #[schema(prefix = "https://")]
    Web(String),
}

#[test]
fn union_into_schema_emits_branches_with_discriminators() {
    let mut builder = SchemaBuilder::new();
    let root = Resource::register_in(&mut builder);
    let graph = builder.into_graph(root);

    assert_eq!(graph.defs.len(), 1);
    match &graph.defs[0].body {
        SchemaType::Union { spec, .. } => {
            assert_eq!(spec.branches.len(), 2);
            assert_eq!(spec.branches[0].tag, "Ssh");
            assert_eq!(spec.branches[1].tag, "Web");
            assert_eq!(
                spec.branches[0].discriminator,
                DiscriminatorRule::Prefix {
                    prefix: "ssh://".to_string(),
                }
            );
            assert_eq!(
                spec.branches[1].discriminator,
                DiscriminatorRule::Prefix {
                    prefix: "https://".to_string(),
                }
            );
            assert_eq!(spec.branches[0].body, SchemaType::string());
            assert_eq!(spec.branches[1].body, SchemaType::string());
        }
        other => panic!("expected union body, got {other:?}"),
    }
}

#[derive(IntoSchema)]
struct CircleBody {
    kind: String,
    radius: u32,
}

#[derive(IntoSchema)]
struct UntaggedBody {
    payload: String,
}

#[derive(IntoSchema)]
#[schema(union)]
enum MixedDisc {
    #[schema(suffix = ".tar.gz")]
    Tarball(String),
    #[schema(contains = "::")]
    Scoped(String),
    #[schema(regex = "^\\d+$")]
    Digits(String),
    #[schema(field_equals(field = "kind", literal = "circle"))]
    Circle(CircleBody),
    #[schema(field_absent = "kind")]
    Untagged(UntaggedBody),
}

#[test]
fn union_supports_all_discriminator_kinds() {
    let graph = try_into_schema_graph::<MixedDisc>().expect("graph should be well-formed");
    let def = graph
        .defs
        .iter()
        .find(|d| d.id == MixedDisc::type_id())
        .expect("mixed disc def");
    let body = match &def.body {
        SchemaType::Union { spec, .. } => spec,
        other => panic!("expected union body, got {other:?}"),
    };
    assert_eq!(body.branches.len(), 5);
    matches_disc(&body.branches[0].discriminator, "suffix");
    matches_disc(&body.branches[1].discriminator, "contains");
    matches_disc(&body.branches[2].discriminator, "regex");
    matches_disc(&body.branches[3].discriminator, "field_equals");
    matches_disc(&body.branches[4].discriminator, "field_absent");
}

fn matches_disc(rule: &DiscriminatorRule, expected: &str) {
    let actual = match rule {
        DiscriminatorRule::Prefix { .. } => "prefix",
        DiscriminatorRule::Suffix { .. } => "suffix",
        DiscriminatorRule::Contains { .. } => "contains",
        DiscriminatorRule::Regex { .. } => "regex",
        DiscriminatorRule::FieldEquals(_) => "field_equals",
        DiscriminatorRule::FieldAbsent { .. } => "field_absent",
    };
    assert_eq!(actual, expected, "discriminator mismatch (rule: {rule:?})");
}
