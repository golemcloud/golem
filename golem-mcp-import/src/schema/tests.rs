use super::*;
use golem_schema::schema::validation::validate_value;
use serde_json::json;
use test_r::test;

fn project(schema: Value) -> Projection {
    Projection::new(schema, Limits::default()).unwrap()
}

fn roundtrip(p: &Projection, json: Value) -> SchemaValue {
    let value = p.from_json(&json).unwrap();
    validate_value(p.graph(), p.root(), &value).unwrap();
    assert_eq!(p.to_json(&value).unwrap(), json);
    value
}

#[test]
fn open_objects_and_presence_preserve_values() {
    let p = project(json!({
        "type":"object", "properties": {
            "fixed":{"type":"string"},
            "nullable":{"type":["null","string"]}
        }, "required":["fixed"]
    }));
    let absent = roundtrip(&p, json!({"fixed":"yes","other":{"nested":[1,false,null]}}));
    let null = roundtrip(
        &p,
        json!({"fixed":"yes","nullable":null,"other":{"nested":[1,false,null]}}),
    );
    assert_ne!(absent, null);
    roundtrip(&p, json!({"fixed":"yes","nullable":"value"}));
    let decoded: Projection = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
    roundtrip(&decoded, json!({"fixed":"again","nullable":null}));
}

#[test]
fn nullable_ref_preserves_presence_and_recursive_containers_work() {
    let p = project(json!({
        "$defs":{"item":{"type":["string","null"]}},
        "type":"object", "properties":{"item":{"$ref":"#/$defs/item"}},
        "additionalProperties":false
    }));
    assert_ne!(
        roundtrip(&p, json!({})),
        roundtrip(&p, json!({"item":null}))
    );
    let recursive = project(json!({
        "$defs":{"node":{"type":"object","properties":{
            "label":{"type":"string"}, "children":{"type":"array","items":{"$ref":"#/$defs/node"}}
        },"required":["label","children"],"additionalProperties":false}},
        "$ref":"#/$defs/node"
    }));
    roundtrip(
        &recursive,
        json!({"label":"root","children":[{"label":"leaf","children":[]}]}),
    );
}

#[test]
fn integer_domain_and_float_conversion_never_saturate_or_round() {
    let signed = project(json!({"type":"integer"}));
    assert_eq!(roundtrip(&signed, json!(-19)), SchemaValue::S64(-19));
    assert_eq!(
        signed.from_json(&json!(100.0)).unwrap(),
        SchemaValue::S64(100)
    );
    assert!(
        signed
            .from_json(&json!(9_223_372_036_854_775_808u64))
            .is_err()
    );
    assert!(
        signed
            .from_json(&json!(9_223_372_036_854_775_808.0))
            .is_err()
    );
    let unsigned = project(json!({"type":"integer","minimum":0}));
    assert_eq!(
        roundtrip(&unsigned, json!(u64::MAX)),
        SchemaValue::U64(u64::MAX)
    );
    assert!(
        unsigned
            .from_json(&json!(18_446_744_073_709_551_616.0))
            .is_err()
    );
    let number = project(json!({"type":"number"}));
    assert!(number.from_json(&json!(9_007_199_254_740_993u64)).is_err());
    assert!(number.from_json(&json!(u64::MAX)).is_err());
    assert_eq!(
        number.from_json(&json!(9_007_199_254_740_992u64)).unwrap(),
        SchemaValue::F64(9_007_199_254_740_992.0)
    );
    let bounded = project(json!({"type":"integer","exclusiveMinimum":-129,"maximum":127.9}));
    assert_eq!(roundtrip(&bounded, json!(-128)), SchemaValue::S8(-128));
    assert!(bounded.from_json(&json!(128)).is_err());
    let wide = project(json!({"type":"integer","maximum":1e30}));
    assert_eq!(
        roundtrip(&wide, json!(i64::MAX)),
        SchemaValue::S64(i64::MAX)
    );
    assert!(wide.from_json(&json!(u64::MAX)).is_err());
    let wide = project(json!({"type":"integer","minimum":0,"maximum":1e30}));
    assert_eq!(
        roundtrip(&wide, json!(u64::MAX)),
        SchemaValue::U64(u64::MAX)
    );
}

#[test]
fn original_constraints_and_value_kinds_are_enforced_both_ways() {
    let p = project(json!({"type":"object", "properties":{
        "word":{"type":"string","pattern":"^[a-z]+$","minLength":2},
        "values":{"type":"array","items":{"type":"integer"},"minItems":2,"uniqueItems":true}
    },"required":["word","values"],"additionalProperties":false}));
    let good = roundtrip(&p, json!({"word":"ok","values":[2,-4]}));
    assert!(p.from_json(&json!({"word":"x","values":[2,-4]})).is_err());
    assert!(p.from_json(&json!({"word":"ok","values":[2,2]})).is_err());
    let mut wrong = good;
    let SchemaValue::Record { fields } = &mut wrong else {
        panic!()
    };
    fields[1] = SchemaValue::String("X".into());
    assert!(p.to_json(&wrong).is_err());
    let integer = project(json!({"type":"integer","minimum":0,"maximum":255}));
    assert!(integer.to_json(&SchemaValue::U64(8)).is_err());
}

