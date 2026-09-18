use super::*;
use golem_schema::schema::{
    render::{from_untrusted_json_value, to_json_value},
    validation::validate_value,
};
use serde_json::json;
use test_r::test;

fn definition(name: &str) -> Value {
    json!({"name":name,"inputSchema":{"type":"object","properties":{},"additionalProperties":false}})
}

fn names(batch: &Batch) -> Vec<&str> {
    batch
        .tools
        .iter()
        .map(|t| t.definition.name().unwrap())
        .collect()
}

#[test]
fn filtering_collision_exclusions_and_precedence() {
    assert_eq!(
        sanitize_name("__slack:Post--Message!"),
        "slack-post-message"
    );
    let upstream = vec![
        definition("get_weather"),
        definition("get-Weather"),
        definition("read_file"),
        definition("write_file"),
    ];
    let batch = project_import(
        &upstream,
        Some("fs"),
        Some(&["get-*".into(), "read-*".into()]),
        None,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(names(&batch), vec!["fs-read-file"]);
    assert_eq!(batch.diagnostics.len(), 2);
    assert!(
        batch
            .diagnostics
            .iter()
            .all(|d| d.reason.contains("ambiguous"))
    );
    let first = project_import(
        &[definition("shared"), definition("native")],
        None,
        None,
        None,
        Limits::default(),
    )
    .unwrap();
    let second = project_import(
        &[definition("shared"), definition("last")],
        None,
        None,
        None,
        Limits::default(),
    )
    .unwrap();
    let MergedImports { tools, diagnostics } =
        merge_imports(&BTreeSet::from(["native".into()]), vec![first, second]);
    assert_eq!(
        tools
            .iter()
            .map(|(i, t)| (*i, t.upstream_name.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "shared"), (1, "last")]
    );
    assert_eq!(
        diagnostics
            .iter()
            .map(|(i, d)| (*i, d.upstream_name.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "native"), (1, "shared")]
    );
}

#[test]
fn canonical_inputs_restore_exact_json_property_names() {
    let upstream = json!({"name":"fetch","title":"Fetch data","description":"Detailed documentation","inputSchema":{
        "type":"object","properties":{
            "User_ID":{"type":"string","description":"Upstream identity"},
            "count":{"type":"integer","minimum":0,"maximum":255},
            "settings":{"type":"object","properties":{"punctuation.name":{"type":"boolean"}},"required":["punctuation.name"],"additionalProperties":false}
        },"required":["User_ID","count"]
    },"annotations":{"readOnlyHint":true,"destructiveHint":false,"idempotentHint":true,"openWorldHint":false}});
    let projected = ProjectedTool::new(&upstream, "fetch", Limits::default()).unwrap();
    let model = projected.definition.canonical_input_model(0).unwrap();
    let input = from_untrusted_json_value(
        &model.record_schema,
        &model.record_schema.root,
        &json!({
            "user-id":"customer-4","count":7,"settings":{"punctuation.name":false},
            "additional-properties":[["vendor$flag","[false,3]"]]
        }),
    )
    .unwrap();
    assert_eq!(
        projected.arguments(&input).unwrap(),
        json!({"User_ID":"customer-4","count":7,"settings":{"punctuation.name":false},"vendor$flag":[false,3]})
    );
    let body = projected.definition.commands.nodes[0]
        .body
        .as_ref()
        .unwrap();
    assert_eq!(
        body.annotations,
        Some(CommandAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false
        })
    );
    assert_eq!(
        projected.definition.commands.nodes[0].doc.description,
        "Detailed documentation"
    );
    assert_eq!(body.errors[0].name, "mcp-tool-error");
    assert_eq!(body.errors[0].payload, Some(SchemaType::string()));
    let duplicate = json!({"name":"bad","inputSchema":{"type":"object","properties":{"user_id":{"type":"string"},"user-id":{"type":"integer"}}}});
    assert!(
        ProjectedTool::new(&duplicate, "bad", Limits::default())
            .unwrap_err()
            .contains("ambiguous")
    );
}

