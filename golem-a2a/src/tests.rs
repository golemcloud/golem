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

use crate::{http, jsonrpc, stream, wire};
use a2a::*;
use futures::{StreamExt, stream as futures_stream};
use poem::IntoResponse;
use serde_json::{Value, json};
use test_r::test;

fn roundtrip<T: wire::WirePayload>(value: Value) -> Value {
    wire::to_value(&wire::from_value::<T>(value).unwrap()).unwrap()
}

#[test]
fn specification_examples() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("../tests/fixtures/spec-v1.0.1-examples.json")).unwrap();
    for fixture in fixtures {
        let value = fixture["value"].clone();
        let actual = match fixture["kind"].as_str().unwrap() {
            "Artifact" => roundtrip::<Artifact>(value.clone()),
            "Message" => roundtrip::<Message>(value.clone()),
            "Part" => roundtrip::<Part>(value.clone()),
            "SendMessageRequest" => roundtrip::<SendMessageRequest>(value.clone()),
            "SendMessageResponse" => roundtrip::<SendMessageResponse>(value.clone()),
            "ListTasksResponse" => roundtrip::<ListTasksResponse>(value.clone()),
            "StreamResponse" => roundtrip::<StreamResponse>(value.clone()),
            "JsonRpcRequest" => {
                let request = jsonrpc::parse_request(&serde_json::to_vec(&value).unwrap()).unwrap();
                if let Some(params) = &request.params {
                    let encoded = match request.method.as_str() {
                        "GetTask" => roundtrip::<GetTaskRequest>(params.clone()),
                        "ListTasks" => roundtrip::<ListTasksRequest>(params.clone()),
                        "CancelTask" => roundtrip::<CancelTaskRequest>(params.clone()),
                        "SubscribeToTask" => roundtrip::<SubscribeToTaskRequest>(params.clone()),
                        other => panic!("unhandled method: {other}"),
                    };
                    assert_eq!(&encoded, params);
                }
                serde_json::to_value(request).unwrap()
            }
            "JsonRpcResponse" => {
                let response: jsonrpc::JsonRpcResponse =
                    serde_json::from_value(value.clone()).unwrap();
                let remote = jsonrpc::decode_error(response.error.clone().unwrap()).unwrap();
                assert_eq!(remote.code, value["error"]["code"].as_i64().unwrap() as i32);
                serde_json::to_value(response).unwrap()
            }
            "HttpError" => http::encode_error(&http::decode_error(value.clone()).unwrap()).unwrap(),
            "StreamSequence" => {
                let values = value.as_array().unwrap();
                assert_eq!(roundtrip::<StreamResponse>(values[0].clone()), values[0]);
                assert_eq!(
                    roundtrip::<StreamResponse>(values[1].clone()),
                    json!({"artifactUpdate": {
                        "taskId":"task-uuid", "contextId":"", "artifact":{"artifactId":"", "parts":[{"text":"# Climate Change Report\n\n"}]}
                    }})
                );
                assert_eq!(
                    roundtrip::<StreamResponse>(values[2].clone()),
                    json!({"statusUpdate": {
                        "taskId":"task-uuid", "contextId":"", "status":{"state":"TASK_STATE_COMPLETED"}
                    }})
                );
                continue;
            }
            "Illustration" => {
                assert!(!fixture["reason"].as_str().unwrap().is_empty());
                continue;
            }
            other => panic!("unhandled fixture: {other}"),
        };
        let expected = fixture.get("expected").unwrap_or(&value);
        assert_eq!(
            &actual, expected,
            "§{} line {}",
            fixture["section"], fixture["line"]
        );
    }
}