#[test]
fn extras_cannot_supply_an_absent_declared_field() {
    let p = project(json!({"type":"object","properties":{"known":{"type":"string"}}}));
    let mut value = p.from_json(&json!({})).unwrap();
    let SchemaValue::Record { fields } = &mut value else {
        panic!()
    };
    let SchemaValue::Map { entries } = fields.last_mut().unwrap() else {
        panic!()
    };
    entries.push((
        SchemaValue::String("known".into()),
        SchemaValue::String("\"bypass\"".into()),
    ));
    assert!(p.to_json(&value).is_err());
}

#[test]
fn dialect_references_and_unsupported_shapes_fail_explicitly() {
    for value in [
        json!({"$schema":"https://example.com/not-2020-12","type":"string"}),
        json!({"$ref":"https://example.invalid/schema"}),
        json!({"$ref":"file:///etc/passwd"}),
        json!({"$dynamicRef":"#node"}),
        json!({"type":["string","integer","null"]}),
    ] {
        assert!(Projection::new(value, Limits::default()).is_err());
    }
    // These are property names, not schema keywords.
    let p = project(
        json!({"type":"object","properties":{"$dynamicRef":{"type":"string"}},"additionalProperties":false}),
    );
    roundtrip(&p, json!({"$dynamicRef":"ordinary"}));
}

#[test]
fn binary_and_escaped_refs_preserve_payloads() {
    let p = project(
        json!({"$defs":{"a/b~c":{"type":"string","contentEncoding":"base64"}},"$ref":"#/$defs/a~1b~0c"}),
    );
    let value = roundtrip(&p, json!("AP9h"));
    assert_eq!(
        value,
        SchemaValue::Binary(BinaryValuePayload {
            bytes: vec![0, 255, 97],
            mime_type: None
        })
    );
    assert!(p.from_json(&json!("bad%")).is_err());
    let p = project(
        json!({"$defs":{"a b/é":{"type":"string"}},"$ref":"#/$defs/a%20b~1%C3%A9","format":"uuid"}),
    );
    roundtrip(&p, json!("format is an annotation"));
}

#[test]
fn schema_and_instance_bounds_are_inclusive() {
    let source = json!({"type":"string"});
    let bytes = source.to_string().len();
    let mut limits = Limits {
        schema_bytes: bytes,
        schema_nodes: 2,
        schema_depth: 1,
        instance_bytes: 4,
        instance_depth: 0,
    };
    let p = Projection::new(source.clone(), limits).unwrap();
    roundtrip(&p, json!("ab"));
    assert!(p.from_json(&json!("abc")).is_err());
    limits.schema_bytes -= 1;
    assert!(Projection::new(source.clone(), limits).is_err());
    limits.schema_bytes = bytes;
    limits.schema_nodes = 1;
    assert!(Projection::new(source.clone(), limits).is_err());
    limits.schema_nodes = 2;
    limits.schema_depth = 0;
    assert!(Projection::new(source, limits).is_err());
}

#[test]
fn tagged_unions_preserve_branch_and_validate_original_constraints() {
    let p = project(json!({"oneOf":[
        {"type":"object","properties":{"kind":{"type":"string","const":"count"},"value":{"type":"integer","minimum":3}},"required":["kind","value"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"type":"string","const":"text"},"value":{"type":"string","pattern":"^abc"}},"required":["kind","value"],"additionalProperties":false}
    ]}));
    let count = roundtrip(&p, json!({"kind":"count","value":7}));
    let text = roundtrip(&p, json!({"kind":"text","value":"abcdef"}));
    let (SchemaValue::Union(count), SchemaValue::Union(text)) = (count, text) else {
        panic!("expected typed unions")
    };
    assert_ne!(count.tag, text.tag);
    assert!(
        p.from_json(&json!({"kind":"text","value":"wrong"}))
            .is_err()
    );
    assert!(p.from_json(&json!({"kind":"count","value":2})).is_err());
}

#[test]
fn conjunction_and_pattern_properties_never_drop_fields() {
    let p = project(json!({"allOf":[
        {"type":"object","properties":{"left":{"type":"string"}},"required":["left"]},
        {"type":"object","properties":{"right":{"type":"integer"}},"required":["right"]}
    ]}));
    assert_eq!(p.root_fields().len(), 2);
    roundtrip(&p, json!({"left":"one","right":-4,"extra":true}));
    assert!(p.from_json(&json!({"left":"one"})).is_err());
    let p = project(
        json!({"type":"object","patternProperties":{"^x-":{"type":"integer"}},"additionalProperties":false}),
    );
    roundtrip(&p, json!({"x-a":5,"x-b":-11}));
    assert!(p.from_json(&json!({"x-a":"wrong"})).is_err());
    assert!(p.from_json(&json!({"y-a":5})).is_err());
    let p = project(
        json!({"type":"string","not":{"const":"forbidden"},"if":{"minLength":3},"then":{"pattern":"^ok"}}),
    );
    roundtrip(&p, json!("ok!"));
    assert!(p.from_json(&json!("forbidden")).is_err());
    assert!(p.from_json(&json!("bad")).is_err());
}