#[test]
fn structured_values_and_stdout_share_one_stable_result_schema() {
    let mut upstream = definition("describe");
    upstream["outputSchema"] = json!({"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false});
    let tool = ProjectedTool::new(&upstream, "describe", Limits::default()).unwrap();
    let original_definition = tool.definition.clone();
    let result_type = &tool.definition.commands.nodes[0]
        .body
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .type_;
    for (blocks, stdout) in [
        (json!([]), None),
        (json!([{"type":"text","text":""}]), Some(vec![])),
        (
            json!([{"type":"image","data":"AP8=","mimeType":"image/png"}]),
            Some(vec![0, 255]),
        ),
        (
            json!([{"type":"text","text":"a"},{"type":"text","text":"b"}]),
            None,
        ),
        (
            json!([{"type":"resource_link","uri":"https://example.com/r","name":"r"}]),
            None,
        ),
    ] {
        let response = tool
            .response(&json!({"structuredContent":{"answer":-42},"content":blocks}))
            .unwrap();
        assert_eq!(response.stdout, stdout);
        validate_value(&tool.definition.schema, result_type, &response.value).unwrap();
        let json = to_json_value(&tool.definition.schema, result_type, &response.value).unwrap();
        assert_eq!(json["structured"]["answer"], -42);
        assert_eq!(tool.definition, original_definition);
    }
    for invalid in [
        json!({"content":[]}),
        json!({"content":[],"structuredContent":{"answer":"bad"}}),
    ] {
        assert!(matches!(
            tool.response(&invalid),
            Err(CallError::InvalidResult(_))
        ));
    }
    assert_eq!(
        tool.response(
            &json!({"isError":true,"content":[{"type":"text","text":"upstream failed"}]})
        ),
        Err(CallError::ToolError("upstream failed".into()))
    );
    assert_eq!(
        tool.response(&json!({"isError":true,"content":[]})),
        Err(CallError::ToolError("[]".into()))
    );
}

#[test]
fn snapshot_reconstruction_keeps_mappings_and_digests() {
    let mut upstream = definition("snap");
    upstream["inputSchema"] = json!({"$defs":{"item":{"type":"string"}},"type":"object","properties":{"x":{"$ref":"#/$defs/item"}},"required":["x"],"additionalProperties":false});
    upstream["outputSchema"] = json!({"$defs":{"item":{"type":"integer"}},"type":"object","properties":{"y":{"$ref":"#/$defs/item"}},"required":["y"],"additionalProperties":false});
    let p = ProjectedTool::new(&upstream, "snap", Limits::default()).unwrap();
    assert_eq!(p.definition.schema.defs.len(), 2);
    let restored = ProjectedTool::from_json(&p.to_json().unwrap()).unwrap();
    assert_eq!(restored.digest, p.digest);
    assert_eq!(
        restored
            .arguments(&SchemaValue::Record {
                fields: vec![SchemaValue::String("original".into())]
            })
            .unwrap(),
        json!({"x":"original"})
    );
    let result = restored
        .response(&json!({"content":[],"structuredContent":{"y":-9}}))
        .unwrap();
    let ty = &restored.definition.commands.nodes[0]
        .body
        .as_ref()
        .unwrap()
        .result
        .as_ref()
        .unwrap()
        .type_;
    validate_value(&restored.definition.schema, ty, &result.value).unwrap();
    upstream["annotations"] = json!({"readOnlyHint":true});
    assert_ne!(
        p.digest,
        ProjectedTool::new(&upstream, "snap", Limits::default())
            .unwrap()
            .digest
    );
}

#[test]
fn invalid_definitions_do_not_hide_other_tools_and_empty_is_successful() {
    let mut invalid = definition("bad");
    invalid["inputSchema"] =
        json!({"type":"object","properties":{"x":{"type":["string","integer"]}}});
    let batch = project_import(
        &[invalid, definition("good")],
        None,
        None,
        None,
        Limits::default(),
    )
    .unwrap();
    assert_eq!(names(&batch), vec!["good"]);
    assert_eq!(batch.diagnostics.len(), 1);
    let batch = project_import(&[], None, None, None, Limits::default()).unwrap();
    assert!(batch.tools.is_empty() && batch.diagnostics.is_empty());
    let limits = Limits {
        max_tools: 1,
        ..Default::default()
    };
    assert!(project_import(&[definition("one")], None, None, None, limits).is_ok());
    assert!(
        project_import(
            &[definition("one"), definition("two")],
            None,
            None,
            None,
            limits
        )
        .is_err()
    );
}

#[test]
fn object_contracts_optional_docs_and_read_only_defaults() {
    let mut upstream = definition("weather");
    upstream["inputSchema"]["properties"] =
        json!({"location":{"type":"string","description":"City name or zip code"}});
    upstream["annotations"] = json!({"readOnlyHint":true,"openWorldHint":true});
    let projected = ProjectedTool::new(&upstream, "weather", Limits::default()).unwrap();
    let body = projected.definition.commands.nodes[0]
        .body
        .as_ref()
        .unwrap();
    assert_eq!(body.options[0].doc.summary, "City name or zip code");
    assert_eq!(
        body.annotations,
        Some(CommandAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: true
        })
    );
    for value in [json!(null), json!("string"), json!([1])] {
        let response = projected
            .response(&json!({"content":[],"structuredContent":value}))
            .unwrap();
        let SchemaValue::Record { fields } = response.value else {
            panic!("expected result record");
        };
        assert_eq!(
            fields[0],
            SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String(value.to_string())))
            }
        );
    }
    upstream["inputSchema"] = json!({"type":"null"});
    assert!(ProjectedTool::new(&upstream, "weather", Limits::default()).is_err());
}

