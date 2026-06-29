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

//! Constructor/method input-field name disambiguation for MCP tools.
//!
//! An MCP tool merges a constructor's user-supplied parameters with a method's
//! into a single flat JSON object (see `combined_input_schema`). If the same
//! user-supplied parameter name appears on both sides, a naive merge would
//! collapse the two properties in the advertised schema and, at invoke time,
//! feed the same JSON value into both the constructor and the method.
//!
//! To keep both sides callable we *disambiguate only the colliding names* by
//! prefixing them (`constructor_<name>` / `method_<name>`), leaving every
//! non-colliding name untouched. The mapping is a pure function of the
//! constructor and method schemas, so the schema-export side and the invoke
//! side can recompute it independently and stay in lockstep without persisting
//! any extra state.
//!
//! The mapping translates between two name spaces:
//! - *advertised name*: what the MCP client sees in the tool schema and sends
//!   back in the arguments object.
//! - *original name*: the field name in the constructor/method schema, which is
//!   also the key the legacy invoke extractors look up.

use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, FieldSource, NamedField,
};
use rmcp::model::JsonObject;
use std::collections::{HashMap, HashSet};

/// A bidirectional mapping between advertised MCP property names and original
/// schema field names, populated only for fields whose name collides between
/// the constructor and the method.
#[derive(Debug, Default, Clone)]
pub struct FieldNameMapping {
    /// `original name -> advertised name` for renamed constructor fields.
    constructor: HashMap<String, String>,
    /// `original name -> advertised name` for renamed method fields.
    method: HashMap<String, String>,
}

/// Compute the disambiguation mapping for a constructor/method pair.
///
/// Only names that are `UserSupplied` on *both* sides are disambiguated;
/// everything else is left as-is. Generated names are guaranteed unique against
/// every user-supplied field name on either side and against each other (a
/// numeric suffix is appended on the rare chance the prefixed name is already
/// taken).
pub fn field_name_mapping(
    constructor: &AgentConstructorSchema,
    method: &AgentMethodSchema,
) -> FieldNameMapping {
    let ctor_names = user_supplied_names(constructor.input_schema.fields());
    let method_names: HashSet<&str> = user_supplied_names(method.input_schema.fields())
        .into_iter()
        .collect();

    // Colliding names, in constructor field order, deduplicated.
    let mut collisions: Vec<&str> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for name in &ctor_names {
        if method_names.contains(name) && seen.insert(name) {
            collisions.push(name);
        }
    }

    if collisions.is_empty() {
        return FieldNameMapping::default();
    }

    // Every existing user-supplied name is "taken"; generated names must avoid
    // them (and each other) so disambiguation never reintroduces a clash.
    let mut taken: HashSet<String> = HashSet::new();
    for name in &ctor_names {
        taken.insert((*name).to_string());
    }
    for name in &method_names {
        taken.insert((*name).to_string());
    }

    let mut mapping = FieldNameMapping::default();
    for name in collisions {
        let advertised_ctor = unique_name(format!("constructor_{name}"), &mut taken);
        let advertised_method = unique_name(format!("method_{name}"), &mut taken);
        mapping
            .constructor
            .insert(name.to_string(), advertised_ctor);
        mapping.method.insert(name.to_string(), advertised_method);
    }
    mapping
}

impl FieldNameMapping {
    /// Clone the constructor fields, renaming any disambiguated field to its
    /// advertised name. Field order and non-colliding fields are preserved.
    pub fn apply_to_constructor_fields(&self, fields: &[NamedField]) -> Vec<NamedField> {
        apply(fields, &self.constructor)
    }

    /// Clone the method fields, renaming any disambiguated field to its
    /// advertised name. Field order and non-colliding fields are preserved.
    pub fn apply_to_method_fields(&self, fields: &[NamedField]) -> Vec<NamedField> {
        apply(fields, &self.method)
    }

    /// Rewrite an MCP arguments object so the constructor extractor can read it:
    /// every advertised constructor name is renamed back to its original name.
    pub fn rewrite_constructor_args(&self, args: &JsonObject) -> JsonObject {
        rewrite(args, &self.constructor)
    }

    /// Rewrite an MCP arguments object so the method extractor can read it:
    /// every advertised method name is renamed back to its original name.
    pub fn rewrite_method_args(&self, args: &JsonObject) -> JsonObject {
        rewrite(args, &self.method)
    }
}

fn user_supplied_names(fields: &[NamedField]) -> Vec<&str> {
    fields
        .iter()
        .filter(|f| matches!(f.source, FieldSource::UserSupplied))
        .map(|f| f.name.as_str())
        .collect()
}