#[test]
fn unsigned_enum_and_conversion_error_path_are_precise() {
    let p = project(json!({"enum":[0,u64::MAX]}));
    assert_eq!(roundtrip(&p, json!(u64::MAX)), SchemaValue::U64(u64::MAX));
    let p = project(
        json!({"type":"object","properties":{"a/b":{"type":"number"}},"required":["a/b"],"additionalProperties":false}),
    );
    assert!(
        matches!(p.from_json(&json!({"a/b":9_007_199_254_740_993u64})), Err(Error::Value { path, .. }) if path == "/a~1b")
    );
    let value = SchemaValue::List {
        elements: vec![SchemaValue::String("x".repeat(1024))],
    };
    assert!(matches!(
        check_value(
            &value,
            Limits {
                instance_bytes: 1023,
                ..Default::default()
            }
        ),
        Err(Error::Limit(_))
    ));
    let mut nested = SchemaValue::String("x".into());
    for _ in 0..20 {
        nested = SchemaValue::List {
            elements: vec![nested],
        };
    }
    assert!(matches!(
        check_value(
            &nested,
            Limits {
                instance_depth: 1,
                schema_depth: 1,
                ..Default::default()
            }
        ),
        Err(Error::Limit(_))
    ));
}

#[test]
fn nullable_choices_local_conjunction_refs_and_structural_siblings() {
    let p = project(json!({"$defs":{"nil":{"type":"null"}},"anyOf":[
        {"type":"object","properties":{"kind":{"const":"a"},"x":{"type":"integer"}},"required":["kind","x"]},
        {"type":"object","properties":{"kind":{"const":"b"},"y":{"type":"string"}},"required":["kind","y"]},
        {"$ref":"#/$defs/nil"}
    ]}));
    roundtrip(&p, json!({"kind":"a","x":-5}));
    roundtrip(&p, json!({"kind":"b","y":"hello"}));
    assert_eq!(
        roundtrip(&p, Value::Null),
        SchemaValue::Option { inner: None }
    );
    let p = project(
        json!({"$defs":{"item":{"type":"string","minLength":3}},"allOf":[{"$ref":"#/$defs/item"},{"description":"Reference with constraints","pattern":"^abc"}]}),
    );
    roundtrip(&p, json!("abc!"));
    assert!(p.from_json(&json!("other")).is_err());
    let p = project(
        json!({"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"boolean"}},"anyOf":[{"required":["a"]},{"required":["b"]}],"additionalProperties":false}),
    );
    roundtrip(&p, json!({"a":-9}));
    roundtrip(&p, json!({"b":false}));
    assert!(p.from_json(&json!({})).is_err());
    let p = project(json!({"anyOf":[{"const":"left"},{"const":"right"}]}));
    assert_ne!(roundtrip(&p, json!("left")), roundtrip(&p, json!("right")));
}

#[test]
fn alias_chain_times_instance_depth_stays_within_stack() {
    std::thread::Builder::new().stack_size(2 * 1024 * 1024).spawn(|| {
        for (count, conjunction) in [(124, false), (62, true)] {
        let mut defs = Map::new();
        defs.insert("node".into(), json!({"type":"object","properties":{"child":{"$ref":"#/$defs/h0"}},"additionalProperties":false}));
        for i in 0..count {
            let next = if i == count - 1 { "#/$defs/node".to_owned() } else { format!("#/$defs/h{}", i + 1) };
            let branch = if conjunction { json!({"allOf":[{"$ref":next}]}) } else { json!({"$ref":next,"description":format!("Alias {i}")}) };
            defs.insert(format!("h{i}"), branch);
        }
        let p = project(json!({"type":"object","properties":{"root":{"$ref":"#/$defs/node"}},"additionalProperties":false,"$defs":defs}));
        let mut instance = json!({});
        for _ in 0..255 { instance = json!({"child":instance}); }
        roundtrip(&p, json!({"root":instance}));
        }
    }).unwrap().join().unwrap();
}

#[test]
fn validation_only_reference_chains_cannot_bypass_depth_limits() {
    let mut defs = Map::new();
    for i in 0..140 {
        let value = if i == 139 {
            json!({"const":"forbidden"})
        } else {
            json!({"$ref":format!("#/$defs/h{}", i + 1)})
        };
        defs.insert(format!("h{i}"), value);
    }
    let source = json!({"type":"string","not":{"$ref":"#/$defs/h0"},"$defs":defs});
    assert!(matches!(
        Projection::new(source, Limits::default()),
        Err(Error::Limit(_))
    ));
    assert!(
        Projection::new(
            json!({"type":"string"}),
            Limits {
                instance_depth: 257,
                ..Default::default()
            }
        )
        .is_err()
    );
}
