// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// You may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use super::*;
use crate::custom_api::openapi::provider_document::parse;
use test_r::test;

fn generated() -> Value {
    json!({"openapi":"3.1.0","info":{"title":"Host","version":"1"},"paths":{}})
}

fn provider(mount: &str, paths: Value) -> ProviderContribution {
    let mut doc = generated();
    doc["paths"] = paths;
    contribution(mount, doc)
}

fn contribution(mount: &str, doc: Value) -> ProviderContribution {
    ProviderContribution {
        router: format!("router:{mount}"),
        mount: mount.into(),
        router_type: "Router".into(),
        component_id: ComponentId::new(),
        document: parse("router", &doc.to_string()).unwrap(),
        mount_security: vec![],
    }
}

fn operation() -> Value {
    json!({"responses":{"200":{"description":"ok"}}})
}

#[test]
fn rebases_semantic_path_references_without_changing_examples() {
    let mut doc = generated();
    doc["paths"] = json!({"/":{"get":operation()},"/a~b/":{"get":{
        "responses":{"200":{"description":"ok","content":{"application/json":{"schema":{"type":"string"},"example":{"$ref":"#/paths/~1a~0b~1/get"}}}}}
    }}});
    doc["components"] = json!({"schemas":{"FromPath":{"$ref":"#/paths/~1a~0b~1/get/responses/200/content/application~1json/schema"}},
        "links":{"Root":{"operationRef":"#/paths/~1/get"},"Tail":{"operationRef":"#/paths/~1a~0b~1/get"}}});
    let result = merge(
        generated(),
        vec![contribution("/mount~root", doc)],
        "https://public.test",
    )
    .unwrap();
    assert!(result["paths"].get("/mount~root").is_some());
    assert!(result["paths"].get("/mount~root/a~b/").is_some());
    assert_eq!(
        result["components"]["links"]["Root"]["operationRef"],
        "#/paths/~1mount~0root/get"
    );
    assert_eq!(
        result["components"]["links"]["Tail"]["operationRef"],
        "#/paths/~1mount~0root~1a~0b~1/get"
    );
    assert_eq!(
        result["components"]["schemas"]["FromPath"]["$ref"],
        "#/paths/~1mount~0root~1a~0b~1/get/responses/200/content/application~1json/schema"
    );
    assert_eq!(
        result["paths"]["/mount~root/a~b/"]["get"]["responses"]["200"]["content"]["application/json"]
            ["example"]["$ref"],
        "#/paths/~1a~0b~1/get"
    );
}

#[test]
fn security_and_distributes_alternatives_preserving_scope_order() {
    let host = requirements(Some(&json!([{"host":["read","host"]},{"alternate":[]}])));
    let provider = requirements(Some(
        &json!([{"host":["write","read"],"provider":["admin"]},{}]),
    ));
    assert_eq!(
        json!(and_security(&host, &provider)),
        json!([
            {"host":["read","host","write"],"provider":["admin"]},
            {"host":["read","host"]},
            {"alternate":[],"host":["write","read"],"provider":["admin"]},
            {"alternate":[]}
        ])
    );
    assert_eq!(and_security(&host, &vec![]), host);
    assert_eq!(and_security(&vec![], &provider), provider);
}

#[test]
fn provider_security_names_must_resolve_before_merging_host_schemes() {
    let mut host = generated();
    host["components"] = json!({"securitySchemes":{"host":{"type":"http","scheme":"bearer"}}});
    let mut doc = generated();
    doc["security"] = json!([{"host":["admin"]}]);
    assert_eq!(
        merge(
            host.clone(),
            vec![contribution("/", doc.clone())],
            "https://public.test"
        )
        .unwrap_err()
        .category,
        Category::Security
    );
    doc["components"] = host["components"].clone();
    doc["paths"] = json!({"/":{"get":operation()}});
    let result = merge(host, vec![contribution("/", doc)], "https://public.test").unwrap();
    assert_eq!(
        result["paths"]["/"]["get"]["security"],
        json!([{"host":["admin"]}])
    );
}