#[test]
fn required_defaults_in_every_emitted_type() {
    let task: Task = wire::from_value(json!({"status":{}})).unwrap();
    let task_json = json!({"id":"", "status":{"state":"TASK_STATE_UNSPECIFIED"}});
    assert_eq!(wire::to_value(&task).unwrap(), task_json);
    let message: Message = wire::from_value(json!({})).unwrap();
    let message_json = json!({"messageId":"", "role":"ROLE_UNSPECIFIED", "parts":[]});
    assert_eq!(wire::to_value(&message).unwrap(), message_json);
    let artifact: Artifact = wire::from_value(json!({})).unwrap();
    let artifact_json = json!({"artifactId":"", "parts":[]});
    assert_eq!(wire::to_value(&artifact).unwrap(), artifact_json);
    let status: TaskStatusUpdateEvent = wire::from_value(json!({"status":{}})).unwrap();
    let status_json =
        json!({"taskId":"", "contextId":"", "status":{"state":"TASK_STATE_UNSPECIFIED"}});
    assert_eq!(wire::to_value(&status).unwrap(), status_json);
    let artifact_update: TaskArtifactUpdateEvent =
        wire::from_value(json!({"artifact":{}})).unwrap();
    let artifact_update_json = json!({"taskId":"", "contextId":"", "artifact":artifact_json});
    assert_eq!(
        wire::to_value(&artifact_update).unwrap(),
        artifact_update_json
    );
    for (event, expected) in [
        (
            StreamResponse::Task(task.clone()),
            json!({"task":task_json}),
        ),
        (
            StreamResponse::Message(message.clone()),
            json!({"message":message_json}),
        ),
        (
            StreamResponse::StatusUpdate(status),
            json!({"statusUpdate":status_json}),
        ),
        (
            StreamResponse::ArtifactUpdate(artifact_update),
            json!({"artifactUpdate":artifact_update_json}),
        ),
    ] {
        assert_eq!(wire::to_value(&event).unwrap(), expected);
    }
    assert_eq!(
        wire::to_value(&SendMessageResponse::Task(task)).unwrap(),
        json!({"task":task_json})
    );
    assert_eq!(
        wire::to_value(&SendMessageResponse::Message(message)).unwrap(),
        json!({"message":message_json})
    );
    assert_eq!(
        roundtrip::<ListTasksResponse>(json!({})),
        json!({"tasks":[],"nextPageToken":"","pageSize":0,"totalSize":0})
    );
    assert_eq!(
        roundtrip::<AgentCard>(json!({})),
        json!({"name":"","description":"","version":"","supportedInterfaces":[],"capabilities":{},"defaultInputModes":[],"defaultOutputModes":[],"skills":[]})
    );
}

#[test]
fn required_defaults_in_requests_and_push_configurations() {
    for value in [
        roundtrip::<GetTaskRequest>(json!({})),
        roundtrip::<CancelTaskRequest>(json!({})),
        roundtrip::<SubscribeToTaskRequest>(json!({})),
    ] {
        assert_eq!(value, json!({"id":""}));
    }
    for value in [
        roundtrip::<GetTaskPushNotificationConfigRequest>(json!({})),
        roundtrip::<DeleteTaskPushNotificationConfigRequest>(json!({})),
    ] {
        assert_eq!(value, json!({"taskId":"","id":""}));
    }
    assert_eq!(
        roundtrip::<ListTaskPushNotificationConfigsRequest>(json!({})),
        json!({"taskId":""})
    );
    assert_eq!(
        roundtrip::<TaskPushNotificationConfig>(json!({})),
        json!({"url":""})
    );
    assert_eq!(
        roundtrip::<ListTaskPushNotificationConfigsResponse>(json!({"configs":[{}]})),
        json!({"configs":[{"url":""}]})
    );
    let request = roundtrip::<SendMessageRequest>(
        json!({"message":{},"configuration":{"taskPushNotificationConfig":{"authentication":{}}}}),
    );
    assert_eq!(
        request["configuration"],
        json!({"taskPushNotificationConfig":{"url":"","authentication":{"scheme":""}}})
    );
}

