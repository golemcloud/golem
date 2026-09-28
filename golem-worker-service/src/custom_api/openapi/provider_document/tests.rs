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
use serde_json::json;
use test_r::test;

fn document() -> Value {
    json!({"openapi":"3.1.0", "info":{"title":"Provider", "version":"1"}, "paths":{}})
}

fn accepted(value: &Value) -> ProviderDocument {
    parse("router", &value.to_string()).unwrap_or_else(|error| panic!("{error:?}"))
}

fn rejected(value: &Value) -> DocumentError {
    parse("router", &value.to_string())
        .err()
        .expect("must reject")
}

#[test]
fn validates_openapi_and_schema_objects_offline() {
    accepted(&document());
    for invalid in [
        json!({"type":42}),
        json!({"required":"field"}),
        json!({"properties":{"a":3}}),
        json!({"discriminator":{"mapping":{"x":42}}}),
    ] {
        let mut value = document();
        value["components"] = json!({"schemas":{"Invalid":invalid}});
        assert_eq!(rejected(&value).category, Category::Structure);
    }
    for (key, value) in [
        ("info", json!({"title":"missing version"})),
        ("servers", json!([{"url":3}])),
        ("openapi", json!("3.1.1")),
        ("unknown", json!(true)),
    ] {
        let mut doc = document();
        doc[key] = value;
        assert_eq!(rejected(&doc).category, Category::Structure);
    }
}

#[test]
fn rejects_duplicate_keys_invalid_unicode_numbers_roots_and_trailing_json() {
    for text in [
        r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{},"x-data":{"a":1,"\u0061":2}}"#,
        r#"{"openapi":"3.1.0","info":{"title":"\ud800","version":"1"},"paths":{}}"#,
        r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{},"x-number":1e999}"#,
        r#"{"openapi":"3.1.0","info":{"title":"x","version":"1"},"paths":{}} false"#,
        "null",
        "[]",
        "true",
        "NaN",
    ] {
        assert!(parse("router", text).is_err(), "{text}");
    }
}

#[test]
fn inclusive_container_depth_and_utf8_byte_limit() {
    let base = document().to_string();
    for (depth, valid) in [(64, true), (65, false)] {
        let text = format!(
            "{},\"x-data\":{}0{}}}",
            &base[..base.len() - 1],
            "[".repeat(depth - 1),
            "]".repeat(depth - 1)
        );
        assert_eq!(parse("router", &text).is_ok(), valid);
    }
    let overhead = base.len() + ",\"x-data\":\"\"".len();
    let padding = "a".repeat(PROVIDER_BYTE_LIMIT - overhead - "λ".len());
    let text = format!("{},\"x-data\":\"{padding}λ\"}}", &base[..base.len() - 1]);
    assert_eq!(text.len(), PROVIDER_BYTE_LIMIT);
    assert!(parse("router", &text).is_ok());
    assert_eq!(
        parse("router", &(text + " ")).err().unwrap().category,
        Category::Size
    );
}

#[test]
fn schema_references_are_local_typed_recursive_and_pointer_decoded() {
    let mut doc = document();
    doc["components"] = json!({"schemas": {
        "Node": {"properties": {"a/b~c":true, "next":{"$ref":"#/components/schemas/Node"}}},
        "Ref": {"$ref":"#/components/schemas/Node/properties/a~1b%7E0c"}
    }, "responses":{"Ok":{"description":"ok"}}});
    let parsed = accepted(&doc);
    assert_eq!(parsed.references.len(), 2);
    assert!(
        parsed
            .references
            .iter()
            .any(|r| r.target == "/components/schemas/Node/properties/a~1b~0c")
    );
    for target in [
        "https://example.com/schema",
        "schema.json",
        "#anchor",
        "#/components/schemas/Missing",
        "#/components/responses/Ok",
        "#/components/schemas/Node/properties/a~2b",
        "#/components/schemas/Node/properties/a%zz",
        "#/components/schemas/Node/properties/a%",
    ] {
        doc["components"]["schemas"]["Ref"]["$ref"] = json!(target);
        assert_eq!(rejected(&doc).category, Category::Reference, "{target}");
    }
}

#[test]
fn opaque_payloads_are_not_interpreted_as_references_or_schema_keywords() {
    let opaque = json!({"$ref":"https://private.invalid/secret", "$id":"private", "servers":[], "externalValue":"secret"});
    let mut doc = document();
    doc["x-payload"] = opaque.clone();
    doc["components"] = json!({
        "schemas":{"S":{"example":opaque, "default":opaque, "const":opaque, "enum":[opaque], "examples":[opaque], "x-custom":opaque}},
        "examples":{"x-example":{"value":opaque}},
        "responses":{"x-response":{"description":"ok", "content":{"application/json":{"example":opaque}}}}
    });
    let parsed = accepted(&doc);
    assert!(parsed.references.is_empty());
    assert_eq!(parsed.value, doc);
}

