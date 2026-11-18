// Copyright 2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

use super::*;
use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCalls, CreateChatCompletionRequest, FinishReason,
};
use futures::StreamExt;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;
use test_r::{test, timeout};

fn client(mock: &ModelMock) -> Client<OpenAIConfig> {
    Client::with_config(
        OpenAIConfig::new()
            .with_api_base(format!("{}/v1", mock.base_url()))
            .with_api_key("mock-secret"),
    )
    .with_http_client(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap(),
    )
}

fn text_script(text: &str) -> Script {
    Script {
        steps: vec![Step {
            expect: Expectations::default(),
            response: ChatResponse::Text { text: text.into() },
        }],
    }
}

fn request() -> Value {
    json!({"model": "scripted-model", "messages": [{"role": "user", "content": "Buy 3 tickets"}]})
}

async fn post(mock: &ModelMock, request: &Value) -> reqwest::Response {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .post(format!("{}/v1/chat/completions", mock.base_url()))
        .bearer_auth("mock-secret")
        .json(request)
        .send()
        .await
        .unwrap()
}

fn assert_drop_fails(mock: ModelMock) {
    assert!(catch_unwind(AssertUnwindSafe(|| drop(mock))).is_err());
}

#[test]
#[timeout("30s")]
async fn independent_client_reads_tool_loop_and_file_script() {
    let rust = Script {
        steps: vec![
            Step {
                expect: Expectations {
                    model: Some("scripted-model".into()),
                    tool_names: Some(vec!["purchase".into(), "lookup".into()]),
                    last_message_role: Some("user".into()),
                    authorization: Some("Bearer mock-secret".into()),
                    ..Default::default()
                },
                response: ChatResponse::ToolCalls {
                    calls: vec![ToolCall {
                        id: "call-purchase".into(),
                        name: "purchase".into(),
                        arguments: r#"{"quantity":3}"#.into(),
                    }],
                },
            },
            Step {
                expect: Expectations {
                    model: Some("scripted-model".into()),
                    tool_names: Some(vec![]),
                    last_message_role: Some("tool".into()),
                    last_tool_call_id: Some("call-purchase".into()),
                    authorization: Some("Bearer mock-secret".into()),
                },
                response: ChatResponse::Text {
                    text: "Purchased 3 tickets.".into(),
                },
            },
        ],
    };
    let file = Script::from_json(include_str!("../examples/tool-loop.json")).unwrap();
    let mut outputs = Vec::new();
    for script in [rust, file] {
        let mock = ModelMock::start(script).await.unwrap();
        let client = client(&mock);
        let mut first = request();
        // Reverse the declared expectation's order: order must not matter.
        first["tools"] = json!([
            {"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}},
            {"type": "function", "function": {"name": "purchase", "parameters": {"type": "object", "properties": {"quantity": {"type": "integer"}}}}}
        ]);
        let first = client
            .chat()
            .create(serde_json::from_value(first).unwrap())
            .await
            .unwrap();
        assert_eq!(first.object, "chat.completion");
        assert_eq!(first.model, "scripted-model");
        assert_eq!(
            first.choices[0].finish_reason,
            Some(FinishReason::ToolCalls)
        );
        let calls = first.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        let ChatCompletionMessageToolCalls::Function(call) = &calls[0] else {
            panic!("expected function tool")
        };
        assert_eq!(call.id, "call-purchase");
        assert_eq!(call.function.name, "purchase");
        assert_eq!(call.function.arguments, r#"{"quantity":3}"#);
        let wire = serde_json::to_value(&calls[0]).unwrap();
        assert_eq!(wire["type"], "function");
        assert!(wire["function"]["arguments"].is_string());
        let second = json!({"model": "scripted-model", "messages": [
            {"role": "user", "content": "Buy 3 tickets"},
            {"role": "assistant", "tool_calls": calls},
            {"role": "tool", "tool_call_id": call.id, "content": "success"}
        ]});
        let second = client
            .chat()
            .create(serde_json::from_value(second).unwrap())
            .await
            .unwrap();
        assert_eq!(second.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(
            second.choices[0].message.content.as_deref(),
            Some("Purchased 3 tickets.")
        );
        outputs.push((
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(second).unwrap(),
        ));
        mock.verify().unwrap();
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
#[timeout("30s")]
async fn mismatches_are_http_errors_and_persist_until_drop() {
    let expectations = Expectations {
        tool_names: Some(vec!["purchase".into()]),
        last_message_role: Some("tool".into()),
        last_tool_call_id: Some("call-7".into()),
        model: Some("scripted-model".into()),
        authorization: Some("Bearer mock-secret".into()),
    };
    let good = json!({"model": "scripted-model", "messages": [{"role": "tool", "tool_call_id": "call-7", "content": "ok"}],
        "tools": [{"type": "function", "function": {"name": "purchase"}}]});
    for field in [
        "model",
        "last_message_role",
        "last_tool_call_id",
        "tool_names",
        "authorization",
        "stream",
    ] {
        let mut script = text_script("must not be returned");
        script.steps[0].expect = expectations.clone();
        let mock = ModelMock::start(script).await.unwrap();
        let mut bad = good.clone();
        match field {
            "model" => bad["model"] = json!("wrong-model"),
            "last_message_role" => bad["messages"][0]["role"] = json!("user"),
            "last_tool_call_id" => bad["messages"][0]["tool_call_id"] = Value::Null,
            "tool_names" => {
                bad["tools"] = json!([
                    {"type": "function", "function": {"name": "purchase"}},
                    {"type": "function", "function": {"name": "purchase"}}
                ])
            }
            "stream" => bad["stream"] = json!(true),
            _ => (),
        }
        let response = reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", mock.base_url()))
            .bearer_auth(if field == "authorization" {
                "golem-standin-private"
            } else {
                "mock-secret"
            })
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response.text().await.unwrap();
        assert!(body.contains(&format!("step 1: {field}:")), "{body}");
        assert!(!body.contains("mock-secret"));
        assert!(!body.contains("golem-standin-private"));
        let failure = mock.verify().unwrap_err().to_string();
        assert!(failure.contains(&format!("step 1: {field}:")), "{failure}");
        assert_drop_fails(mock);
    }
}

#[test]
#[timeout("30s")]
async fn extra_missing_and_malformed_calls_fail_verification() {
    let mock = ModelMock::start(text_script("one reply")).await.unwrap();
    assert!(post(&mock, &request()).await.status().is_success());
    let extra = post(&mock, &request()).await;
    assert_eq!(extra.status(), StatusCode::BAD_REQUEST);
    assert!(
        extra
            .text()
            .await
            .unwrap()
            .contains("step 2: script exhausted")
    );
    assert!(
        mock.verify()
            .unwrap_err()
            .to_string()
            .contains("step 2: script exhausted")
    );
    assert_drop_fails(mock);

    let mut script = text_script("first reply");
    script.steps.push(Step {
        expect: Expectations::default(),
        response: ChatResponse::Text {
            text: "not requested".into(),
        },
    });
    let mock = ModelMock::start(script).await.unwrap();
    assert!(post(&mock, &request()).await.status().is_success());
    assert!(
        mock.verify()
            .unwrap_err()
            .to_string()
            .contains("step 2: 1 unrequested")
    );
    assert_drop_fails(mock);

    for path in ["/v1/chat/completions", "/wrong-endpoint"] {
        let mock = ModelMock::start(Script::default()).await.unwrap();
        let response = reqwest::Client::new()
            .post(format!("{}{path}", mock.base_url()))
            .body("not json")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(mock.verify().is_err());
        assert_drop_fails(mock);
    }
}

fn streamed_text() -> Script {
    Script {
        steps: vec![Step {
            expect: Expectations::default(),
            response: ChatResponse::Stream {
                deltas: vec![
                    Delta {
                        content: Some("Hello".into()),
                        ..Default::default()
                    },
                    Delta {
                        content: Some(", ".into()),
                        ..Default::default()
                    },
                    Delta {
                        content: Some("世界!".into()),
                        ..Default::default()
                    },
                ],
            },
        }],
    }
}

fn streamed_tools() -> Script {
    Script::from_json(include_str!("../examples/streamed-tools.json")).unwrap()
}

#[test]
#[timeout("30s")]
async fn sse_matches_handwritten_openai_transcripts() {
    // Golden files are hand-written from OpenAI's ChatCompletionChunk reference,
    // not generated by the response encoder. Compare the complete SSE framing.
    let mut goldens = goldenfile::Mint::new("tests/goldenfiles");
    for (name, script) in [
        ("text.sse", streamed_text()),
        ("tools.sse", streamed_tools()),
    ] {
        let mock = ModelMock::start(script).await.unwrap();
        let mut request = request();
        request["stream"] = json!(true);
        let response = post(&mock, &request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/event-stream");
        let transcript = response.text().await.unwrap();
        write!(goldens.new_goldenfile(name).unwrap(), "{transcript}").unwrap();
        mock.verify().unwrap();
    }
}

#[test]
#[timeout("30s")]
async fn independent_client_reassembles_text_and_interleaved_tool_deltas() {
    let mock = ModelMock::start(streamed_text()).await.unwrap();
    let client = client(&mock);
    let mut request: CreateChatCompletionRequest = serde_json::from_value(request()).unwrap();
    request.stream = Some(true);
    let chunks = client
        .chat()
        .create_stream(request.clone())
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(chunks.len(), 5);
    let text = chunks
        .iter()
        .filter_map(|c| c.choices[0].delta.content.as_deref())
        .collect::<String>();
    assert_eq!(text, "Hello, 世界!");
    assert_eq!(
        chunks.last().unwrap().choices[0].finish_reason,
        Some(FinishReason::Stop)
    );
    mock.verify().unwrap();

    let mock = ModelMock::start(streamed_tools()).await.unwrap();
    let chunks = self::client(&mock)
        .chat()
        .create_stream(request)
        .await
        .unwrap()
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut calls = [
        (String::new(), String::new(), String::new()),
        (String::new(), String::new(), String::new()),
    ];
    let mut indices = Vec::new();
    for chunk in &chunks {
        if let Some(deltas) = &chunk.choices[0].delta.tool_calls {
            for delta in deltas {
                indices.push(delta.index);
                let (id, name, arguments) = &mut calls[delta.index as usize];
                if let Some(value) = &delta.id {
                    id.push_str(value);
                }
                if let Some(function) = &delta.function {
                    if let Some(value) = &function.name {
                        name.push_str(value);
                    }
                    if let Some(value) = &function.arguments {
                        arguments.push_str(value);
                    }
                }
            }
        }
    }
    assert_eq!(indices, [0, 1, 0, 1]);
    assert_eq!(
        calls,
        [
            (
                "call-purchase".into(),
                "purchase".into(),
                r#"{"quantity":3}"#.into()
            ),
            (
                "call-weather".into(),
                "weather".into(),
                r#"{"city":"Budapest"}"#.into()
            ),
        ]
    );
    assert_eq!(
        chunks.last().unwrap().choices[0].finish_reason,
        Some(FinishReason::ToolCalls)
    );
    mock.verify().unwrap();
}

#[test]
#[timeout("30s")]
async fn concurrent_fixtures_have_independent_ports_and_scripts() {
    let (left, right) = tokio::join!(
        ModelMock::start(text_script("left")),
        ModelMock::start(text_script("right"))
    );
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_ne!(left.base_url(), right.base_url());
    let request = request();
    let (a, b) = tokio::join!(post(&left, &request), post(&right, &request));
    assert_eq!(
        a.json::<Value>().await.unwrap()["choices"][0]["message"]["content"],
        "left"
    );
    assert_eq!(
        b.json::<Value>().await.unwrap()["choices"][0]["message"]["content"],
        "right"
    );
    left.verify().unwrap();
    right.verify().unwrap();
}

#[test]
fn script_typos_fail_without_echoing_credentials() {
    for json in [
        r#"{"steps":[],"typo":"private"}"#,
        r#"{"steps":[{"expect":{"authorization":["private"]},"response":{"type":"text","text":"ok"}}]}"#,
    ] {
        let error = Script::from_json(json).err().unwrap().to_string();
        assert!(error.contains("invalid model script"));
        assert!(!error.contains("private"));
    }
}