#[test]
fn nested_defaults_do_not_rewrite_user_content() {
    let content = json!({"status":{}, "task":{}, "timestamp":"2026-01-02T10:00:00+03:00"});
    let task = json!({"id":"t", "status":{"state":"TASK_STATE_WORKING", "message":{
        "parts":[{"data":content}], "metadata":content
    }}, "history":[{}], "artifacts":[{}], "metadata":content});
    let encoded = roundtrip::<Task>(task);
    assert_eq!(encoded["status"]["message"]["role"], "ROLE_UNSPECIFIED");
    assert_eq!(
        encoded["history"][0],
        json!({"messageId":"","role":"ROLE_UNSPECIFIED","parts":[]})
    );
    assert_eq!(encoded["artifacts"][0], json!({"artifactId":"","parts":[]}));
    assert_eq!(encoded["metadata"], content);
    assert_eq!(encoded["status"]["message"]["metadata"], content);
    assert_eq!(encoded["status"]["message"]["parts"][0]["data"], content);
    let card =
        roundtrip::<AgentCard>(json!({"supportedInterfaces":[{}],"skills":[{}],"provider":{}}));
    assert_eq!(
        card["supportedInterfaces"][0],
        json!({"url":"","protocolBinding":"","protocolVersion":""})
    );
    assert_eq!(
        card["skills"][0],
        json!({"id":"","name":"","description":"","tags":[]})
    );
    assert_eq!(card["provider"], json!({"url":"","organization":""}));
    for (scheme, expected) in [
        (
            json!({"apiKeySecurityScheme":{}}),
            json!({"apiKeySecurityScheme":{"location":"","name":""}}),
        ),
        (
            json!({"httpAuthSecurityScheme":{}}),
            json!({"httpAuthSecurityScheme":{"scheme":""}}),
        ),
        (
            json!({"openIdConnectSecurityScheme":{}}),
            json!({"openIdConnectSecurityScheme":{"openIdConnectUrl":""}}),
        ),
        (
            json!({"oauth2SecurityScheme":{"flows":{"authorizationCode":{}}}}),
            json!({"oauth2SecurityScheme":{"flows":{"authorizationCode":{"authorizationUrl":"","tokenUrl":"","scopes":{}}}}}),
        ),
        (
            json!({"oauth2SecurityScheme":{"flows":{"clientCredentials":{}}}}),
            json!({"oauth2SecurityScheme":{"flows":{"clientCredentials":{"tokenUrl":"","scopes":{}}}}}),
        ),
        (
            json!({"oauth2SecurityScheme":{"flows":{"deviceCode":{}}}}),
            json!({"oauth2SecurityScheme":{"flows":{"deviceCode":{"deviceAuthorizationUrl":"","tokenUrl":"","scopes":{}}}}}),
        ),
    ] {
        let card =
            roundtrip::<AgentCard>(json!({"securitySchemes":{"auth":scheme},"signatures":[{}]}));
        assert_eq!(card["securitySchemes"]["auth"], expected);
        assert_eq!(card["signatures"], json!([{"protected":"","signature":""}]));
    }
}

#[test]
fn sample_agent_card_uses_normative_security_requirements() {
    let fixtures: Vec<Value> =
        serde_json::from_str(include_str!("../tests/fixtures/spec-v1.0.1-examples.json")).unwrap();
    let fixture = fixtures.iter().find(|f| f["line"] == 2138).unwrap();
    let mut card: Value = serde_json::from_str(fixture["source"].as_str().unwrap()).unwrap();
    let decoded: AgentCard = wire::from_value(card.clone()).unwrap();
    assert!(decoded.security_requirements.is_none());
    assert!(wire::to_value(&decoded).unwrap().get("security").is_none());
    // §8.5's sample uses an obsolete field. Use the §4.5 schema explicitly,
    // without interpreting the obsolete field as security requirements.
    card.as_object_mut().unwrap().remove("security");
    card["securityRequirements"] =
        json!([{"schemes":{"google":{"list":["openid","profile","email"]}}}]);
    assert_eq!(roundtrip::<AgentCard>(card.clone()), card);
}

