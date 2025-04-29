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

const BASE_URL: &str = "https://api.anthropic.com";

pub struct Anthropic {
    api_key: String,
    base_url: String,
}

impl Anthropic {
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

        let anthropic_version = config
            .provider_options
            .iter()
            .find(|opt| opt.key == "anthropic-version")
            .ok_or(llm::Error {
                code: llm::ErrorCode::InvalidRequest,
                message: "'anthropic-version' is missing! Pass it in the provider_options array"
                    .to_owned(),
                provider_error_json: None,
            })?
            .value
            .clone();

        let anthropic_beta = config
            .provider_options
            .iter()
            .find(|opt| opt.key == "anthropic-beta")
            .map(|kv| kv.value.clone());

        let headers = create_headers(&self.api_key, anthropic_version, anthropic_beta);
        let body = generate_request_body(messages, tool_results, config, false);

        let response = http_client
            .post(format!("{}/v1/messages", self.base_url))
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

        let anthropic_version = config
            .provider_options
            .iter()
            .find(|opt| opt.key == "anthropic-version")
            .ok_or(llm::Error {
                code: llm::ErrorCode::InvalidRequest,
                message: "'anthropic-version' is missing! Pass it in the provider_options array"
                    .to_owned(),
                provider_error_json: None,
            })?
            .value
            .clone();

        let anthropic_beta = config
            .provider_options
            .iter()
            .find(|opt| opt.key == "anthropic-beta")
            .map(|kv| kv.value.clone());

        let headers = create_headers(&self.api_key, anthropic_version, anthropic_beta);

        let body = generate_request_body(messages, vec![], config, true);

        let response = http_client
            .post(format!("{}/v1/messages", self.base_url))
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
        Anthropic { api_key, base_url }
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

fn create_headers(
    api_key: &str,
    anthropic_version: String,
    anthropic_beta: Option<String>,
) -> HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-api-key", api_key.parse().unwrap());
    headers.insert("Content-Type", "application/json".parse().unwrap());

    headers.insert("anthropic-version", anthropic_version.parse().unwrap());

    if let Some(beta) = anthropic_beta {
        headers.insert("anthropic-beta", beta.parse().unwrap());
    }

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
                    "type": "tool_result",
                    "tool_use_id": result.id,
                    "content": result.result_json,
                })
            }
            llm::ToolResult::Error(result) => {
                serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": result.id,
                    "content": result.error_message,
                })
            }
        })
        .collect::<Vec<_>>();

    let system_message = config
        .provider_options
        .iter()
        .find(|opt| opt.key == "system_message")
        .map(|kv| kv.value.clone());

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
        "max_tokens": config.max_tokens.or(Some(1024)),
        "stop_sequences": config.stop_sequences,
        "tool_choice": match config.tool_choice {
            Some(choice) => Some(serde_json::from_str::<serde_json::Value>(&choice).unwrap_or(serde_json::Value::String(choice))),
            None => None
        },
        "system": system_message,
        "tools": config.tools.into_iter().map(|t| types::RequestTool::from(t)).collect::<Vec<_>>(),
        "stream": stream,
    })
}

