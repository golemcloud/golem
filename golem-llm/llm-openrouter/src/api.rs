use std::io::BufReader;

use crate::exports::golem::llm::llm;
use crate::types;
use reqwest::{StatusCode, header::HeaderMap};

thread_local! {
    #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
    static CLIENT: reqwest::Client = reqwest::Client::new();

    #[cfg(not(target_arch = "wasm32"))]
    static CLIENT: reqwest::blocking::Client = reqwest::blocking::Client::new();
}

const BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouter {
    api_key: String,
    base_url: String,
}

impl OpenRouter {
    pub fn new(api_key: String) -> Self {
        Self::new_with_base_url(api_key, BASE_URL.to_string())
    }

    pub fn generate_completions(
        &self,
        messages: Vec<llm::Message>,
        tool_results: Vec<llm::ToolResult>,
        config: llm::Config,
    ) -> Result<llm::ChatEvent, llm::Error> {
        let http_client = CLIENT.with(|client| client.clone());

        let headers = create_headers(&self.api_key);
        let body = generate_request_body(messages, tool_results, config, false);

        let response = http_client
            .post(format!("{}/chat/completions", self.base_url))
            .headers(headers)
            .json(&body)
            .send()?;

        let status_code = response.status();

        if status_code.is_success() {
            let completion: types::CompletionResponse = response.json()?;
            completion.to_chat_event()
        } else {
            let body = response.json::<serde_json::Value>()?;

            Err(error_factory(body, status_code))
        }
    }

    pub fn stream_completions(
        &self,
        messages: Vec<llm::Message>,
        config: llm::Config,
    ) -> Result<types::ChatStream, llm::Error> {
        let http_client = CLIENT.with(|client| client.clone());

        let headers = create_headers(&self.api_key);
        let body = generate_request_body(messages, vec![], config, true);

        let response = http_client
            .post(format!("{}/chat/completions", self.base_url))
            .headers(headers)
            .json(&body)
            .send()?;

        let status_code = response.status();

        if status_code.is_success() {
            let reader = BufReader::new(response);
            Ok(types::ChatStream::from_reader(reader))
        } else {
            let body = response.json::<serde_json::Value>()?;
            Err(error_factory(body, status_code))
        }
    }

    fn new_with_base_url(api_key: String, base_url: String) -> Self {
        OpenRouter { api_key, base_url }
    }
}

impl From<reqwest::Error> for llm::Error {
    fn from(err: reqwest::Error) -> Self {
        llm::Error {
            code: llm::ErrorCode::InternalError,
            message: err.to_string(),
            provider_error_json: None,
        }
    }
}

fn create_headers(api_key: &str) -> HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "Authorization",
        format!("Bearer {}", api_key).parse().unwrap(),
    );
    headers.insert("Content-Type", "application/json".parse().unwrap());

    headers
}

fn generate_request_body(
    messages: Vec<llm::Message>,
    tool_results: Vec<llm::ToolResult>,
    config: llm::Config,
    stream: bool,
) -> serde_json::Value {
    let tool_results_json = tool_results
        .into_iter()
        .map(|result| match result {
            llm::ToolResult::Success(result) => {
                serde_json::json!({
                    "role": "tool",
                    "content": result.result_json,
                    "tool_call_id": result.id,
                })
            }
            llm::ToolResult::Error(result) => {
                serde_json::json!({
                    "role": "tool",
                    "content": result.error_message,
                    "tool_call_id": result.id,
                })
            }
        })
        .collect::<Vec<_>>();

    let mut messages = messages
        .into_iter()
        .map(|m| {
            let message = types::RequestMessage::from(m);
            serde_json::to_value(message).unwrap()
        })
        .collect::<Vec<_>>();

    messages.extend(tool_results_json);

    serde_json::json!({
        "model": config.model,
        "messages": messages,
        "temperature": config.temperature,
        "max_tokens": config.max_tokens,
        "stop": config.stop_sequences,
        "tool_choice": match config.tool_choice {
            Some(choice) => Some(serde_json::from_str(&choice).unwrap_or(choice)),
            None => None
        },
        "tools": config.tools.into_iter().map(|t| {
            serde_json::json!({
                "type": "function",
                "function": types::RequestTool::from(t),
            })
        }).collect::<Vec<_>>(),
        "stream": stream,
    })
}