#[test]
fn timestamps_are_utc_and_null_lists_are_empty() {
    let mut task: Task = wire::from_value(
        json!({"status":{"timestamp":"2026-01-02T10:00:00+03:00"},"artifacts":null,"history":null}),
    )
    .unwrap();
    // Construct a timestamp from an offset independently of the wire decoder.
    task.status.timestamp = Some("2026-01-02T10:00:00+03:00".parse().unwrap());
    assert_eq!(
        wire::to_value(&task).unwrap()["status"]["timestamp"],
        "2026-01-02T07:00:00Z"
    );
    assert!(task.artifacts.is_none());
    assert!(task.history.is_none());
    let request =
        roundtrip::<ListTasksRequest>(json!({"statusTimestampAfter":"2026-01-02T10:00:00+03:00"}));
    assert_eq!(request["statusTimestampAfter"], "2026-01-02T07:00:00Z");
    let card: AgentCard = wire::from_value(json!({"supportedInterfaces":null,"defaultInputModes":null,"defaultOutputModes":null,"skills":null,"securityRequirements":null,"signatures":null,"capabilities":{"extensions":null}})).unwrap();
    let encoded = wire::to_value(&card).unwrap();
    for key in [
        "supportedInterfaces",
        "defaultInputModes",
        "defaultOutputModes",
        "skills",
    ] {
        assert_eq!(encoded[key], json!([]));
    }
    let message =
        roundtrip::<Message>(json!({"parts":null,"extensions":null,"referenceTaskIds":null}));
    assert_eq!(
        message,
        json!({"messageId":"","role":"ROLE_UNSPECIFIED","parts":[]})
    );
    assert_eq!(
        roundtrip::<Artifact>(json!({"parts":null,"extensions":null})),
        json!({"artifactId":"","parts":[]})
    );
    let card = roundtrip::<AgentCard>(
        json!({"skills":[{"tags":null,"examples":null,"inputModes":null,"outputModes":null,"securityRequirements":null}],"securityRequirements":[{"schemes":{"auth":{"list":null}}}]}),
    );
    assert_eq!(
        card["skills"][0],
        json!({"id":"","name":"","description":"","tags":[]})
    );
    assert_eq!(
        card["securityRequirements"][0],
        json!({"schemes":{"auth":{}}})
    );
    assert_eq!(
        roundtrip::<ListTasksResponse>(json!({"tasks":null}))["tasks"],
        json!([])
    );
    assert_eq!(
        roundtrip::<ListTaskPushNotificationConfigsResponse>(json!({"configs":null})),
        json!({})
    );
    let request = roundtrip::<SendMessageRequest>(
        json!({"message":{},"configuration":{"acceptedOutputModes":null}}),
    );
    assert_eq!(request["configuration"], json!({}));
    assert!(wire::from_value::<Message>(json!({"parts":[{}]})).is_err());
    assert!(wire::from_value::<SendMessageRequest>(json!({})).is_err());
}

#[test]
fn binding_error_mappings_follow_specification_5_4() {
    // Literal expected values from §5.4, not upstream's lookup functions.
    for (code, status, grpc, reason) in [
        (-32001, 404, "NOT_FOUND", "TASK_NOT_FOUND"),
        (-32002, 400, "FAILED_PRECONDITION", "TASK_NOT_CANCELABLE"),
        (
            -32003,
            400,
            "FAILED_PRECONDITION",
            "PUSH_NOTIFICATION_NOT_SUPPORTED",
        ),
        (-32004, 400, "FAILED_PRECONDITION", "UNSUPPORTED_OPERATION"),
        (
            -32005,
            400,
            "INVALID_ARGUMENT",
            "CONTENT_TYPE_NOT_SUPPORTED",
        ),
        (-32006, 500, "INTERNAL", "INVALID_AGENT_RESPONSE"),
        (
            -32007,
            400,
            "FAILED_PRECONDITION",
            "EXTENDED_AGENT_CARD_NOT_CONFIGURED",
        ),
        (
            -32008,
            400,
            "FAILED_PRECONDITION",
            "EXTENSION_SUPPORT_REQUIRED",
        ),
        (-32009, 400, "FAILED_PRECONDITION", "VERSION_NOT_SUPPORTED"),
    ] {
        let error = A2AError::new(code, "distinct message");
        let rpc = error.to_jsonrpc_error();
        assert_eq!(rpc.code, code);
        assert_eq!(rpc.data.unwrap()[0]["reason"], reason);
        let body = http::encode_error(&error).unwrap();
        assert_eq!(
            body,
            json!({"error":{"code":status,"status":grpc,"message":"distinct message","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":reason,"domain":"a2a-protocol.org"}]}})
        );
        let decoded = http::decode_error(body.clone()).unwrap();
        assert_eq!(decoded.code, code);
        assert_eq!(decoded.message, error.message);
        assert_eq!(http::encode_error(&decoded).unwrap(), body);
        let mut without_status = body;
        without_status["error"]
            .as_object_mut()
            .unwrap()
            .remove("status");
        let decoded = http::decode_error(without_status).unwrap();
        assert_eq!(decoded.code, code);
        assert_eq!(decoded.message, error.message);
    }
}