#[test]
fn rejects_unsupported_semantics_even_when_empty() {
    for (pointer, value) in [
        ("/webhooks", json!({})),
        (
            "/jsonSchemaDialect",
            json!("https://spec.openapis.org/oas/3.1/dialect/base"),
        ),
        ("/components", json!({"callbacks":{}})),
        ("/components", json!({"pathItems":{}})),
        ("/paths", json!({"/":{"servers":[]}})),
        (
            "/paths",
            json!({"/":{"get":{"responses":{"200":{"description":"ok"}},"servers":[]}}}),
        ),
        (
            "/paths",
            json!({"/":{"get":{"responses":{"200":{"description":"ok"}},"callbacks":{}}}}),
        ),
        (
            "/components",
            json!({"examples":{"E":{"externalValue":"https://example.com/e"}}}),
        ),
    ] {
        let mut doc = document();
        doc[pointer.strip_prefix('/').unwrap()] = value;
        assert_eq!(rejected(&doc).category, Category::Unsupported, "{pointer}");
    }
    let mut doc = document();
    doc["paths"] = json!({"/":{"$ref":"#/paths/~1other"},"/other":{}});
    assert_eq!(rejected(&doc).category, Category::Unsupported);
    for keyword in ["$id", "$anchor", "$dynamicAnchor", "$dynamicRef", "$schema"] {
        let mut doc = document();
        doc["components"] = json!({"schemas":{"S":{"properties":{"nested":{keyword:"scope"}}}}});
        assert_eq!(rejected(&doc).category, Category::Unsupported, "{keyword}");
    }
}

#[test]
fn discriminator_and_link_references_resolve_only_to_correct_targets() {
    let mut doc = document();
    doc["paths"] = json!({"/a/~value":{"get":{"operationId":"target", "responses":{"200":{"description":"ok"}}}}});
    doc["components"] = json!({"schemas":{"S":{"discriminator":{"propertyName":"kind", "mapping":{"s":"#/components/schemas/S"}}}},
        "links":{"Next":{"operationRef":"#/paths/~1a~1~0value/get"}}});
    assert_eq!(accepted(&doc).references.len(), 2);
    doc["components"]["links"]["Next"]["operationRef"] = json!("#/paths/~1a~1~0value");
    assert_eq!(rejected(&doc).category, Category::Reference);
    doc["components"]["links"]["Next"]["operationRef"] = json!("#/paths/~1a~1~0value/get");
    doc["components"]["schemas"]["S"]["discriminator"]["mapping"]["s"] = json!("S");
    assert_eq!(rejected(&doc).category, Category::Reference);
}