fn error_factory(body: serde_json::Value, status_code: StatusCode) -> llm::Error {
    llm::Error {
        code: match status_code {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                llm::ErrorCode::AuthenticationFailed
            }
            reqwest::StatusCode::TOO_MANY_REQUESTS => llm::ErrorCode::RateLimitExceeded,
            reqwest::StatusCode::SERVICE_UNAVAILABLE
            | reqwest::StatusCode::INTERNAL_SERVER_ERROR => llm::ErrorCode::InternalError,
            _ => llm::ErrorCode::Unknown,
        },
        message: body.as_object().unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .to_string(),
        provider_error_json: Some(body.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::exports::golem::llm::llm::{self, GuestChatStream, ToolCallDelta};
    use test_utils::{http, sse};

    macro_rules! assert_sse_content_matches {
        ($stream:expr, $expected:expr) => {
            for expected in $expected.iter() {
                assert!(
                    $stream.has_next(),
                    "Expected stream to have next item, but it does not."
                );
                let next = $stream.get_next();
                assert!(
                    sse_content_part_matches(next, expected),
                    "Expected content part to match '{}', but it did not.",
                    expected
                );
            }
        };
    }

    macro_rules! assert_sse_tool_delta_matches {
        ($stream:expr, $expected:expr, $matcher:expr) => {
            for expected in $expected.iter() {
                assert!(
                    $stream.has_next(),
                    "Expected stream to have next item, but it does not."
                );
                let next = $stream.get_next();
                assert!(
                    sse_tool_delta_matches(next, |call| $matcher(call, expected.to_string())),
                    "Expected tool delta to match '{}', but it did not.",
                    expected
                );
            }
        };
    }

    fn configure_mock_open_router_server() -> http::Server {
        http::Server::start()
    }

    #[test]
    fn test_generate_completions_unauthorized_api_key() {
        let server = configure_mock_open_router_server();

        let unauthorized_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/chat/completions")
                    .header("Authorization", "Bearer invalid_api_key")
                    .header("Content-Type", "application/json")
                    .json_body_partial(r#"
                    {
                        "messages": [
                            {
                                "role": "user",
                                "content": [{"type": "text", "text": "What is the capital of France?"}]
                            }
                        ]
                    }
                    "#);

                then.status(401)
                    .header("Content-Type", "application/json")
                    .body(UNAUTHORIZED_RESPONSE);
            });

        let open_router = OpenRouter::new_with_base_url("invalid_api_key".to_owned(), server.base_url());

        let response = open_router.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        unauthorized_mock.assert();

        assert!(matches!(
            response,
            Err(llm::Error {
                code: llm::ErrorCode::AuthenticationFailed,
                ..
            })
        ));
    }

    #[test]
    fn test_generate_completions_authorized_api_key() {
        let server = configure_mock_open_router_server();

        let authorized_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/chat/completions")
                    .header("Authorization", "Bearer api_key")
                    .header("Content-Type", "application/json")
                    .json_body_partial(r#"
                    {
                        "messages": [
                            {
                                "role": "user",
                                "content": [{"type": "text", "text": "What is the capital of France?"}]
                            }
                        ]
                    }
                    "#);

                then.status(200)
                    .header("Content-Type", "application/json")
                    .body(CHAT_COMPLETION_RESPONSE);
            });

        let open_router = OpenRouter::new_with_base_url("api_key".to_owned(), server.base_url());

        let response = open_router.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        authorized_mock.assert();

        if let Ok(llm::ChatEvent::Message(llm::CompleteResponse { content, .. })) = response {
            let content = content
                .first()
                .expect("Expected at least one content part")
                .clone();
            match content {
                llm::ContentPart::Text(text) => {
                    assert_eq!(text, "The capital of France is Paris.");
                }
                _ => {
                    assert!(false, "Expected text content part");
                }
            }
        } else {
            assert!(false, "Expected successful response");
        }
    }

    #[test]
    fn test_generate_completions_function_calling() {
        let server = configure_mock_open_router_server();

        let function_calling_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/chat/completions")
                    .header("Authorization", "Bearer api_key")
                    .header("Content-Type", "application/json")
                    .json_body_partial(
                        r#"
                        {
                            "tool_choice": "none",
                            "tools": [
                                {
                                    "type": "function",
                                    "function": {
                                        "name": "get_current_weather",
                                        "description": "Get the current weather in a given location.",
                                        "parameters": {
                                            "type": "object",
                                            "properties": {
                                                "location": {
                                                    "type": "string",
                                                    "description": "The location to get the weather for."
                                                }
                                            },
                                            "required": ["location"]
                                        }
                                    }
                                }
                            ]
                        }"#,
                    );

                then.status(200)
                    .header("Content-Type", "application/json")
                    .body(FUNCTION_CALLING_RESPONSE);
            });

        let open_router = OpenRouter::new_with_base_url("api_key".to_owned(), server.base_url());

        let response = open_router.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the weather today in Boston, MA?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![llm::ToolDefinition {
                    name: "get_current_weather".to_owned(),
                    description: Some("Get the current weather in a given location.".to_owned()),
                    parameters_schema: r#"{
                            "type": "object",
                            "properties": {
                                "location": {
                                    "type": "string",
                                    "description": "The location to get the weather for."
                                }
                            },
                            "required": ["location"]
                        }"#
                    .to_owned(),
                }],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        function_calling_mock.assert();

        if let Ok(llm::ChatEvent::ToolRequest(calls)) = response {
            let call = calls.first().expect("Expected at least one call").clone();
            assert_eq!(call.name, "get_current_weather");
        } else {
            assert!(false, "Expected successful response");
        }
    }

    #[test]
    fn test_stream_completions_unauthorized_api_key() {
        let server = configure_mock_open_router_server();

        let unauthorized_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/chat/completions")
                    .header("Authorization", "Bearer invalid_api_key")
                    .header("Content-Type", "application/json")
                    .json_body_partial(r#"
                    {
                        "messages": [
                            {
                                "role": "user",
                                "content": [{"type": "text", "text": "What is the capital of France?"}]
                            }
                        ]
                    }
                    "#);

                then.status(401)
                    .header("Content-Type", "application/json")
                    .body(UNAUTHORIZED_RESPONSE);
            });

        let open_router = OpenRouter::new_with_base_url("invalid_api_key".to_owned(), server.base_url());

        let response = open_router.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        unauthorized_mock.assert();

        assert!(matches!(
            response,
            Err(llm::Error {
                code: llm::ErrorCode::AuthenticationFailed,
                ..
            })
        ));
    }

    #[test]
    fn test_stream_completions_authorized_api_key() {
        let sse_server = sse::Server::start(
            "/chat/completions".to_owned(),
            CHAT_COMPLETION_EVENT_STREAM
                .iter()
                .map(|s| String::from_str(s).unwrap())
                .collect(),
        );

        let open_router = OpenRouter::new_with_base_url("api_key".to_owned(), sse_server.base_url.clone());

        let response = open_router.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the meaning of life? keep the answer short".to_owned(),
                )],
            }],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        if let Ok(stream) = response {
            assert_sse_content_matches!(stream, ["", "The", " meaning", " of", " life",]);

            assert!(stream.has_next(), "Expected stream to have next item");
            let mut next = stream.get_next();
            while stream.has_next() {
                next = stream.get_next();
            }
            assert!(
                matches!(next, llm::StreamEvent::Finish(llm::ResponseMetadata { .. })),
                "Expected finish event at end of stream"
            )
        } else {
            assert!(false, "Expected successful response");
        }

        sse_server.shutdown();
    }

    #[test]
    fn test_stream_completions_function_calling() {
        let sse_server = sse::Server::start(
            "/chat/completions".to_owned(),
            FUNCTION_CALLING_EVENT_STREAM
                .iter()
                .map(|s| String::from_str(s).unwrap())
                .collect(),
        );

        let open_router = OpenRouter::new_with_base_url("api_key".to_owned(), sse_server.base_url.clone());

        let response = open_router.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the weather today in Boston, MA?".to_owned(),
                )],
            }],
            llm::Config {
                model: "gpt-4o".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![llm::ToolDefinition {
                    name: "get_current_weather".to_owned(),
                    description: Some("Get the current weather in a given location.".to_owned()),
                    parameters_schema: r#"{
                            "type": "object",
                            "properties": {
                                "location": {
                                    "type": "string",
                                    "description": "The location to get the weather for."
                                }
                            },
                            "required": ["location"]
                        }"#
                    .to_owned(),
                }],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![],
            },
        );

        if let Ok(stream) = response {
            assert_sse_tool_delta_matches!(
                stream,
                ["get_current_weather",],
                |call: llm::ToolCallDelta, expected: String| {
                    call.name.expect("name should be present") == expected
                }
            );

            assert_sse_tool_delta_matches!(
                stream,
                ["{\"", "location", "\":\"", "Boston",],
                |call: llm::ToolCallDelta, expected: String| { call.arguments_json == expected }
            );
        } else {
            assert!(false, "Expected successful response");
        }

        sse_server.shutdown();
    }

    fn sse_tool_delta_matches(
        event: llm::StreamEvent,
        expr: impl Fn(ToolCallDelta) -> bool,
    ) -> bool {
        if let llm::StreamEvent::Delta(llm::StreamDelta { tool_calls, .. }) = event {
            if let Some(tool_calls) = tool_calls {
                if let Some(call) = tool_calls.first() {
                    return expr(call.clone());
                }
            }
        }
        false
    }

    fn sse_content_part_matches(event: llm::StreamEvent, expected: &str) -> bool {
        if let llm::StreamEvent::Delta(llm::StreamDelta { content, .. }) = event {
            if let Some(content) = content {
                if let Some(part) = content.first() {
                    if let llm::ContentPart::Text(text) = part {
                        return text == expected;
                    }
                }
            }
        }
        false
    }

    const CHAT_COMPLETION_RESPONSE: &str = r#"
    {
        "id": "chatcmpl-abc123",
        "object": "chat.completion",
        "created": 1699896916,
        "model": "gpt-4o",
        "choices": [
            {
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "The capital of France is Paris."
            },
            "logprobs": null,
            "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 82,
            "completion_tokens": 17,
            "total_tokens": 99,
            "completion_tokens_details": {
                "reasoning_tokens": 0,
                "accepted_prediction_tokens": 0,
                "rejected_prediction_tokens": 0
            }
        }
    }"#;

    const UNAUTHORIZED_RESPONSE: &str = r#"
    {
        "error": {
            "message": "You didn't provide an API key. You need to provide your API key in an Authorization header using Bearer auth (i.e. Authorization: Bearer YOUR_KEY), or as the password field (with blank username) if you're accessing the API from your browser and are prompted for a username and password. You can obtain an API key from https://platform.open_router.com/account/api-keys.",
            "type": "invalid_request_error",
            "param": null,
            "code": null
        }
    }"#;

    const FUNCTION_CALLING_RESPONSE: &str = r#"
    {
        "id": "chatcmpl-abc123",
        "object": "chat.completion",
        "created": 1699896916,
        "model": "gpt-4o-mini",
        "choices": [
            {
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                {
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "get_current_weather",
                        "arguments": "{\n\"location\": \"Boston, MA\"\n}"
                    }
                }
                ]
            },
            "logprobs": null,
            "finish_reason": "tool_calls"
            }
        ],
        "usage": {
            "prompt_tokens": 82,
            "completion_tokens": 17,
            "total_tokens": 99,
            "completion_tokens_details": {
                "reasoning_tokens": 0,
                "accepted_prediction_tokens": 0,
                "rejected_prediction_tokens": 0
            }
        }
    }"#;

    const CHAT_COMPLETION_EVENT_STREAM: [&str; 27] = [
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"role":"assistant","content":"","refusal":null},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":"The"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" meaning"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" of"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" life"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" is"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" subjective"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" and"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" varies"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" for"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" each"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" person"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":","},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" often"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" involving"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" the"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" pursuit"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" of"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" happiness"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":","},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" purpose"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":","},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" and"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":" connection"},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"content":"."},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BPtIHMG1W8Gi6haKboovsXFHfPIu2","object":"chat.completion.chunk","created":1745510449,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{},"logprobs":null,"finish_reason":"stop"}]}"#,
        "[DONE]",
    ];

    const FUNCTION_CALLING_EVENT_STREAM: [&str; 10] = [
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_ssTROIKA9lFrLcXpVidnBGFJ","type":"function","function":{"name":"get_current_weather","arguments":""}}],"refusal":null},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\""}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"location"}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":\""}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"Boston"}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":","}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" MA"}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"}"}}]},"logprobs":null,"finish_reason":null}]}"#,
        r#"{"id":"chatcmpl-BOuPLoRj1MWDOl7ZvWlYxQE5M9bZ8","object":"chat.completion.chunk","created":1745276403,"model":"gpt-4o-2024-08-06","service_tier":"default","system_fingerprint":"fp_f5bdcc3276","choices":[{"index":0,"delta":{},"logprobs":null,"finish_reason":"tool_calls"}]}"#,
        "[DONE]",
    ];
}