#[test]
fn error_details_and_http_fallbacks() {
    let details = json!([
        {"@type":"type.googleapis.com/google.rpc.BadRequest","fieldViolations":[{"field":"message","description":"required"}]},
        {"@type":"example.org/CustomDetail","context":{"items":[null,42,"x"]}}
    ]);
    let rpc_error = jsonrpc::decode_error(jsonrpc::JsonRpcError {
        code: -32602,
        message: "bad input".into(),
        data: Some(details.clone()),
    })
    .unwrap();
    let http_error = http::decode_error(json!({"error":{
        "code":400,"message":"bad input","details":details
    }}))
    .unwrap();
    for error in [rpc_error, http_error] {
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "bad input");
        assert_eq!(
            serde_json::to_value(error.details.unwrap()).unwrap(),
            details
        );
    }
    for invalid in [
        json!({"taskId":"t1"}),
        json!("explanation"),
        json!([true]),
        json!([{}]),
        json!([{"@type":42}]),
        json!([{"@type":""}]),
        json!([details[0], {}]),
    ] {
        assert!(
            jsonrpc::decode_error(jsonrpc::JsonRpcError {
                code: -32603,
                message: "failure".into(),
                data: Some(invalid.clone()),
            })
            .is_err()
        );
        assert!(
            http::decode_error(json!({"error":{
                "code":500,"message":"failure","details":invalid
            }}))
            .is_err()
        );
    }
    let no_details = jsonrpc::decode_error(jsonrpc::JsonRpcError {
        code: -32603,
        message: "failure".into(),
        data: None,
    })
    .unwrap();
    assert_eq!(no_details.code, -32603);
    assert!(no_details.details.is_none());
    for (status, code) in [(400, -32602), (404, -32001), (500, -32603), (501, -32601)] {
        let error =
            http::decode_error(json!({"error":{"code":status,"message":"failure"}})).unwrap();
        assert_eq!(error.code, code);
        assert_eq!(error.message, "failure");
        assert!(error.details.is_none());
    }
    for incomplete in [
        json!({"error":{"message":"failure"}}),
        json!({"error":{"code":500}}),
    ] {
        assert!(http::decode_error(incomplete).is_err());
    }
}

#[test]
fn routes_equal_specification_5_3() {
    let expected = [
        ("POST", "/message:send", "SendMessage"),
        ("POST", "/message:stream", "SendStreamingMessage"),
        ("GET", "/tasks/{id}", "GetTask"),
        ("GET", "/tasks", "ListTasks"),
        ("POST", "/tasks/{id}:cancel", "CancelTask"),
        ("POST", "/tasks/{id}:subscribe", "SubscribeToTask"),
        (
            "POST",
            "/tasks/{id}/pushNotificationConfigs",
            "CreateTaskPushNotificationConfig",
        ),
        (
            "GET",
            "/tasks/{id}/pushNotificationConfigs/{configId}",
            "GetTaskPushNotificationConfig",
        ),
        (
            "GET",
            "/tasks/{id}/pushNotificationConfigs",
            "ListTaskPushNotificationConfigs",
        ),
        (
            "DELETE",
            "/tasks/{id}/pushNotificationConfigs/{configId}",
            "DeleteTaskPushNotificationConfig",
        ),
        ("GET", "/extendedAgentCard", "GetExtendedAgentCard"),
    ];
    assert_eq!(
        http::ROUTES
            .iter()
            .map(|r| (r.verb, r.path, r.method))
            .collect::<Vec<_>>(),
        expected
    );
    let id = "my.agent-id.019b267a-1375-7000-8000-123456789abc";
    for (verb, template, method) in expected {
        let path = template
            .replace("{id}", id)
            .replace("{configId}", "config-2");
        let matched = http::match_route(verb, &path).unwrap();
        assert_eq!(matched.route.method, method);
        if template.contains("{id}") {
            assert_eq!(matched.task_id, Some(id));
        }
        if template.contains("{configId}") {
            assert_eq!(matched.config_id, Some("config-2"));
        }
    }
    for path in [
        "/message/send",
        "/tasks/t/cancel",
        "/tasks/t/push-configs",
        "/tenant/tasks/t:cancel",
        "/tasks/:cancel",
        "/tasks/t:cancel/extra",
    ] {
        assert!(http::match_route("POST", path).is_none(), "{path}");
    }
    assert_eq!(
        http::match_route("POST", "/tasks/t%3Acancel:cancel")
            .unwrap()
            .task_id,
        Some("t%3Acancel")
    );
    for action in ["cancel", "subscribe"] {
        let path = format!("/tasks/{id}:{action}");
        for verb in ["GET", "DELETE", "PUT", "HEAD"] {
            assert!(http::match_route(verb, &path).is_none(), "{verb} {path}");
        }
        for escaped in ["%3A", "%3a"] {
            let escaped_id = format!("{id}{escaped}{action}");
            assert_eq!(
                http::match_route("GET", &format!("/tasks/{escaped_id}"))
                    .unwrap()
                    .task_id,
                Some(escaped_id.as_str())
            );
        }
    }
}

