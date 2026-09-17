use super::{ProjectedResponse, decode_response, encode_response};
use golem_common::model::oplog::payload::types::{SerializableToolError, SerializableToolRpcError};
use golem_common::schema::{SchemaValue, VariantValuePayload};
use golem_mcp_import::tool::{Limits, ProjectedTool};
use golem_mcp_import::transport::TransportError;
use serde_json::{Value, json};
use test_r::test;

fn project_response(tool: ProjectedTool, bytes: &[u8]) -> anyhow::Result<ProjectedResponse> {
    super::project_response(tool, decode_response(bytes)?, None)
}

fn tool(output_schema: Option<Value>) -> ProjectedTool {
    let mut definition = json!({
        "name": "probe",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }
    });
    if let Some(output_schema) = output_schema {
        definition["outputSchema"] = output_schema;
    }
    ProjectedTool::new(&definition, "probe", Limits::default()).unwrap()
}

#[test]
async fn project_response_preserves_structured_result_and_binary_stdout() {
    let response = Ok(json!({
        "content": [{"type":"image", "data":"AAEC", "mimeType":"image/png"}],
        "structuredContent": {"answer": 42}
    }));
    let bytes = encode_response(response).await.unwrap();
    let (result, stdout) = project_response(
        tool(Some(json!({
            "type":"object",
            "properties":{"answer":{"type":"integer"}},
            "required":["answer"],
            "additionalProperties":false
        }))),
        &bytes,
    )
    .unwrap()
    .unwrap();

    assert_eq!(
        result.result.unwrap().into_typed().unwrap().value(),
        &SchemaValue::Record {
            fields: vec![
                SchemaValue::Record {
                    fields: vec![SchemaValue::S64(42)]
                },
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(SchemaValue::Record {
                        fields: vec![
                            SchemaValue::String("image/png".into()),
                            SchemaValue::Option { inner: None },
                            SchemaValue::Option { inner: None },
                        ]
                    })),
                }),
            ],
        }
    );
    assert_eq!(stdout, Some(vec![0, 1, 2]));
}

#[test]
async fn project_response_keeps_empty_content_and_maps_protocol_errors() {
    let bytes = encode_response(Ok(json!({"content":[]}))).await.unwrap();
    let (result, stdout) = project_response(tool(None), &bytes).unwrap().unwrap();
    assert_eq!(
        result.result.unwrap().into_typed().unwrap().value(),
        &SchemaValue::Record {
            fields: vec![
                SchemaValue::Option { inner: None },
                SchemaValue::Variant(VariantValuePayload {
                    case: 0,
                    payload: None
                }),
            ],
        }
    );
    assert_eq!(stdout, None);

    let invalid = encode_response(Err(TransportError::Remote {
        code: -32602,
        message: "bad arguments".into(),
        data: None,
    }))
    .await
    .unwrap();
    assert!(matches!(
        project_response(tool(None), &invalid).unwrap(),
        Err(SerializableToolRpcError::RemoteToolError(error))
            if matches!(*error, SerializableToolError::InvalidInput(ref details) if details == "bad arguments")
    ));
    assert!(matches!(
        super::project_response(tool(None), decode_response(&invalid).unwrap(), Some(false)).unwrap(),
        Err(SerializableToolRpcError::RemoteToolError(error))
            if matches!(*error, SerializableToolError::InvalidToolName(ref name) if name == "probe")
    ));
    assert!(matches!(
        super::project_response(tool(None), decode_response(&invalid).unwrap(), Some(true)).unwrap(),
        Err(SerializableToolRpcError::RemoteToolError(error))
            if matches!(*error, SerializableToolError::InvalidInput(_))
    ));

    let denied = encode_response(Err(TransportError::Denied)).await.unwrap();
    assert!(matches!(
        project_response(tool(None), &denied).unwrap(),
        Err(SerializableToolRpcError::Denied(_))
    ));
}

#[test]
async fn recorded_protocol_error_preserves_code_message_and_data() {
    let bytes = encode_response(Err(TransportError::Remote {
        code: -32007,
        message: "upstream operation failed".into(),
        data: Some(json!({"retryable":false,"reason":{"id":17}})),
    }))
    .await
    .unwrap();
    let Err(SerializableToolRpcError::RemoteToolError(error)) =
        project_response(tool(None), &bytes).unwrap()
    else {
        panic!("expected tool error");
    };
    let SerializableToolError::CustomError(value) = *error else {
        panic!("expected custom error");
    };
    let SchemaValue::String(details) = value.value() else {
        panic!("expected string payload");
    };
    assert_eq!(
        serde_json::from_str::<Value>(details).unwrap(),
        json!({
            "code": -32007,
            "message": "upstream operation failed",
            "data": {"retryable":false,"reason":{"id":17}}
        })
    );
}

#[test]
async fn recorded_tool_error_bypasses_declared_success_schema() {
    let schema = json!({"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false});
    let bytes = encode_response(Ok(
        json!({"isError":true,"content":[{"type":"text","text":"permission rejected"}]}),
    ))
    .await
    .unwrap();
    let Err(SerializableToolRpcError::RemoteToolError(error)) =
        project_response(tool(Some(schema.clone())), &bytes).unwrap()
    else {
        panic!("expected tool error");
    };
    assert!(
        matches!(*error, SerializableToolError::CustomError(ref value) if value.value() == &SchemaValue::String("permission rejected".into()))
    );
    let bytes = encode_response(Ok(json!({"content":[]}))).await.unwrap();
    assert!(
        matches!(project_response(tool(Some(schema)), &bytes).unwrap(), Err(SerializableToolRpcError::RemoteToolError(error)) if matches!(*error, SerializableToolError::InvalidResult(_)))
    );
}
