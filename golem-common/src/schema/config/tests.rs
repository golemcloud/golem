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

use super::*;
use crate::schema::{
    DiscriminatorRule, FieldDiscriminator, SchemaTypeDef, SecretSpec, UnionBranch, UnionSpec,
};
use serde_json::json;
use test_r::test;

fn origin(layer: &str) -> ConfigOrigin {
    ConfigOrigin {
        file: format!("{layer}.yaml").into(),
        path: vec!["config".into(), "prod".into()],
        layer: layer.into(),
    }
}

fn schema(kind: ConfigFieldKind, ty: SchemaType) -> ConfigSchema {
    ConfigSchema {
        graph: SchemaGraph::empty(),
        descriptors: BTreeMap::new(),
        fields: vec![ConfigFieldSchema {
            path: vec!["setting".into()],
            kind,
            value_type: ty,
            default: None,
        }],
    }
}

fn catalog(value: Value) -> ConfigCatalogSources {
    let mut sources = ConfigCatalogSources::default();
    sources
        .add(&serde_json::from_value(value).unwrap(), &origin("catalog"))
        .unwrap();
    sources
}

fn layers(values: Vec<Value>) -> Vec<ConfigLayer> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| ConfigLayer {
            origin: origin(&format!("layer{index}")),
            value,
        })
        .collect()
}

fn record(fields: Vec<(&str, SchemaType)>) -> SchemaType {
    SchemaType::record(
        fields
            .into_iter()
            .map(|(name, body)| NamedFieldType {
                name: name.into(),
                body,
                metadata: Default::default(),
            })
            .collect(),
    )
}

#[test]
fn catalog_static_defaults_and_explicit_values() {
    let mut schema = schema(ConfigFieldKind::Static, SchemaType::u32());
    schema.fields[0].default = Some(SchemaValue::U32(4000));
    let sources = catalog(json!({"ports": {"main": 8123}}));
    for (input, expected) in [
        (json!({}), 4000),
        (json!({"setting": 0}), 0),
        (json!({"setting": {"$config": "ports.main"}}), 8123),
    ] {
        let plan = schema
            .resolve(&sources, &layers(vec![input]), &origin("code"))
            .unwrap();
        assert_eq!(
            plan[0].source,
            ConfigSourcePlan::Static(SchemaValue::U32(expected))
        );
    }
    for input in [
        json!({"setting": "bad"}),
        json!({"setting": null}),
        json!({"setting": {"$config": "ports.missing"}}),
        json!({"Setting": 9}),
    ] {
        assert!(
            schema
                .resolve(&sources, &layers(vec![input]), &origin("code"))
                .is_err()
        );
    }
    for (ty, default, fill, expected) in [
        (
            SchemaType::bool(),
            SchemaValue::Bool(true),
            json!(false),
            SchemaValue::Bool(false),
        ),
        (
            SchemaType::list(SchemaType::u32()),
            SchemaValue::List {
                elements: vec![SchemaValue::U32(7)],
            },
            json!([]),
            SchemaValue::List { elements: vec![] },
        ),
        (
            SchemaType::option(SchemaType::u32()),
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::U32(7))),
            },
            Value::Null,
            SchemaValue::Option { inner: None },
        ),
    ] {
        schema.fields[0].value_type = ty;
        schema.fields[0].default = Some(default);
        let plan = schema
            .resolve(
                &sources,
                &layers(vec![json!({"setting":fill})]),
                &origin("code"),
            )
            .unwrap();
        assert_eq!(plan[0].source, ConfigSourcePlan::Static(expected));
    }
}