#[test]
fn jsonrpc_ids_parse_errors_and_stream_failures() {
    let event: StreamResponse = wire::from_value(
        json!({"message":{"messageId":"m","role":"ROLE_AGENT","parts":[{"text":"hello"}]}}),
    )
    .unwrap();
    for id in [json!(47), json!("47")] {
        let request = jsonrpc::parse_request(
            &serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":"SendStreamingMessage"}))
                .unwrap(),
        )
        .unwrap();
        let response =
            serde_json::to_value(stream::encode_jsonrpc(request.id.clone(), &event).unwrap())
                .unwrap();
        assert_eq!(response["id"], id);
        assert_eq!(
            stream::decode_jsonrpc(response, &request.id).unwrap(),
            event
        );
    }
    let malformed = jsonrpc::parse_request(br#"{"id":9,"method":"GetTask", "#).unwrap_err();
    assert_eq!(malformed.id, jsonrpc::JsonRpcId::Null);
    assert_eq!(malformed.error.unwrap().code, -32700);
    for id in [json!(5), json!("five")] {
        for (field, invalid) in [
            ("method", json!(7)),
            ("jsonrpc", json!(7)),
            ("jsonrpc", json!("1.0")),
            ("params", json!(42)),
        ] {
            let mut body = json!({"jsonrpc":"2.0","id":id,"method":"GetTask"});
            body[field] = invalid;
            let response = jsonrpc::parse_request(&serde_json::to_vec(&body).unwrap()).unwrap_err();
            assert_eq!(serde_json::to_value(response.id).unwrap(), id);
            assert_eq!(response.error.unwrap().code, -32600);
        }
    }
    for id in [json!([]), json!({}), json!(false)] {
        let body = json!({"jsonrpc":"2.0","id":id,"method":"GetTask"});
        let response = jsonrpc::parse_request(&serde_json::to_vec(&body).unwrap()).unwrap_err();
        assert_eq!(response.id, jsonrpc::JsonRpcId::Null);
        assert_eq!(response.error.unwrap().code, -32600);
    }
    for code in [-32700, -32600] {
        let response = json!({"jsonrpc":"2.0","id":null,"error":{"code":code,"message":"failure"}});
        match stream::decode_jsonrpc(response, &47.into()) {
            Err(stream::DecodeError::Remote(error)) => assert_eq!(error.code, code),
            other => panic!("expected remote request error, got {other:?}"),
        }
    }
    for (id, code) in [
        (json!(null), -32001),
        (json!(48), -32700),
        (json!("47"), -32600),
    ] {
        let response = json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":"failure"}});
        assert!(matches!(
            stream::decode_jsonrpc(response, &47.into()),
            Err(stream::DecodeError::Envelope(_))
        ));
    }
    assert!(matches!(
        stream::decode_jsonrpc(
            json!({"jsonrpc":"2.0","id":null,"result":stream::encode_http(&event).unwrap()}),
            &47.into()
        ),
        Err(stream::DecodeError::Envelope(_))
    ));
    for params in [json!({"id":"t"}), json!(["t"])] {
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":7,"method":"GetTask","params":params}))
                .unwrap();
        assert!(jsonrpc::parse_request(&body).is_ok());
    }
    for params in [json!(42), json!(true), json!("not structured"), json!(null)] {
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":7,"method":"GetTask","params":params}))
                .unwrap();
        assert_eq!(
            jsonrpc::parse_request(&body)
                .unwrap_err()
                .error
                .unwrap()
                .code,
            -32600
        );
    }
    let mut response =
        json!({"jsonrpc":"2.0","id":47,"result":stream::encode_http(&event).unwrap()});
    response["error"] = Value::Null;
    assert!(stream::decode_jsonrpc(response, &47.into()).is_err());
    assert_eq!(
        jsonrpc::parse_request(br#"{"jsonrpc":"1.0","id":"a","method":"GetTask"}"#)
            .unwrap_err()
            .error
            .unwrap()
            .code,
        -32600
    );
    let remote = jsonrpc::JsonRpcResponse::error(
        47.into(),
        A2AError::task_not_found("t").to_jsonrpc_error(),
    );
    match stream::decode_jsonrpc(serde_json::to_value(remote).unwrap(), &47.into()) {
        Err(stream::DecodeError::Remote(error)) => assert_eq!(error.code, -32001),
        other => panic!("expected remote error, got {other:?}"),
    }
    assert!(
        stream::decode_http(
            serde_json::to_value(stream::encode_jsonrpc(47.into(), &event).unwrap()).unwrap()
        )
        .is_err()
    );
    assert!(stream::decode_jsonrpc(stream::encode_http(&event).unwrap(), &47.into()).is_err());
    assert!(
        stream::decode_jsonrpc(
            json!({"jsonrpc":"2.0","id":48,"result":stream::encode_http(&event).unwrap()}),
            &47.into()
        )
        .is_err()
    );
}

#[test]
async fn poem_sse_to_client_parser_preserves_both_bindings() {
    let event: StreamResponse = wire::from_value(json!({"message":{"messageId":"m","role":"ROLE_AGENT","parts":[{"text":"first\ndata: not a new event\n\nλ"}]}})).unwrap();
    let events = [event,
        wire::from_value(json!({"task":{"id":"t.123-45","contextId":"c","status":{"state":"TASK_STATE_WORKING"}}})).unwrap(),
        wire::from_value(json!({"statusUpdate":{"taskId":"t.123-45","contextId":"c","status":{"state":"TASK_STATE_COMPLETED","timestamp":"2026-01-02T07:00:00Z"}}})).unwrap(),
        wire::from_value(json!({"artifactUpdate":{"taskId":"t.123-45","contextId":"c","append":true,"lastChunk":true,"artifact":{"artifactId":"a","parts":[{"data":{"count":42,"fraction":1.25}},{"data":null},{"raw":"AAEC/w==","filename":"x.bin"}]}}})).unwrap(),
    ];
    for rpc in [false, true] {
        for event in &events {
            let payload = if rpc {
                serde_json::to_value(stream::encode_jsonrpc(9.into(), event).unwrap()).unwrap()
            } else {
                stream::encode_http(event).unwrap()
            };
            let response = stream::sse_response(futures_stream::iter(vec![
                stream::sse_event(&payload).unwrap(),
            ]))
            .into_response();
            assert_eq!(response.headers()["content-type"], "text/event-stream");
            let bytes = response.into_body().into_bytes().await.unwrap();
            // Split inside UTF-8, JSON escape sequences, and SSE delimiters.
            let chunks = bytes
                .chunks(1)
                .map(|b| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(b)))
                .collect::<Vec<_>>();
            let mut parsed = Box::pin(stream::parse_sse(futures_stream::iter(chunks)));
            let value = stream::sse_data(parsed.next().await.unwrap().unwrap())
                .unwrap()
                .unwrap();
            let decoded = if rpc {
                stream::decode_jsonrpc(value, &9.into()).unwrap()
            } else {
                stream::decode_http(value).unwrap()
            };
            assert_eq!(&decoded, event);
            assert!(parsed.next().await.is_none());
        }
    }
    let named = sse_stream::Sse {
        event: Some("error".into()),
        data: Some(
            http::encode_error(&A2AError::task_not_found("t"))
                .unwrap()
                .to_string(),
        ),
        ..Default::default()
    };
    assert!(matches!(
        stream::sse_data(named),
        Err(stream::DecodeError::Envelope(_))
    ));
}
