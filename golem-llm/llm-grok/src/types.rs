use std::cell::RefCell;
use std::io::BufRead;

use crate::exports::golem::llm::llm;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct CompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    pub service_tier: Option<String>,
}

const GROK_PROVIDER_ID: &str = "grok";

impl CompletionResponse {
    pub fn to_chat_event(self) -> Result<llm::ChatEvent, llm::Error> {
        let choice = self.choices.first().ok_or(llm::Error {
            code: llm::ErrorCode::InternalError,
            message: "No choices were returned from API call".to_owned(),
            provider_error_json: None,
        })?;

        if choice.message.content == None {
            Ok(llm::ChatEvent::ToolRequest(
                choice
                    .message
                    .tool_calls
                    .clone()
                    .unwrap_or(vec![])
                    .into_iter()
                    .map(|call| llm::ToolCall {
                        id: call.id,
                        name: call.function.name,
                        arguments_json: call.function.arguments,
                    })
                    .collect(),
            ))
        } else {
            Ok(llm::ChatEvent::Message(llm::CompleteResponse {
                id: self.id,
                content: match choice.message.content.clone() {
                    Some(content) => vec![llm::ContentPart::Text(content)],
                    None => vec![],
                },
                tool_calls: choice
                    .message
                    .tool_calls
                    .clone()
                    .unwrap_or(vec![])
                    .into_iter()
                    .map(|call| llm::ToolCall {
                        id: call.id,
                        name: call.function.name,
                        arguments_json: call.function.arguments,
                    })
                    .collect(),
                metadata: llm::ResponseMetadata {
                    finish_reason: Some(choice.finish_reason.clone().into()),
                    provider_id: Some(GROK_PROVIDER_ID.to_owned()),
                    provider_metadata_json: None,
                    timestamp: None,
                    usage: Some(self.usage.into()),
                },
            }))
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Choice {
    pub index: u32,
    pub message: ResponseMessage,
    pub logprobs: Option<serde_json::Value>,
    pub finish_reason: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ResponseToolCall>>,
    pub refusal: Option<String>,
    pub annotations: Option<Vec<serde_json::Value>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub prompt_tokens_details: Option<TokenDetails>,
    pub completion_tokens_details: TokenDetails,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TokenDetails {
    pub cached_tokens: Option<u32>,
    pub audio_tokens: Option<u32>,
    pub reasoning_tokens: Option<u32>,
    pub accepted_prediction_tokens: Option<u32>,
    pub rejected_prediction_tokens: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct RequestMessage {
    role: String,
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResponseToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ResponseFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResponseFunction {
    pub name: String,
    pub arguments: String,
}

impl From<llm::Message> for RequestMessage {
    fn from(message: llm::Message) -> Self {
        RequestMessage {
            role: match message.role {
                llm::Role::User => "user".to_string(),
                llm::Role::System => "developer".to_string(),
                llm::Role::Tool => "tool".to_string(),
                llm::Role::Assistant => "assistant".to_string(),
            },
            content: message
                .content
                .iter()
                .map(|part| match part {
                    llm::ContentPart::Text(text) => serde_json::json!({
                        "type": "text",
                        "text": text
                    }),
                    llm::ContentPart::Image(image_url) => serde_json::json!({
                        "type": "image_url",
                        "image_url": image_url.url,
                    }),
                })
                .collect(),
            name: message.name.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RequestTool {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

impl From<llm::ToolDefinition> for RequestTool {
    fn from(tool_definition: llm::ToolDefinition) -> Self {
        RequestTool {
            name: tool_definition.name.clone(),
            description: tool_definition.description.clone(),
            parameters: match serde_json::from_str(&tool_definition.parameters_schema) {
                Ok(params) => Some(params),
                Err(_) => None,
            },
        }
    }
}

impl From<String> for llm::FinishReason {
    fn from(reason: String) -> Self {
        match reason.as_str() {
            "stop" => llm::FinishReason::Stop,
            "length" => llm::FinishReason::Length,
            "content_filter" => llm::FinishReason::ContentFilter,
            "tool_calls" => llm::FinishReason::ToolCalls,
            _ => llm::FinishReason::Other,
        }
    }
}

impl From<Usage> for llm::Usage {
    fn from(usage: Usage) -> Self {
        llm::Usage {
            input_tokens: Some(usage.prompt_tokens),
            output_tokens: Some(usage.completion_tokens),
            total_tokens: Some(usage.total_tokens),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CompletionResponseChunk {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub service_tier: String,
    pub system_fingerprint: String,
    pub choices: Vec<ChunkChoice>,
}

impl CompletionResponseChunk {
    pub fn to_stream_event(self) -> llm::StreamEvent {
        let choice = self.choices.first().unwrap();

        llm::StreamEvent::Delta(llm::StreamDelta {
            content: match choice.delta.content.clone() {
                Some(content) => Some(vec![llm::ContentPart::Text(content)]),
                None => None,
            },
            tool_calls: match choice.delta.tool_calls.clone() {
                Some(tool_calls) => Some(
                    tool_calls
                        .into_iter()
                        .map(|call| llm::ToolCallDelta {
                            id: call.id,
                            name: call.function.name,
                            arguments_json: call.function.arguments,
                        })
                        .collect(),
                ),
                None => None,
            },
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub logprobs: Option<serde_json::Value>,
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<DeltaToolCall>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeltaToolCall {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: DeltaFunction,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DeltaFunction {
    pub name: Option<String>,
    pub arguments: String,
}

pub struct ChatStreamState<'r> {
    reached_end: bool,
    last_chunk: Option<CompletionResponseChunk>,
    reader: Box<dyn BufRead + 'r>,
}

pub struct ChatStream {
    state: RefCell<ChatStreamState<'static>>,
}

impl ChatStream {
    pub fn from_reader(reader: impl BufRead + 'static) -> Self {
        ChatStream {
            state: RefCell::new(ChatStreamState {
                reached_end: false,
                last_chunk: None,
                reader: Box::new(reader),
            }),
        }
    }

    fn read_line(&self, buffer: &mut String) -> std::io::Result<usize> {
        (*self.state.borrow_mut()).reader.read_line(buffer)
    }

    fn has_finished_stream(&self) -> bool {
        (*self.state.borrow()).reached_end
    }

    fn finish_stream(&self) {
        (*self.state.borrow_mut()).reached_end = true;
    }

    fn get_final_chunk(&self) -> Option<CompletionResponseChunk> {
        (*self.state.borrow()).last_chunk.clone()
    }

    fn set_final_chunk(&self, chunk: CompletionResponseChunk) {
        (*self.state.borrow_mut()).last_chunk = Some(chunk);
    }
}

impl llm::GuestChatStream for ChatStream {
    fn get_next(&self) -> llm::StreamEvent {
        if self.has_finished_stream() {
            return llm::StreamEvent::Error(llm::Error {
                code: llm::ErrorCode::InternalError,
                message: "Stream has already ended".to_owned(),
                provider_error_json: None,
            });
        }
        let mut payload = String::new();
        if let Err(err) = self.read_line(&mut payload) {
            self.finish_stream();
            return llm::StreamEvent::Error(llm::Error {
                code: llm::ErrorCode::InternalError,
                message: format!("Error occurred while reading event stream: {}", err),
                provider_error_json: None,
            });
        } else {
            if payload.is_empty() || payload == "\n" {
                self.get_next()
            } else {
                if let Some(payload) = payload.strip_prefix("data: ") {
                    if payload == "[DONE]\n" {
                        self.finish_stream();

                        let last_chunk = self.get_final_chunk();

                        if last_chunk.is_none() {
                            llm::StreamEvent::Error(llm::Error {
                                code: llm::ErrorCode::InternalError,
                                message: "Invalid order of events received. DONE event received before any other chunks".to_owned(),
                                provider_error_json: None,
                            })
                        } else {
                            let chunk = last_chunk.clone().unwrap();
                            llm::StreamEvent::Finish(llm::ResponseMetadata {
                                finish_reason: Some(
                                    chunk.choices[0].finish_reason.clone().unwrap().into(),
                                ),
                                provider_id: Some(GROK_PROVIDER_ID.to_owned()),
                                provider_metadata_json: None,
                                timestamp: Some(format!("{}", chunk.created)),
                                usage: None,
                            })
                        }
                    } else {
                        let chunk: CompletionResponseChunk = serde_json::from_str(payload).unwrap();
                        self.set_final_chunk(chunk.clone());
                        chunk.to_stream_event()
                    }
                } else {
                    llm::StreamEvent::Error(llm::Error {
                        code: llm::ErrorCode::InternalError,
                        message: format!(
                            "Invalid event received from event stream >>> {}",
                            payload
                        ),
                        provider_error_json: None,
                    })
                }
            }
        }
    }

    fn has_next(&self) -> bool {
        !self.has_finished_stream()
    }
}