#[test]
fn catalog_shared_omission_is_not_a_binding_or_fallback() {
    let alias = TypeId::new("MaybeSetting");
    let mut schema = schema(ConfigFieldKind::Shared, SchemaType::ref_to(alias.clone()));
    schema.graph.defs.push(SchemaTypeDef {
        id: alias,
        name: None,
        body: SchemaType::option(SchemaType::u32()),
    });
    let sources = catalog(
        json!({"live": {"$kind":"shared", "$init":null}, "plain": 1, "private": {"$kind":"secret"}}),
    );
    let absent = schema
        .resolve(&sources, &layers(vec![json!({})]), &origin("code"))
        .unwrap();
    assert_eq!(absent[0].source, ConfigSourcePlan::CapturedAbsence);
    let bound = schema
        .resolve(
            &sources,
            &layers(vec![json!({"setting":{"$config":"live"}}), json!({})]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        bound[0].source,
        ConfigSourcePlan::Shared {
            catalog_path: vec!["live".into()]
        }
    );
    for value in [
        json!(null),
        json!({"$config":"missing"}),
        json!({"$config":"plain"}),
        json!({"$config":"private"}),
        json!({"$kind":"shared"}),
    ] {
        assert!(
            schema
                .resolve(
                    &sources,
                    &layers(vec![json!({"setting":value})]),
                    &origin("code")
                )
                .is_err()
        );
    }
    for (kind, ty) in [
        (
            ConfigFieldKind::Secret,
            SchemaType::option(SchemaType::string()),
        ),
        (ConfigFieldKind::Shared, SchemaType::list(SchemaType::u32())),
    ] {
        assert!(
            super::tests::schema(kind, ty)
                .resolve(&sources, &[], &origin("code"))
                .is_err()
        );
    }
}

#[test]
fn catalog_union_overlays_replace_cases_and_preserve_positions() {
    let choice = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "a".into(),
                body: record(vec![
                    ("kind", SchemaType::string()),
                    ("a", SchemaType::u32()),
                    ("common", SchemaType::u32()),
                ]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".into(),
                    literal: Some("a".into()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "b".into(),
                body: record(vec![
                    ("kind", SchemaType::string()),
                    ("b", SchemaType::u32()),
                ]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".into(),
                    literal: Some("b".into()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let schema = schema(
        ConfigFieldKind::Static,
        record(vec![
            ("choice", choice),
            (
                "items",
                SchemaType::list(SchemaType::option(SchemaType::u32())),
            ),
        ]),
    );
    let base = json!({"setting":{"choice":{"kind":"a","a":7,"common":11},"items":[1,2]}});
    for (overlay, expected) in [
        (
            json!({"setting":{"choice":{"kind":"a","a":3},"items":[8,null,6]}}),
            json!({"choice":{"kind":"a","a":3,"common":11},"items":[8,null,6]}),
        ),
        (
            json!({"setting":{"choice":{"kind":"b","b":29}}}),
            json!({"choice":{"kind":"b","b":29},"items":[1,2]}),
        ),
        (
            json!({"setting":{"choice":{"a":19}}}),
            json!({"choice":{"kind":"a","a":19,"common":11},"items":[1,2]}),
        ),
    ] {
        let plan = schema
            .resolve(
                &catalog(json!({})),
                &layers(vec![base.clone(), overlay]),
                &origin("code"),
            )
            .unwrap();
        let ConfigSourcePlan::Static(value) = &plan[0].source else {
            panic!("static capture expected")
        };
        assert_eq!(
            to_json_value(&schema.graph, &schema.fields[0].value_type, value).unwrap(),
            expected
        );
    }
}

#[test]
fn catalog_selectors_replace_records_instead_of_merging_them() {
    let schema = schema(
        ConfigFieldKind::Static,
        record(vec![
            ("left", SchemaType::u32()),
            ("right", SchemaType::u32()),
        ]),
    );
    let sources = catalog(json!({"partial":{"left":13}}));
    let input = layers(vec![
        json!({"setting":{"left":1,"right":2}}),
        json!({"setting":{"$config":"partial"}}),
    ]);
    assert!(
        schema.resolve(&sources, &input, &origin("code")).is_err(),
        "a selector must not inherit right=2"
    );
}

#[test]
fn catalog_whole_groups_keep_kinds_and_origins() {
    let schema = ConfigSchema {
        graph: SchemaGraph::empty(),
        descriptors: BTreeMap::new(),
        fields: vec![
            ConfigFieldSchema {
                path: vec!["group".into(), "plain".into()],
                kind: ConfigFieldKind::Static,
                value_type: SchemaType::u32(),
                default: None,
            },
            ConfigFieldSchema {
                path: vec!["group".into(), "live".into()],
                kind: ConfigFieldKind::Shared,
                value_type: SchemaType::u32(),
                default: None,
            },
            ConfigFieldSchema {
                path: vec!["group".into(), "key".into()],
                kind: ConfigFieldKind::Secret,
                value_type: SchemaType::string(),
                default: None,
            },
        ],
    };
    let sources = catalog(
        json!({"defaults":{"plain":17,"live":{"$kind":"shared"},"key":{"$kind":"secret"}}}),
    );
    let plan = schema
        .resolve(
            &sources,
            &layers(vec![json!({"group":{"$config":"defaults"}})]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        plan[0].source,
        ConfigSourcePlan::Static(SchemaValue::U32(17))
    );
    assert_eq!(
        plan[1].source,
        ConfigSourcePlan::Shared {
            catalog_path: vec!["defaults".into(), "live".into()]
        }
    );
    assert_eq!(
        plan[2].source,
        ConfigSourcePlan::Secret {
            catalog_path: vec!["defaults".into(), "key".into()]
        }
    );
    assert_eq!(
        plan[0].catalog_origin.as_ref().unwrap().file,
        PathBuf::from("catalog.yaml")
    );
    let selected = catalog(json!({
        "keys":{"api":{"$kind":"secret"}},
        "defaults":{"plain":17,"live":{"$kind":"shared"},"key":{"$secret":"keys.api"}}
    }));
    let plan = schema
        .resolve(
            &selected,
            &layers(vec![json!({"group":{"$config":"defaults"}})]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        plan[2].source,
        ConfigSourcePlan::Secret {
            catalog_path: vec!["keys".into(), "api".into()]
        }
    );
    assert_eq!(
        plan[2].origins[&vec!["group".into(), "key".into()]].path,
        vec!["config", "prod", "defaults", "key"]
    );
    let mismatched = catalog(
        json!({"defaults":{"plain":17,"live":{"$kind":"shared"},"key":{"$kind":"secret"},"unexpected":9}}),
    );
    assert!(
        schema
            .resolve(
                &mismatched,
                &layers(vec![json!({"group":{"$config":"defaults"}})]),
                &origin("code")
            )
            .is_err()
    );
}

#[test]
fn catalog_nesting_restrictions_follow_aliases() {
    let key = TypeId::new("Credential");
    let secret = SchemaType::secret(SecretSpec::default());
    let mut schema = schema(
        ConfigFieldKind::Static,
        SchemaType::option(SchemaType::ref_to(key.clone())),
    );
    schema.graph.defs.push(SchemaTypeDef {
        id: key.clone(),
        name: None,
        body: record(vec![("key", secret)]),
    });
    let error = schema.validate(&origin("code")).unwrap_err();
    assert_eq!(error.origin.path, vec!["config", "prod", "setting", "key"]);
    schema.fields[0].kind = ConfigFieldKind::Shared;
    assert!(schema.validate(&origin("code")).is_ok());
    schema.fields[0].kind = ConfigFieldKind::Secret;
    assert!(schema.validate(&origin("code")).is_err());
    schema.fields[0].kind = ConfigFieldKind::Shared;
    schema.fields[0].default = Some(SchemaValue::Option { inner: None });
    assert!(schema.validate(&origin("code")).is_err());
}

#[test]
fn catalog_oauth_grammar_and_dependency_cycles_are_pure() {
    let oauth = |path: &str| json!({"$kind":"secret","$oauth":{"authorizationServer":"https://login.test","clientId":"app","clientSecret":{"$secret":path},"scopes":["read"],"resource":"https://service.test"}});
    assert!(
        catalog(json!({"token":oauth("key"),"key":{"$kind":"secret"}}))
            .validate()
            .is_ok()
    );
    assert!(
        catalog(json!({"token":oauth("missing")}))
            .validate()
            .is_err()
    );
    assert!(
        catalog(json!({"token":oauth("key"),"key":4}))
            .validate()
            .is_err()
    );
    assert!(
        catalog(json!({"a":oauth("b"),"b":oauth("a")}))
            .validate()
            .is_err()
    );
    for invalid in [
        json!({"a.b":1}),
        json!({"a":{"$config":"b"}}),
        json!({"a":{"$kind":"shared","$init":{"nested":{"$kind":"shared"}}}}),
        json!({"a":{"$kind":"secret","$oauth":{},"$init":"private"}}),
        json!({"a":{"$kind":"resource","$init":null}}),
    ] {
        assert!(serde_json::from_value::<ConfigCatalog>(invalid).is_err());
    }
    let sources = catalog(json!({"live":{"$kind":"shared","$init":{"value":1}}}));
    assert!(sources.select("live.value", &origin("owner")).is_err());
    assert!(sources.select("Live", &origin("owner")).is_err());
}

#[test]
fn catalog_static_descriptors_preserve_secret_selectors_without_handles() {
    let schema = schema(
        ConfigFieldKind::Static,
        record(vec![
            ("endpoint", SchemaType::string()),
            ("credential", SchemaType::secret(SecretSpec::default())),
        ]),
    );
    let sources = catalog(json!({
        "private": {"$kind":"secret"},
        "descriptor": {"endpoint":"https://example.test", "credential":{"$secret":"private"}}
    }));
    let plan = schema
        .resolve(
            &sources,
            &layers(vec![json!({"setting":{"$config":"descriptor"}})]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        plan[0].source,
        ConfigSourcePlan::Descriptor(
            json!({"endpoint":"https://example.test", "credential":{"$secret":"private"}})
        )
    );
    assert_eq!(
        plan[0].origins[&vec!["setting".into(), "endpoint".into()]].file,
        PathBuf::from("catalog.yaml")
    );
    for credential in [
        json!({"$secret":"missing"}),
        json!({"$secret":"descriptor"}),
        json!("private"),
        json!({"$secret":"private","extra":1}),
    ] {
        assert!(
            schema
                .resolve(
                    &sources,
                    &layers(vec![
                        json!({"setting":{"endpoint":"x", "credential":credential}})
                    ]),
                    &origin("code")
                )
                .is_err()
        );
    }
}

#[test]
fn catalog_group_selection_is_atomic_and_groups_remain_required() {
    let mut schema = schema(
        ConfigFieldKind::Shared,
        SchemaType::option(SchemaType::u32()),
    );
    schema.fields[0].path = vec!["group".into(), "setting".into()];
    let sources = catalog(json!({"empty":{},"live":{"$kind":"shared"}}));
    assert!(schema.resolve(&sources, &[], &origin("code")).is_err());
    for input in [
        vec![json!({"$config":"empty"})],
        vec![json!({"group":{}}), json!({"$config":"empty"})],
        vec![
            json!({"group":{"setting":{"$config":"live"}}}),
            json!({"$config":"empty"}),
        ],
    ] {
        let error = schema
            .resolve(&sources, &layers(input), &origin("code"))
            .expect_err("replacing the root with an empty catalog removes the required group");
        assert!(
            error
                .message
                .contains("missing required config group 'group'")
        );
    }
    for input in [
        vec![json!({"group":{}}), json!({})],
        vec![
            json!({"group":{}}),
            json!({"$config":"empty"}),
            json!({"group":{}}),
        ],
    ] {
        let plan = schema
            .resolve(&sources, &layers(input), &origin("code"))
            .unwrap();
        assert_eq!(plan[0].source, ConfigSourcePlan::CapturedAbsence);
    }
    let plan = schema
        .resolve(
            &sources,
            &layers(vec![
                json!({"group":{"setting":{"$config":"live"}}}),
                json!({"group":{"$config":"empty"}}),
            ]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(plan[0].source, ConfigSourcePlan::CapturedAbsence);
    schema.fields[0].value_type = SchemaType::u32();
    assert!(
        schema
            .resolve(
                &sources,
                &layers(vec![
                    json!({"group":{"setting":{"$config":"live"}}}),
                    json!({"group":{"$config":"empty"}})
                ]),
                &origin("code")
            )
            .is_err()
    );
    schema.fields[0].value_type = SchemaType::option(SchemaType::u32());
    schema.fields[0].path = vec!["group".into(), "nested".into(), "setting".into()];
    for (overlay, valid) in [
        (json!({}), true),
        (json!({"group":{"$config":"empty"}}), false),
    ] {
        let result = schema.resolve(
            &sources,
            &layers(vec![json!({"group":{"nested":{}}}), overlay]),
            &origin("code"),
        );
        assert_eq!(result.is_ok(), valid);
        if let Ok(plan) = result {
            assert_eq!(plan[0].source, ConfigSourcePlan::CapturedAbsence);
        }
    }
}

#[test]
fn catalog_static_omission_and_layer_provenance() {
    let mut schema = schema(
        ConfigFieldKind::Static,
        record(vec![
            ("left", SchemaType::u32()),
            ("right", SchemaType::u32()),
        ]),
    );
    let plan = schema
        .resolve(
            &catalog(json!({})),
            &layers(vec![
                json!({"setting":{"left":1,"right":2}}),
                json!({"setting":{"right":3}}),
            ]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        plan[0].origins[&vec!["setting".into(), "left".into()]].layer,
        "layer0"
    );
    assert_eq!(
        plan[0].origins[&vec!["setting".into(), "right".into()]].layer,
        "layer1"
    );
    let error = schema
        .resolve(
            &catalog(json!({})),
            &layers(vec![
                json!({"setting":{"left":"invalid","right":2}}),
                json!({"setting":{"right":3}}),
            ]),
            &origin("code"),
        )
        .unwrap_err();
    assert_eq!(error.origin.file, PathBuf::from("layer0.yaml"));
    assert_eq!(error.origin.path, vec!["config", "prod", "setting", "left"]);
    schema.fields[0].value_type = SchemaType::option(SchemaType::u32());
    assert_eq!(
        schema
            .resolve(&catalog(json!({})), &[], &origin("code"))
            .unwrap()[0]
            .source,
        ConfigSourcePlan::Static(SchemaValue::Option { inner: None })
    );
    let error = schema
        .resolve(
            &catalog(json!({})),
            &layers(vec![json!({"setting":"invalid"})]),
            &origin("code"),
        )
        .unwrap_err();
    assert_eq!(error.origin.file, PathBuf::from("layer0.yaml"));
}

#[test]
fn catalog_named_managed_values_have_one_schema() {
    let mut schema = schema(ConfigFieldKind::Shared, SchemaType::u32());
    schema.fields.push(ConfigFieldSchema {
        path: vec!["other".into()],
        kind: ConfigFieldKind::Shared,
        value_type: SchemaType::string(),
        default: None,
    });
    let input = layers(vec![
        json!({"setting":{"$config":"live"},"other":{"$config":"live"}}),
    ]);
    let sources = catalog(json!({"live":{"$kind":"shared"}}));
    assert!(schema.resolve(&sources, &input, &origin("code")).is_err());
    let alias = TypeId::new("Count");
    schema.graph.defs.push(SchemaTypeDef {
        id: alias.clone(),
        name: None,
        body: SchemaType::u32(),
    });
    schema.fields[1].value_type = SchemaType::ref_to(alias);
    assert!(schema.resolve(&sources, &input, &origin("code")).is_ok());
    for (index, kind) in [ConfigDescriptorKind::File, ConfigDescriptorKind::Tool]
        .into_iter()
        .enumerate()
    {
        let id = TypeId::new(format!("Descriptor{index}"));
        schema.graph.defs.push(SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::string(),
        });
        schema.descriptors.insert(id.clone(), kind);
        schema.fields[index].value_type = SchemaType::ref_to(id);
    }
    assert!(schema.resolve(&sources, &input, &origin("code")).is_err());
}

#[test]
fn catalog_lists_replace_and_resolve_positional_selectors() {
    let schema = schema(
        ConfigFieldKind::Static,
        SchemaType::list(SchemaType::option(SchemaType::u32())),
    );
    let sources = catalog(json!({"first":13,"last":29}));
    let plan = schema
        .resolve(
            &sources,
            &layers(vec![
                json!({"setting":[1,2,3,4]}),
                json!({"setting":[{"$config":"first"},null,{"$config":"last"}]}),
            ]),
            &origin("code"),
        )
        .unwrap();
    let ConfigSourcePlan::Static(value) = &plan[0].source else {
        panic!("expected static list")
    };
    assert_eq!(
        to_json_value(&schema.graph, &schema.fields[0].value_type, value).unwrap(),
        json!([13, null, 29])
    );
    assert_eq!(
        plan[0].origins[&vec!["setting".into(), "0".into()]].file,
        PathBuf::from("catalog.yaml")
    );
    assert_eq!(
        plan[0].origins[&vec!["setting".into(), "1".into()]].file,
        PathBuf::from("layer1.yaml")
    );
    assert!(
        !plan[0]
            .origins
            .contains_key(&vec!["setting".into(), "3".into()])
    );
}

#[test]
fn catalog_special_descriptors_are_explicit_and_keep_their_own_grammar() {
    let sources = catalog(json!({"budget":{"$kind":"resource"},"notBudget":3}));
    for (kind, valid, expected, invalid) in [
        (
            ConfigDescriptorKind::File,
            json!("prompts/system.txt"),
            json!("prompts/system.txt"),
            json!("/etc/passwd"),
        ),
        (
            ConfigDescriptorKind::Tool,
            json!({"target":"grep","middleware":["audit@1"]}),
            json!({"target":"grep","middleware":["audit@1"]}),
            json!({"target":"grep","middleware":false}),
        ),
        (
            ConfigDescriptorKind::RetryPolicy,
            json!({"periodic":"2s"}),
            json!({"periodic":{"secs":2,"nanos":0}}),
            json!({"periodic":"bad"}),
        ),
        (
            ConfigDescriptorKind::Quota,
            json!({"resource":"budget","expectedUse":7}),
            json!({"resource":"budget","expectedUse":7}),
            json!({"resource":"notBudget"}),
        ),
    ] {
        let id = TypeId::new("Descriptor");
        let mut schema = schema(
            ConfigFieldKind::Static,
            SchemaType::list(SchemaType::option(SchemaType::ref_to(id.clone()))),
        );
        schema.graph.defs.push(SchemaTypeDef {
            id: id.clone(),
            name: None,
            body: SchemaType::string(),
        });
        schema.descriptors.insert(id, kind);
        let plan = schema
            .resolve(
                &sources,
                &layers(vec![json!({"setting":[valid,null]})]),
                &origin("code"),
            )
            .unwrap();
        assert_eq!(
            plan[0].source,
            ConfigSourcePlan::Descriptor(json!([expected, null]))
        );
        assert!(
            schema
                .resolve(
                    &sources,
                    &layers(vec![json!({"setting":[invalid]})]),
                    &origin("code")
                )
                .is_err()
        );
        schema.fields[0].kind = ConfigFieldKind::Secret;
        assert_eq!(
            schema.validate(&origin("code")).is_ok(),
            kind == ConfigDescriptorKind::RetryPolicy
        );
    }
}

#[test]
fn catalog_tool_descriptors_replace_wiring_and_code_defaults_are_canonical() {
    let id = TypeId::new("Callable");
    let mut schema = schema(ConfigFieldKind::Static, SchemaType::ref_to(id.clone()));
    schema.graph.defs.push(SchemaTypeDef {
        id: id.clone(),
        name: None,
        body: SchemaType::string(),
    });
    schema.descriptors.insert(id, ConfigDescriptorKind::Tool);
    schema.fields[0].default = Some(SchemaValue::String(
        r#"{"target":"grep","middleware":["audit"]}"#.into(),
    ));
    let sources = catalog(json!({}));
    let plan = schema.resolve(&sources, &[], &origin("code")).unwrap();
    assert_eq!(
        plan[0].source,
        ConfigSourcePlan::Descriptor(json!({"target":"grep","middleware":["audit"]}))
    );
    let plan = schema
        .resolve(
            &sources,
            &layers(vec![json!({"setting":{"target":"bash","middleware":[]}})]),
            &origin("code"),
        )
        .unwrap();
    assert_eq!(
        plan[0].source,
        ConfigSourcePlan::Descriptor(json!({"target":"bash","middleware":[]}))
    );
    schema.fields[0].default = Some(SchemaValue::String("not canonical JSON".into()));
    assert!(schema.validate(&origin("code")).is_err());
}

#[test]
fn catalog_tool_middleware_scopes_use_manifest_authoring_grammar() {
    let id = TypeId::new("Callable");
    let mut schema = schema(ConfigFieldKind::Static, SchemaType::ref_to(id.clone()));
    schema.graph.defs.push(SchemaTypeDef {
        id: id.clone(),
        name: None,
        body: SchemaType::string(),
    });
    schema.descriptors.insert(id, ConfigDescriptorKind::Tool);
    let sources = catalog(json!({}));
    for (readable, revealable) in [
        (json!("*"), json!([])),
        (json!([]), json!("*")),
        (
            json!([r"credentials.github\=token"]),
            json!([r#""database url".password"#]),
        ),
        (json!(null), json!(null)),
    ] {
        let value = json!({
            "target":"grep",
            "middleware":[{
                "name":"audit",
                "secretKeysReadable":readable,
                "secretKeysRevealable":revealable
            }]
        });
        let plan = schema
            .resolve(
                &sources,
                &layers(vec![json!({"setting":value})]),
                &origin("code"),
            )
            .unwrap();
        assert_eq!(plan[0].source, ConfigSourcePlan::Descriptor(value));
    }
    for name in ["secretKeysReadable", "secretKeysRevealable"] {
        for invalid in [
            json!("anything"),
            json!(["*"]),
            json!(["credentials..key"]),
            json!(["credentials\\"]),
            json!([r#""credentials"#]),
            json!([4]),
            json!({"kind":"all"}),
        ] {
            let value = json!({"target":"grep","middleware":[{"name":"audit",name:invalid}]});
            assert!(
                schema
                    .resolve(
                        &sources,
                        &layers(vec![json!({"setting":value})]),
                        &origin("code"),
                    )
                    .is_err(),
                "unexpectedly accepted {value}"
            );
        }
    }
}