#[test]
fn equivalent_templates_conflict_even_across_methods_but_trailing_slashes_do_not() {
    let mut first = generated();
    first["paths"] = json!({"/a/{x}":{"get":operation()}});
    let mut second = generated();
    second["paths"] = json!({"/a/{y}":{"post":{"parameters":[{"in":"path","name":"y","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"ok"}}}}});
    assert_eq!(
        merge(
            first,
            vec![contribution("/", second)],
            "https://public.test"
        )
        .unwrap_err()
        .category,
        Category::OperationConflict
    );
    let result = merge(
        generated(),
        vec![provider(
            "/",
            json!({"/x":{"get":operation()},"/x/":{"get":operation()}}),
        )],
        "https://public.test",
    )
    .unwrap();
    assert_eq!(result["paths"].as_object().unwrap().len(), 2);
}

#[test]
fn absent_and_empty_parameters_match_but_one_sided_requirements_cannot_leak() {
    let mut first = generated();
    first["paths"] = json!({"/x":{"get":operation()}});
    let result = merge(
        first.clone(),
        vec![provider(
            "/",
            json!({"/x":{"parameters":[],"post":operation()}}),
        )],
        "https://public.test",
    )
    .unwrap();
    assert!(result["paths"]["/x"].get("get").is_some());
    let error = merge(first, vec![provider("/", json!({"/x":{"parameters":[{"name":"query","in":"query","schema":{"type":"string"}}],"post":operation()}}))], "https://public.test").unwrap_err();
    assert_eq!(error.category, Category::PathItemConflict);
}

#[test]
fn link_operation_ids_resolve_globally_without_renaming_duplicates() {
    let mut doc = generated();
    doc["components"] = json!({"links":{"Next":{"operationId":"read"}}});
    assert_eq!(
        merge(
            generated(),
            vec![contribution("/", doc.clone())],
            "https://public.test"
        )
        .unwrap_err()
        .category,
        Category::Reference
    );
    let mut host = generated();
    host["paths"] =
        json!({"/typed":{"get":{"operationId":"read","responses":{"200":{"description":"ok"}}}}});
    merge(
        host.clone(),
        vec![contribution("/", doc.clone())],
        "https://public.test",
    )
    .unwrap();
    doc["paths"] =
        json!({"/custom":{"post":{"operationId":"read","responses":{"200":{"description":"ok"}}}}});
    assert_eq!(
        merge(host, vec![contribution("/", doc)], "https://public.test")
            .unwrap_err()
            .category,
        Category::OperationIdConflict
    );
}

#[test]
fn canonical_provider_order_controls_arrays_and_json_is_key_sorted() {
    let make = |name: &str, mount: &str, component: u128| {
        let mut doc = generated();
        doc["tags"] = json!([{"name":name}]);
        doc["x-order"] = json!({"z":1,"a":2});
        let mut item = contribution(mount, doc);
        item.router_type = name.into();
        item.component_id = ComponentId(uuid::Uuid::from_u128(component));
        item
    };
    let first = merge(
        generated(),
        vec![
            make("z", "/a", 1),
            make("first", "/a", 2),
            make("last", "/z", 1),
        ],
        "https://public.test",
    )
    .unwrap();
    let second = merge(
        generated(),
        vec![
            make("last", "/z", 1),
            make("z", "/a", 1),
            make("first", "/a", 2),
        ],
        "https://public.test",
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert_eq!(
        first["tags"],
        json!([{"name":"first"},{"name":"z"},{"name":"last"}])
    );
    assert_eq!(
        serde_json::to_string(&first["x-order"]).unwrap(),
        r#"{"a":2,"z":1}"#
    );
    assert_eq!(
        serde_yaml::from_str::<Value>(&serde_yaml::to_string(&first).unwrap()).unwrap(),
        first
    );
}

#[test]
fn host_scheme_conflicts_fail_even_when_provider_operation_clears_security() {
    let mut host = generated();
    host["components"] =
        json!({"securitySchemes":{"host":{"type":"apiKey","in":"header","name":"x-session"}}});
    let mut doc = generated();
    doc["components"] = json!({"securitySchemes":{"host":{"type":"http","scheme":"bearer"}}});
    doc["paths"] = json!({"/":{"get":{"security":[],"responses":{"200":{"description":"ok"}}}}});
    let mut provider = contribution("/", doc);
    provider.mount_security = requirements(Some(&json!([{"host":[]}])));
    let error = merge(host, vec![provider], "https://public.test").unwrap_err();
    assert_eq!(error.category, Category::ComponentConflict);
    assert_eq!(error.location, "/components/securitySchemes/host");
}

#[test]
fn metadata_and_extensions_merge_only_when_deeply_equal() {
    let mut doc = generated();
    doc["info"]["title"] = json!("Provider");
    doc["servers"] = json!([{"url":"https://private.test"}]);
    doc["externalDocs"] = json!({"url":"https://docs.test","description":"documentation"});
    doc["x-order"] = json!(["first", "second"]);
    doc["tags"] =
        json!([{"name":"tag","description":"full","externalDocs":{"url":"https://tags.test"}}]);
    let result = merge(
        generated(),
        vec![
            contribution("/a", doc.clone()),
            contribution("/b", doc.clone()),
        ],
        "https://public.test",
    )
    .unwrap();
    assert_eq!(result["tags"], doc["tags"]);
    assert_eq!(result["externalDocs"], doc["externalDocs"]);
    assert_eq!(result["info"]["title"], "Host");
    assert_eq!(result["servers"], json!([{"url":"https://public.test"}]));
    for field in ["x-order", "externalDocs"] {
        let mut changed = doc.clone();
        changed[field] = if field == "x-order" {
            json!(["second", "first"])
        } else {
            json!({"url":"https://other.test"})
        };
        assert_eq!(
            merge(
                generated(),
                vec![contribution("/a", doc.clone()), contribution("/b", changed)],
                "https://public.test"
            )
            .unwrap_err()
            .category,
            Category::ExtensionConflict
        );
    }
}

#[test]
fn openapi_document_corpus_exercises_parser_and_merge() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json"
    ))
    .unwrap();
    let mut count = 0;
    for case in corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["suite"] == "openapi" && case["input"].get("providers").is_some())
    {
        count += 1;
        let id = case["id"].as_str().unwrap();
        let input = &case["input"];
        let mut host = generated();
        if let Some(fields) = input.get("generated") {
            for (key, value) in fields.as_object().unwrap() {
                host[key] = value.clone();
            }
        }
        let providers: Result<Vec<_>, _> = input["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                Ok(ProviderContribution {
                    router: id.into(),
                    mount: p["mount"].as_str().unwrap().into(),
                    router_type: "Router".into(),
                    component_id: ComponentId::new(),
                    document: parse(id, &p["document"].to_string())?,
                    mount_security: requirements(p.get("mount_security")),
                })
            })
            .collect();
        let result = providers.and_then(|providers| {
            merge(
                host,
                providers,
                input
                    .get("public_origin")
                    .and_then(Value::as_str)
                    .unwrap_or("https://public.test"),
            )
        });
        let expected = &case["expect"];
        if let Some(error) = expected.get("error") {
            let actual = result.expect_err(id);
            let category = match error.as_str().unwrap() {
                "operation-conflict" => Category::OperationConflict,
                "path-item-conflict" => Category::PathItemConflict,
                "component-conflict" => Category::ComponentConflict,
                "operation-id-conflict" => Category::OperationIdConflict,
                "tag-conflict" => Category::TagConflict,
                "unsupported-reference" | "dangling-reference" => Category::Reference,
                "unsupported-field" => Category::Unsupported,
                other => panic!("unhandled corpus error {other}"),
            };
            assert_eq!(actual.category, category, "{id}");
            if let Some(location) = expected.get("location") {
                assert_eq!(actual.location, location.as_str().unwrap(), "{id}");
            }
            continue;
        }
        let result = result.unwrap_or_else(|error| panic!("{id}: {error:?}"));
        if let Some(paths) = expected.get("paths") {
            assert_eq!(
                json!(
                    result["paths"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .collect::<Vec<_>>()
                ),
                *paths,
                "{id}"
            );
        }
        if let Some(names) = expected.get("component_schema_names") {
            assert_eq!(
                json!(
                    result["components"]["schemas"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .collect::<Vec<_>>()
                ),
                *names,
                "{id}"
            );
        }
        for key in ["servers", "tags"] {
            if let Some(value) = expected.get(key) {
                assert_eq!(result[key], *value, "{id}");
            }
        }
        if let Some(security) = expected.get("operation_security") {
            for (operation, expected) in security.as_object().unwrap() {
                let (path, method) = operation.rsplit_once(':').unwrap();
                assert_eq!(result["paths"][path][method]["security"], *expected, "{id}");
            }
        }
        if let Some(examples) = expected.get("schema_examples") {
            assert_eq!(
                result["components"]["schemas"]["A"]["examples"], *examples,
                "{id}"
            );
        }
        if let Some(refs) = expected.get("operation_refs") {
            assert_eq!(
                json!([
                    result["paths"]["/api/a~b"]["get"]["responses"]["200"]["links"]["self"]["operationRef"]
                ]),
                *refs,
                "{id}"
            );
        }
        assert!(result.get("security").is_none(), "{id}");
        assert_eq!(
            serde_yaml::from_str::<Value>(&serde_yaml::to_string(&result).unwrap()).unwrap(),
            result,
            "{id}"
        );
    }
    assert_eq!(count, 20);
}