#[test]
fn structured_output_supports_non_object_schemas() {
    for (schema, value, expected, invalid) in [
        (
            json!({"type":"string"}),
            json!("answer"),
            SchemaValue::String("answer".into()),
            json!(17),
        ),
        (
            json!({"type":"array","items":{"type":"integer"}}),
            json!([7, -3]),
            SchemaValue::List {
                elements: vec![SchemaValue::S64(7), SchemaValue::S64(-3)],
            },
            json!(["wrong"]),
        ),
        (
            json!({"type":"null"}),
            Value::Null,
            SchemaValue::Tuple { elements: vec![] },
            json!({}),
        ),
    ] {
        let mut upstream = definition("output");
        upstream["outputSchema"] = schema;
        let projected = ProjectedTool::new(&upstream, "output", Limits::default()).unwrap();
        let response = projected
            .response(&json!({"content":[],"structuredContent":value}))
            .unwrap();
        let SchemaValue::Record { fields } = response.value else {
            panic!("expected result record");
        };
        assert_eq!(fields[0], expected);
        assert_eq!(response.stdout, None);
        assert!(matches!(
            projected.response(&json!({"content":[],"structuredContent":invalid})),
            Err(CallError::InvalidResult(_))
        ));
        assert!(matches!(
            projected.response(&json!({"content":[]})),
            Err(CallError::InvalidResult(_))
        ));
    }
}

#[test]
fn per_definition_depth_failure_is_not_an_import_failure() {
    let mut invalid = definition("deep");
    invalid["inputSchema"]["properties"] =
        json!({"x":{"type":"array","items":{"type":"array","items":{"type":"string"}}}});
    let mut limits = Limits::default();
    limits.schema.schema_depth = 4;
    let batch = project_import(&[invalid, definition("good")], None, None, None, limits).unwrap();
    assert_eq!(names(&batch), vec!["good"]);
    assert_eq!(batch.diagnostics.len(), 1);
    assert_eq!(batch.diagnostics[0].upstream_name, "deep");
    assert!(batch.diagnostics[0].reason.contains("Depth"));
}

#[test]
fn listing_byte_limit_includes_definitions_rejected_by_per_tool_limits() {
    let mut invalid = definition("oversized");
    invalid["ignored"] = json!("x".repeat(1024));
    let mut limits = Limits::default();
    limits.schema.schema_depth = 0;
    limits.max_listing_bytes = 256;

    assert!(project_import(&[invalid], None, None, None, limits).is_err());
    let values = [json!({"\"key":[null,true,1.5,"hé",{},[]]}), json!(false)];
    let bytes = serde_json::to_vec(&values).unwrap().len();
    assert!(crate::limits::check_array_bytes(&values, bytes).is_ok());
    assert!(crate::limits::check_array_bytes(&values, bytes - 1).is_err());
    let mut nested = json!(0);
    for _ in 0..512 {
        nested = json!([nested]);
    }
    assert!(crate::limits::check_array_bytes(&[nested], 1027).is_ok());
}