#[test]
fn paths_use_shared_literal_rules_and_whole_segment_templates() {
    for path in ["/", "/a/", "/a/{id}", "/a/{id}/", "/a/%CE%BB", "/a/~value"] {
        assert!(validate_path(path).is_ok(), "{path}");
    }
    for path in [
        "",
        "a",
        "//",
        "/a//b",
        "/a/../b",
        "/a/%2e%2e/b",
        "/a/%2fb",
        "/a?b",
        "/a#b",
        "/a/*",
        "/a/{id}.json",
        "/a/{}",
        "/a/{id",
        "/a/%zz",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
}

#[test]
fn diagnostics_do_not_include_values_or_library_messages() {
    let mut doc = document();
    doc["components"] = json!({"schemas":{"S":{"$ref":"https://secret.invalid/token"}}});
    let error = rejected(&doc);
    assert_eq!(
        error,
        DocumentError {
            router: "router".into(),
            category: Category::Reference,
            section: "components",
            location: "/components/schemas/S/$ref".into()
        }
    );
    assert!(!format!("{error:?}").contains("secret"));
}

#[test]
fn reference_objects_ignore_siblings_but_schemas_do_not() {
    let mut doc = document();
    doc["components"] = json!({"responses":{
        "R":{"description":"ok"},
        "Ref":{"$ref":"#/components/responses/R", "description":"retained", "headers":42, "content":false}
    }});
    let parsed = accepted(&doc);
    assert_eq!(parsed.references.len(), 1);
    assert_eq!(
        parsed.value["components"]["responses"]["Ref"],
        json!({"$ref":"#/components/responses/R", "description":"retained"})
    );
    doc["components"]["schemas"] = json!({"S":{"$ref":"#/components/schemas/S", "properties":42}});
    assert_eq!(rejected(&doc).category, Category::Structure);
}

#[test]
fn parameters_resolve_before_checking_duplicates_and_templates() {
    let mut doc = document();
    let parameter = json!({"name":"id", "in":"path", "required":true, "schema":{"type":"string"}});
    doc["components"] = json!({"parameters":{"Id":parameter}});
    doc["paths"] = json!({"/{id}":{"parameters":[{"$ref":"#/components/parameters/Id"}],"get":{"responses":{"200":{"description":"ok"}}}}});
    accepted(&doc);
    doc["paths"]["/{id}"]["get"]["parameters"] = json!([parameter]);
    accepted(&doc); // An operation may override a path-level parameter.
    doc["paths"]["/{id}"]["get"]["parameters"] = json!([parameter, parameter]);
    assert_eq!(rejected(&doc).category, Category::Structure);
    doc["paths"]["/{id}"]["get"]["parameters"] = json!([]);
    doc["paths"]["/{id}"]["parameters"] = json!([]);
    assert_eq!(rejected(&doc).category, Category::Path);
    doc["paths"] = json!({"/literal":{"get":{"parameters":[parameter],"responses":{"200":{"description":"ok"}}}}});
    assert_eq!(rejected(&doc).category, Category::Path);
}

#[test]
fn rejects_unmatched_path_item_parameter_without_operations() {
    let mut doc = document();
    doc["paths"] = json!({
        "/literal": {
            "parameters": [{
                "name": "id",
                "in": "path",
                "required": true,
                "schema": {"type": "string"}
            }]
        }
    });

    assert_eq!(rejected(&doc).category, Category::Path);
    doc["paths"]["/{id}"] = doc["paths"]["/literal"].clone();
    doc["paths"].as_object_mut().unwrap().remove("/literal");
    accepted(&doc);
}

#[test]
fn non_schema_reference_cycles_do_not_resolve_to_concrete_objects() {
    let mut doc = document();
    doc["components"] = json!({"responses":{
        "A":{"$ref":"#/components/responses/B"},
        "B":{"$ref":"#/components/responses/A"}
    }});
    assert_eq!(rejected(&doc).category, Category::Reference);
}

#[test]
fn accepts_templated_root_server_url() {
    let mut doc = document();
    doc["servers"] = json!([{
        "url": "https://{username}.example.com:{port}/{basePath}",
        "variables": {
            "username": {"default": "demo"},
            "port": {"default": "8443"},
            "basePath": {"default": "v1"}
        }
    }]);
    accepted(&doc);
}

#[test]
fn accepts_link_server_and_opaque_literal_parameter_values() {
    let mut doc = document();
    doc["components"] = json!({"links":{"Next":{
        "operationId":"read", "server":{"url":"https://{env}.example.com", "variables":{"env":{"default":"dev"}}},
        "parameters":{"id":42,"body":{"$ref":"https://example.com/data"}},
        "requestBody":{"operationRef":"not a reference"}
    }}});
    let parsed = accepted(&doc);
    assert_eq!(parsed.value, doc);
    assert!(parsed.references.is_empty());
}

#[test]
fn legacy_schema_scope_keywords_are_outside_supported_dialect() {
    for (keyword, value) in [
        (
            "definitions",
            json!({"Old":{"$ref":"https://example.com/old"}}),
        ),
        (
            "dependencies",
            json!({"x":{"$id":"https://example.com/scope"}}),
        ),
        ("$recursiveAnchor", json!(true)),
        ("$recursiveRef", json!("https://example.com/recursive")),
    ] {
        let mut doc = document();
        doc["components"] = json!({"schemas":{"S":{keyword:value}}});
        assert_eq!(rejected(&doc).category, Category::Unsupported, "{keyword}");
    }
}

#[test]
fn schema_structure_is_checked_but_patterns_are_not_compiled() {
    let mut doc = document();
    doc["components"] = json!({"schemas":{"S":{"pattern":"(?=prefix).*"}}});
    accepted(&doc);
    for schema in [
        json!(42),
        json!({"properties":{"nested":{"discriminator":{"mapping":{"x":42}}}}}),
    ] {
        doc["components"]["schemas"]["S"] = schema;
        assert_eq!(rejected(&doc).category, Category::Structure);
    }
}

#[test]
fn pointer_fragments_require_uri_encoding_separate_from_json_pointer_escaping() {
    let mut doc = document();
    doc["components"] = json!({"schemas":{"S":{"properties":{"λ space":true}},"Ref":{"$ref":"#/components/schemas/S/properties/%CE%BB%20space"}}});
    assert_eq!(
        accepted(&doc).references[0].target,
        "/components/schemas/S/properties/λ space"
    );
    doc["components"]["schemas"]["Ref"]["$ref"] =
        json!("#/components/schemas/S/properties/λ space");
    assert_eq!(rejected(&doc).category, Category::Reference);
}