fn error_factory(body: serde_json::Value, status_code: StatusCode) -> llm::Error {
    llm::Error {
        code: match status_code {
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::PAYLOAD_TOO_LARGE => {
                llm::ErrorCode::InvalidRequest
            }
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
    use crate::exports::golem::llm::llm::{self, GuestChatStream};
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

    fn configure_mock_anthropic_server() -> http::Server {
        http::Server::start()
    }

    #[test]
    fn test_generate_completions_unauthorized_api_key() {
        let server = configure_mock_anthropic_server();

        let unauthorized_mock = server.mock(|when, then| {
            when.method(http::Method::POST)
                .path("/v1/messages")
                .header("x-api-key", "invalid_api_key")
                .header("Content-Type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .json_body_partial(
                    r#"
                    {
                        "model": "claude-3-7-sonnet-20250219",
                        "max_tokens": 1024,
                        "messages": [
                            {"role": "user", "content": [{"type": "text", "text": "What is the capital of France?"}]}
                        ]
                    }
                    "#,
                );

            then.status(401)
                .header("Content-Type", "application/json")
                .body(UNAUTHORIZED_RESPONSE);
        });

        let anthropic =
            Anthropic::new_with_base_url("invalid_api_key".to_owned(), server.base_url());

        let response = anthropic.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
                temperature: Some(1.0),
                max_tokens: None,
                stop_sequences: None,
                tools: vec![],
                tool_choice: Some(r#"{"type": "none"}"#.to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
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
        let server = configure_mock_anthropic_server();

        let authorized_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/v1/messages")
                    .header("x-api-key", "api_key")
                .header("Content-Type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .json_body_partial(
                    r#"
                    {
                        "model": "claude-3-7-sonnet-20250219",
                        "max_tokens": 1024,
                        "messages": [
                            {"role": "user", "content": [{"type": "text", "text": "What is the capital of France?"}]}
                        ]
                    }
                    "#,
                );

                then.status(200)
                    .header("Content-Type", "application/json")
                    .body(CHAT_COMPLETION_RESPONSE);
            });

        let anthropic = Anthropic::new_with_base_url("api_key".to_owned(), server.base_url());

        let response = anthropic.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
                temperature: Some(1.0),
                max_tokens: None,
                stop_sequences: None,
                tools: vec![],
                tool_choice: Some("none".to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
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
        let server = configure_mock_anthropic_server();

        let function_calling_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/v1/messages")
                    .header("x-api-key", "api_key")
                .header("Content-Type", "application/json")
                .header("anthropic-version", "2023-06-01")
                    .json_body_partial(
                        r#"
                        {
                            "tool_choice": {
                                "type": "none"
                            },
                            "tools": [
                                {
                                    "name": "get_current_weather",
                                    "description": "Get the current weather in a given location.",
                                    "input_schema": {
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
                            ]
                        }"#,
                    );

                then.status(200)
                    .header("Content-Type", "application/json")
                    .body(FUNCTION_CALLING_RESPONSE);
            });

        let anthropic = Anthropic::new_with_base_url("api_key".to_owned(), server.base_url());

        let response = anthropic.generate_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the weather today in Boston, MA?".to_owned(),
                )],
            }],
            vec![],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
                temperature: Some(1.0),
                max_tokens: None,
                stop_sequences: None,
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
                tool_choice: Some(r#"{"type": "none"}"#.to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
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
        let server = configure_mock_anthropic_server();

        let unauthorized_mock = server
            .mock(|when, then| {
                when.method(http::Method::POST)
                    .path("/v1/messages")
                    .header("x-api-key", "invalid_api_key")
                    .header("Content-Type", "application/json")
                .header("anthropic-version", "2023-06-01")
                .json_body_partial(
                    r#"
                    {
                        "model": "claude-3-7-sonnet-20250219",
                        "max_tokens": 1024,
                        "messages": [
                            {"role": "user", "content": [{"type": "text", "text": "What is the capital of France?"}]}
                        ]
                    }
                    "#,
                );

                then.status(401)
                    .header("Content-Type", "application/json")
                    .body(UNAUTHORIZED_RESPONSE);
            });

        let anthropic =
            Anthropic::new_with_base_url("invalid_api_key".to_owned(), server.base_url());

        let response = anthropic.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the capital of France?".to_owned(),
                )],
            }],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
                temperature: Some(1.0),
                max_tokens: None,
                stop_sequences: None,
                tools: vec![],
                tool_choice: Some(r#"{"type": "none"}"#.to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
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
            "/v1/messages".to_owned(),
            CHAT_COMPLETION_EVENT_STREAM
                .iter()
                .map(|s| String::from_str(s).unwrap())
                .collect(),
        );

        let anthropic =
            Anthropic::new_with_base_url("api_key".to_owned(), sse_server.base_url.clone());

        let response = anthropic.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the meaning of life? keep the answer short".to_owned(),
                )],
            }],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
                temperature: Some(1.0),
                max_tokens: Some(100),
                stop_sequences: Some(vec!["violation".to_owned()]),
                tools: vec![],
                tool_choice: Some(r#"{"type": "none"}"#.to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
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
            "/v1/messages".to_owned(),
            FUNCTION_CALLING_EVENT_STREAM
                .iter()
                .map(|s| String::from_str(s).unwrap())
                .collect(),
        );

        let anthropic =
            Anthropic::new_with_base_url("api_key".to_owned(), sse_server.base_url.clone());

        let response = anthropic.stream_completions(
            vec![llm::Message {
                role: llm::Role::User,
                name: None,
                content: vec![llm::ContentPart::Text(
                    "What is the weather today in Boston, MA?".to_owned(),
                )],
            }],
            llm::Config {
                model: "claude-3-7-sonnet-20250219".to_owned(),
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
                tool_choice: Some(r#"{"type": "none"}"#.to_owned()),
                provider_options: vec![llm::Kv {
                    key: "anthropic-version".to_owned(),
                    value: "2023-06-01".to_owned(),
                }],
            },
        );

        if let Ok(stream) = response {
            assert_sse_content_matches!(
                stream,
                [
                    "",
                    "Okay",
                    ",",
                    " let",
                    "'s",
                    " check",
                    " the",
                    " weather",
                    " for",
                    " San",
                    " Francisco",
                    ",",
                    " CA",
                    ":",
                ]
            );

            assert_sse_tool_delta_matches!(
                stream,
                ["get_weather",],
                |call: llm::ToolCallDelta, expected: String| {
                    call.name.expect("name should be present") == expected
                }
            );

            assert_sse_tool_delta_matches!(
                stream,
                [
                    "",
                    "{\"location\":",
                    " \"San",
                    " Francisc",
                    "o,",
                    " CA\"",
                    ", ",
                    "\"unit\": \"fah",
                    "renheit\"}"
                ],
                |call: llm::ToolCallDelta, expected: String| { call.arguments_json == expected }
            );

            assert!(stream.has_next(), "Expected stream to have next item");
            let next = stream.get_next();

            if let llm::StreamEvent::Finish(llm::ResponseMetadata {
                finish_reason,
                usage,
                ..
            }) = next
            {
                assert!(!stream.has_next(), "Expected no more items in stream");

                assert!(
                    matches!(finish_reason, Some(llm::FinishReason::ToolCalls)),
                    "Expected finish reason to be 'tool_calls', but it was {:?}",
                    finish_reason
                );

                assert_eq!(usage.unwrap().input_tokens, Some(472));
                assert_eq!(usage.unwrap().output_tokens, Some(89));
                assert_eq!(usage.unwrap().total_tokens, Some(89 + 472));
            } else {
                assert!(false, "Expected finish event at end of stream");
            }
        } else {
            assert!(false, "Expected successful response");
        }

        sse_server.shutdown();
    }

    fn sse_tool_delta_matches(
        event: llm::StreamEvent,
        expr: impl Fn(llm::ToolCallDelta) -> bool,
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
        "content": [
            {
                "text": "The capital of France is Paris.",
                "type": "text"
            }
        ],
        "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF",
        "model": "claude-3-7-sonnet-20250219",
        "role": "assistant",
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "type": "message",
        "usage": {
            "input_tokens": 2095,
            "output_tokens": 503
        }
    }"#;

    const UNAUTHORIZED_RESPONSE: &str = r#"
    {
        "error": {
            "message": "Invalid request",
            "type": "invalid_request_error"
        },
        "type": "error"
    }"#;

    const FUNCTION_CALLING_RESPONSE: &str = r#"
    {
        "content": [
            {
                "type": "tool_use",
                "id": "toolu_01D7FLrfh4GYq7yT1ULFeyMV",
                "name": "get_current_weather",
                "input": { "location": "Boston, MA" }
            }
        ],
        "id": "msg_013Zva2CMHLNnXjNJJKqJ2EF",
        "model": "claude-3-7-sonnet-20250219",
        "role": "assistant",
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "type": "message",
        "usage": {
            "input_tokens": 2095,
            "output_tokens": 503
        }
    }"#;

    const CHAT_COMPLETION_EVENT_STREAM: [&str; 16] = [
        r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY", "type": "message", "role": "assistant", "content": [], "model": "claude-3-7-sonnet-20250219", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 25, "output_tokens": 1}}}"#,
        r#"event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}"#,
        r#"event: ping
data: {"type": "ping"}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "The"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " meaning"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " of"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " life"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " is"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " a"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " subjective"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " attempt"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " at"}}"#,
        r#"event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": " death"}}"#,
        r#"event: content_block_stop
data: {"type": "content_block_stop", "index": 0}"#,
        r#"event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence":null}, "usage": {"output_tokens": 15}}"#,
        r#"event: message_stop
data: {"type": "message_stop"}"#,
    ];

    const FUNCTION_CALLING_EVENT_STREAM: [&str; 30] = [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_014p7gG3wDgGV9EUtLvnow3U","type":"message","role":"assistant","model":"claude-3-haiku-20240307","stop_sequence":null,"usage":{"input_tokens":472,"output_tokens":2},"content":[],"stop_reason":null}}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"event: ping
data: {"type": "ping"}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Okay"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":","}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" let"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"'s"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" check"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" the"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" weather"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" for"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" San"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" Francisco"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":","}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" CA"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":":"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01T1x1fJ34qAmk2tNTrN7Up6","name":"get_weather","input":{}}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"San"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" Francisc"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"o,"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" CA\""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":", "}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"unit\": \"fah"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"renheit\"}"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":89}}"#,
        r#"event: message_stop
data: {"type":"message_stop"}"#,
    ];
}
