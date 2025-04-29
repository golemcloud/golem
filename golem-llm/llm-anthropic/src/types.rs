use std::cell::RefCell;
use std::io::BufRead;

use crate::exports::golem::llm::llm;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct CompletionResponse {
    pub id: String,
    pub model: String,
    pub stop_reason: String,
    pub content: Vec<Content>,
    pub stop_sequence: Option<String>,
    pub usage: Usage,
}

const ANTHROPIC_PROVIDER_ID: &str = "anthropic";

impl CompletionResponse {
    pub fn to_chat_event(self) -> Result<llm::ChatEvent, llm::Error> {
        if self.content.is_empty() {
            return Err(llm::Error {
                code: llm::ErrorCode::InternalError,
                message: "no content parts were returned in the response".to_owned(),
                provider_error_json: None,
            });
        }

        let tool_calls = self
            .content
            .iter()
            .filter_map(|content| {
                if let Content::ToolUse { id, name, input } = content {
                    Some(llm::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments_json: input.to_string(),
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<llm::ToolCall>>();

        if tool_calls.len() == self.content.len() {
            return Ok(llm::ChatEvent::ToolRequest(tool_calls));
        } else {
            let content_without_tool_calls = self
                .content
                .iter()
                .filter_map(|content| {
                    if let Content::Text { text } = content {
                        Some(llm::ContentPart::Text(text.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<llm::ContentPart>>();

            Ok(llm::ChatEvent::Message(llm::CompleteResponse {
                id: self.id,
                content: content_without_tool_calls,
                tool_calls: tool_calls,
                metadata: llm::ResponseMetadata {
                    finish_reason: Some(self.stop_reason.into()),
                    provider_id: Some(ANTHROPIC_PROVIDER_ID.to_owned()),
                    provider_metadata_json: None,
                    timestamp: None,
                    usage: Some(self.usage.into()),
                },
            }))
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum Content {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
}

#[derive(Serialize, Deserialize)]
pub struct RequestMessage {
    role: String,
    content: serde_json::Value,
}

fn parse_base64(url: &str) -> Option<serde_json::Value> {
    // Check and strip "data:"
    let url = url.strip_prefix("data:")?;
    let mut parts = url.splitn(2, ',');

    let meta = parts.next()?;
    let data = parts.next()?;

    let media_type = meta.strip_suffix(";base64")?.to_string();

    Some(serde_json::json!({
        "type": "base64".to_string(),
        "media_type": media_type,
        "data": data.to_string(),
    }))
}

impl From<llm::Message> for RequestMessage {
    fn from(message: llm::Message) -> Self {
        RequestMessage {
            role: match message.role {
                llm::Role::Tool => "tool".to_string(),
                llm::Role::Assistant => "assistant".to_string(),
                _ => "user".to_string(),
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
                        "type": "image",
                        "source": parse_base64(&image_url.url),
                    }),
                })
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct RequestTool {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_schema: Option<serde_json::Value>,
}

impl From<llm::ToolDefinition> for RequestTool {
    fn from(tool_definition: llm::ToolDefinition) -> Self {
        RequestTool {
            name: tool_definition.name.clone(),
            description: tool_definition.description.clone(),
            input_schema: match serde_json::from_str(&tool_definition.parameters_schema) {
                Ok(params) => Some(params),
                Err(_) => None,
            },
        }
    }
}

impl From<String> for llm::FinishReason {
    fn from(reason: String) -> Self {
        match reason.as_str() {
            "end_turn" => llm::FinishReason::Stop,
            "max_tokens" => llm::FinishReason::Length,
            "stop_sequence" => llm::FinishReason::ContentFilter,
            "tool_use" => llm::FinishReason::ToolCalls,
            _ => llm::FinishReason::Other,
        }
    }
}

impl From<Usage> for llm::Usage {
    fn from(usage: Usage) -> Self {
        llm::Usage {
            input_tokens: usage.input_tokens,
            output_tokens: Some(usage.output_tokens),
            total_tokens: Some(usage.input_tokens.unwrap_or(0) + usage.output_tokens),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum CompletionEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Message },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: u32,
        content_block: ContentBlock,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: u32, delta: Delta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: u32 },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDelta, usage: Usage },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub model: String,
    pub usage: Usage,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Delta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "input_json_delta")]
    ToolArguments { partial_json: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MessageDelta {
    pub stop_reason: Option<String>,
    pub stop_sequence: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Usage {
    pub input_tokens: Option<u32>,
    pub output_tokens: u32,
}

impl CompletionEvent {
    pub fn to_stream_event(self) -> llm::StreamEvent {
        match self {
            CompletionEvent::ContentBlockDelta { delta, .. } => {
                llm::StreamEvent::Delta(match delta {
                    Delta::Text { text } => llm::StreamDelta {
                        content: Some(vec![llm::ContentPart::Text(text)]),
                        tool_calls: None,
                    },
                    Delta::ToolArguments { partial_json } => llm::StreamDelta {
                        content: None,
                        tool_calls: Some(vec![llm::ToolCallDelta {
                            id: None,
                            name: None,
                            arguments_json: partial_json,
                        }]),
                    },
                })
            }
            CompletionEvent::ContentBlockStart { content_block, .. } => {
                llm::StreamEvent::Delta(match content_block {
                    ContentBlock::ToolUse { id, name } => llm::StreamDelta {
                        content: None,
                        tool_calls: Some(vec![llm::ToolCallDelta {
                            id: Some(id),
                            name: Some(name),
                            arguments_json: "".to_owned(),
                        }]),
                    },
                    ContentBlock::Text { text } => llm::StreamDelta {
                        content: Some(vec![llm::ContentPart::Text(text)]),
                        tool_calls: None,
                    },
                })
            }
            _ => unreachable!("completion event can't be converted to stream event"),
        }
    }
}

pub struct ChatStreamState<'r> {
    reached_end: bool,
    input_tokens: Option<u32>,
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
                input_tokens: None,
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

    fn get_input_tokens(&self) -> Option<u32> {
        (*self.state.borrow()).input_tokens
    }

    fn set_input_tokens(&self, input_tokens: u32) {
        (*self.state.borrow_mut()).input_tokens = Some(input_tokens);
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
                return self.get_next();
            }

            if let Some(..) = payload.strip_prefix("event: ") {
                return self.get_next();
            }

            if let Some(payload) = payload.strip_prefix("data: ") {
                if let Ok(completion_event) = serde_json::from_str::<CompletionEvent>(payload) {
                    match completion_event {
                        CompletionEvent::MessageStart { message } => {
                            self.set_input_tokens(message.usage.input_tokens.unwrap_or(0));
                            return self.get_next();
                        }
                        CompletionEvent::MessageDelta { delta, usage } => {
                            self.finish_stream();

                            if self.get_input_tokens().is_none() {
                                return llm::StreamEvent::Error(llm::Error {
                                        code: llm::ErrorCode::InternalError,
                                        message: "Invalid order of events received. DONE event received before any other chunks".to_owned(),
                                        provider_error_json: None,
                                    });
                            }

                            let input_tokens = self.get_input_tokens().unwrap();

                            return llm::StreamEvent::Finish(llm::ResponseMetadata {
                                finish_reason: delta.stop_reason.map(|reason| reason.into()),
                                provider_id: Some(ANTHROPIC_PROVIDER_ID.to_owned()),
                                provider_metadata_json: None,
                                timestamp: None,
                                usage: Some(llm::Usage {
                                    input_tokens: Some(input_tokens),
                                    output_tokens: Some(usage.output_tokens),
                                    total_tokens: Some(input_tokens + usage.output_tokens),
                                }),
                            });
                        }
                        CompletionEvent::ContentBlockStop { .. } | CompletionEvent::Ping => {
                            return self.get_next();
                        }
                        _ => {
                            return completion_event.to_stream_event();
                        }
                    }
                }
                return llm::StreamEvent::Error(llm::Error {
                    code: llm::ErrorCode::InternalError,
                    message: "Event payload could not be parsed".to_owned(),
                    provider_error_json: None,
                });
            }
            return llm::StreamEvent::Error(llm::Error {
                code: llm::ErrorCode::InternalError,
                message: format!("Invalid event received from event stream >>> {}", payload),
                provider_error_json: None,
            });
        }
    }

    fn has_next(&self) -> bool {
        !self.has_finished_stream()
    }
}