fn unique_name(candidate: String, taken: &mut HashSet<String>) -> String {
    if taken.insert(candidate.clone()) {
        return candidate;
    }
    let mut i = 2;
    loop {
        let suffixed = format!("{candidate}_{i}");
        if taken.insert(suffixed.clone()) {
            return suffixed;
        }
        i += 1;
    }
}

/// Clone `fields`, replacing the name of any field present in `renames`
/// (`original -> advertised`) with its advertised name.
fn apply(fields: &[NamedField], renames: &HashMap<String, String>) -> Vec<NamedField> {
    fields
        .iter()
        .map(|f| {
            let mut f = f.clone();
            if let Some(advertised) = renames.get(&f.name) {
                f.name = advertised.clone();
            }
            f
        })
        .collect()
}

/// Rewrite `args`, renaming each advertised key back to its original name
/// (`renames` is `original -> advertised`).
fn rewrite(args: &JsonObject, renames: &HashMap<String, String>) -> JsonObject {
    if renames.is_empty() {
        return args.clone();
    }
    let mut out = args.clone();
    for (original, advertised) in renames {
        if let Some(value) = out.remove(advertised) {
            out.insert(original.clone(), value);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::agent::{AgentConstructorSchema, AgentMethodSchema, OutputSchema};
    use golem_common::schema::agent::{AutoInjectedKind, InputSchema};
    use golem_common::schema::schema_type::SchemaType;
    use serde_json::json;
    use test_r::test;

    fn constructor(fields: Vec<NamedField>) -> AgentConstructorSchema {
        AgentConstructorSchema {
            name: None,
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(fields),
        }
    }

    fn method(fields: Vec<NamedField>) -> AgentMethodSchema {
        AgentMethodSchema {
            name: "m".to_string(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::Parameters(fields),
            output_schema: OutputSchema::Unit,
            http_endpoint: vec![],
            read_only: None,
        }
    }

    fn names(fields: &[NamedField]) -> Vec<String> {
        fields.iter().map(|f| f.name.clone()).collect()
    }

    fn is_empty(mapping: &FieldNameMapping) -> bool {
        mapping.constructor.is_empty() && mapping.method.is_empty()
    }

    #[test]
    fn no_collision_is_empty_and_noop() {
        let ctor = constructor(vec![NamedField::user_supplied(
            "name",
            SchemaType::string(),
        )]);
        let meth = method(vec![NamedField::user_supplied(
            "city",
            SchemaType::string(),
        )]);
        let mapping = field_name_mapping(&ctor, &meth);
        assert!(is_empty(&mapping));
        assert_eq!(
            names(&mapping.apply_to_constructor_fields(ctor.input_schema.fields())),
            vec!["name"]
        );
        assert_eq!(
            names(&mapping.apply_to_method_fields(meth.input_schema.fields())),
            vec!["city"]
        );
        let args = json!({"name": "a", "city": "b"})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(mapping.rewrite_constructor_args(&args), args);
        assert_eq!(mapping.rewrite_method_args(&args), args);
    }

    #[test]
    fn collision_renames_both_sides_in_schema() {
        let ctor = constructor(vec![
            NamedField::user_supplied("id", SchemaType::string()),
            NamedField::user_supplied("shared", SchemaType::string()),
        ]);
        let meth = method(vec![
            NamedField::user_supplied("shared", SchemaType::u32()),
            NamedField::user_supplied("extra", SchemaType::string()),
        ]);
        let mapping = field_name_mapping(&ctor, &meth);
        assert!(!is_empty(&mapping));

        assert_eq!(
            names(&mapping.apply_to_constructor_fields(ctor.input_schema.fields())),
            vec!["id", "constructor_shared"]
        );
        assert_eq!(
            names(&mapping.apply_to_method_fields(meth.input_schema.fields())),
            vec!["method_shared", "extra"]
        );
    }

    #[test]
    fn collision_rewrites_args_back_to_original_names() {
        let ctor = constructor(vec![NamedField::user_supplied(
            "shared",
            SchemaType::string(),
        )]);
        let meth = method(vec![NamedField::user_supplied("shared", SchemaType::u32())]);
        let mapping = field_name_mapping(&ctor, &meth);

        let args = json!({"constructor_shared": "abc", "method_shared": 7})
            .as_object()
            .unwrap()
            .clone();

        let ctor_args = mapping.rewrite_constructor_args(&args);
        assert_eq!(ctor_args.get("shared"), Some(&json!("abc")));

        let method_args = mapping.rewrite_method_args(&args);
        assert_eq!(method_args.get("shared"), Some(&json!(7)));
    }

    #[test]
    fn generated_name_avoids_existing_field() {
        // The method already contains `constructor_shared`, so the generated
        // name for the constructor side must dodge it.
        let ctor = constructor(vec![NamedField::user_supplied(
            "shared",
            SchemaType::string(),
        )]);
        let meth = method(vec![
            NamedField::user_supplied("shared", SchemaType::u32()),
            NamedField::user_supplied("constructor_shared", SchemaType::string()),
        ]);
        let mapping = field_name_mapping(&ctor, &meth);

        let ctor_fields = mapping.apply_to_constructor_fields(ctor.input_schema.fields());
        let advertised = &ctor_fields[0].name;
        assert_ne!(advertised, "constructor_shared");
        assert_eq!(advertised, "constructor_shared_2");
    }

    use proptest::prelude::*;

    /// Build a constructor/method pair from name lists (all user-supplied,
    /// string-typed), deduplicating names within each side (a real schema can't
    /// declare the same field name twice on one side).
    fn pair_from_names(
        ctor_names: Vec<String>,
        method_names: Vec<String>,
    ) -> (AgentConstructorSchema, AgentMethodSchema) {
        let dedup = |names: Vec<String>| {
            let mut seen = HashSet::new();
            names
                .into_iter()
                .filter(|n| seen.insert(n.clone()))
                .map(|n| NamedField::user_supplied(n, SchemaType::string()))
                .collect::<Vec<_>>()
        };
        (constructor(dedup(ctor_names)), method(dedup(method_names)))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// However names collide between the two sides, the advertised property
        /// names (constructor fields + method fields, after disambiguation) are
        /// always globally unique, so no property is ever silently dropped from
        /// the merged MCP tool schema.
        #[test]
        fn advertised_names_are_globally_unique(
            ctor_names in proptest::collection::vec("[a-c]{1,3}", 0..6),
            method_names in proptest::collection::vec("[a-c]{1,3}", 0..6),
        ) {
            let (ctor, meth) = pair_from_names(ctor_names, method_names);
            let mapping = field_name_mapping(&ctor, &meth);

            let mut advertised: Vec<String> =
                names(&mapping.apply_to_constructor_fields(ctor.input_schema.fields()));
            advertised
                .extend(names(&mapping.apply_to_method_fields(meth.input_schema.fields())));

            let unique: HashSet<&String> = advertised.iter().collect();
            prop_assert_eq!(
                unique.len(),
                advertised.len(),
                "duplicate advertised names: {:?}",
                advertised
            );
        }

        /// For every colliding name the advertised argument keys round-trip back
        /// to the original field name each extractor reads, with the value kept
        /// on the correct side.
        #[test]
        fn colliding_args_round_trip_to_original_names(
            shared in proptest::collection::hash_set("[a-c]{1,3}", 0..4),
        ) {
            let shared: Vec<String> = shared.into_iter().collect();
            let (ctor, meth) = pair_from_names(shared.clone(), shared.clone());
            let mapping = field_name_mapping(&ctor, &meth);

            // Advertise, then send one value per advertised key.
            let ctor_fields = mapping.apply_to_constructor_fields(ctor.input_schema.fields());
            let method_fields = mapping.apply_to_method_fields(meth.input_schema.fields());

            let mut args = JsonObject::new();
            for f in ctor_fields.iter() {
                args.insert(f.name.clone(), json!(format!("ctor::{}", f.name)));
            }
            for f in method_fields.iter() {
                args.insert(f.name.clone(), json!(format!("method::{}", f.name)));
            }

            let ctor_args = mapping.rewrite_constructor_args(&args);
            let method_args = mapping.rewrite_method_args(&args);

            for (original, advertised) in &mapping.constructor {
                prop_assert_eq!(
                    ctor_args.get(original),
                    Some(&json!(format!("ctor::{advertised}")))
                );
            }
            for (original, advertised) in &mapping.method {
                prop_assert_eq!(
                    method_args.get(original),
                    Some(&json!(format!("method::{advertised}")))
                );
            }
        }
    }

    #[test]
    fn auto_injected_fields_are_not_disambiguated() {
        // A method field that shares a name but is auto-injected must not be
        // treated as a collision (auto-injected fields are never advertised).
        let ctor = constructor(vec![NamedField::user_supplied(
            "shared",
            SchemaType::string(),
        )]);
        let meth = method(vec![NamedField::auto_injected(
            "shared",
            AutoInjectedKind::Principal,
            SchemaType::string(),
        )]);
        let mapping = field_name_mapping(&ctor, &meth);
        assert!(is_empty(&mapping));
    }
}