#[test]
fn response_does_not_charge_wrapper_and_structured_content_to_content_limit() {
    for declared in [true, false] {
        let mut upstream = definition("bounded");
        if declared {
            upstream["outputSchema"] = json!({
                "type":"object",
                "properties":{"answer":{"type":"string"}},
                "required":["answer"],
                "additionalProperties":false
            });
        }
        let content = json!([{"type":"text","text":"payload"}]);
        let mut limits = Limits::default();
        limits.content.max_total_bytes = serde_json::to_vec(&content).unwrap().len();
        limits.content.max_blocks = 1;
        limits.schema.instance_bytes = r#"{"answer":"ok"}"#.len();
        limits.schema.instance_depth = 1;
        let tool = ProjectedTool::new(&upstream, "bounded", limits).unwrap();

        assert_eq!(
            tool.response(&json!({
                "content": content,
                "structuredContent":{"answer":"ok"},
                "_meta":{"transportOwned":"not charged to content"}
            }))
            .unwrap()
            .stdout,
            Some(b"payload".to_vec())
        );
        assert!(matches!(
            tool.response(&json!({"content":content,"structuredContent":{"answer":"too long"}})),
            Err(CallError::InvalidResult(_))
        ));
        assert!(matches!(tool.response(&json!({"content":[{"type":"text","text":"payload!"}],"structuredContent":{"answer":"ok"}})), Err(CallError::InvalidResult(_))));
        assert_eq!(tool.response(&json!({"isError":true,"content":content,"structuredContent":"not a successful object"})), Err(CallError::ToolError("payload".into())));
        assert_eq!(tool.response(&json!({"isError":true,"content":content,"structuredContent":{"ignored":{"nested":["not examined"]}}})), Err(CallError::ToolError("payload".into())));
        assert!(matches!(
            tool.response(&json!({"isError":true,"content":[{},{}]})),
            Err(CallError::InvalidResult(_))
        ));
    }
}

#[test]
fn undeclared_structured_content_has_its_own_depth_limit() {
    let mut limits = Limits::default();
    limits.schema.instance_depth = 1;
    let tool = ProjectedTool::new(&definition("bounded"), "bounded", limits).unwrap();
    assert!(
        tool.response(&json!({"content":[],"structuredContent":{"x":0}}))
            .is_ok()
    );
    assert!(matches!(
        tool.response(&json!({"content":[],"structuredContent":{"x":{"y":0}}})),
        Err(CallError::InvalidResult(_))
    ));
}

#[test]
fn runtime_limit_changes_do_not_change_metadata_identity() {
    let upstream = definition("stable");
    let mut limits = Limits::default();
    let before = ProjectedTool::new(&upstream, "stable", limits).unwrap();
    limits.schema.instance_bytes *= 2;
    limits.content.max_blocks *= 2;
    let after = ProjectedTool::new(&upstream, "stable", limits).unwrap();
    assert_eq!(before.digest, after.digest);
    assert_eq!(before.definition, after.definition);
}

#[test]
fn digest_ignores_projection_irrelevant_upstream_extensions() {
    let upstream = definition("stable");
    let before = ProjectedTool::new(&upstream, "stable", Limits::default()).unwrap();

    let mut with_extension = upstream;
    with_extension["x-server-internal"] = json!({"requestId": "ephemeral"});
    let after = ProjectedTool::new(&with_extension, "stable", Limits::default()).unwrap();

    assert_eq!(before.definition, after.definition);
    assert_eq!(before.digest, after.digest);

    let mut changed_schema = with_extension;
    changed_schema["inputSchema"]["maxProperties"] = json!(0);
    let after = ProjectedTool::new(&changed_schema, "stable", Limits::default()).unwrap();
    assert_eq!(before.definition, after.definition);
    assert_ne!(before.digest, after.digest);
}

#[test]
fn root_object_unions_remain_typed_in_arguments_and_results() {
    let choice = json!({"type":"object","oneOf":[
        {"type":"object","properties":{"kind":{"const":"count"},"amount":{"type":"integer"}},"required":["kind","amount"],"additionalProperties":false},
        {"type":"object","properties":{"kind":{"const":"label"},"text":{"type":"string"}},"required":["kind","text"],"additionalProperties":false}
    ]});
    let tool = ProjectedTool::new(
        &json!({"name":"choose","inputSchema":choice,"outputSchema":choice}),
        "choose",
        Limits::default(),
    )
    .unwrap();
    let model = tool.definition.canonical_input_model(0).unwrap();
    for value in [
        json!({"kind":"count","amount":-17}),
        json!({"kind":"label","text":"value"}),
    ] {
        let input = from_untrusted_json_value(
            &model.record_schema,
            &model.record_schema.root,
            &json!({"arguments":value}),
        )
        .unwrap();
        assert_eq!(tool.arguments(&input).unwrap(), value);
        let output = tool
            .response(&json!({"content":[],"structuredContent":value}))
            .unwrap();
        let result = &tool.definition.commands.nodes[0]
            .body
            .as_ref()
            .unwrap()
            .result
            .as_ref()
            .unwrap()
            .type_;
        validate_value(&tool.definition.schema, result, &output.value).unwrap();
        assert_eq!(
            to_json_value(&tool.definition.schema, result, &output.value).unwrap()["structured"],
            value
        );
    }
    assert!(
        tool.arguments(&SchemaValue::Record { fields: vec![] })
            .is_err()
    );
}
