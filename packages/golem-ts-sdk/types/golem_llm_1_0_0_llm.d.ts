declare module 'golem:llm/llm@1.0.0' {
  /**
   * --- Core Functions ---
   */
  export function send(messages: Message[], config: Config): ChatEvent;
  export function continue_(messages: Message[], toolResults: [ToolCall, ToolResult][], config: Config): ChatEvent;
  export function stream(messages: Message[], config: Config): ChatStream;
  export class ChatStream {
    getNext(): StreamEvent[] | undefined;
    blockingGetNext(): StreamEvent[];
  }
  /**
   * --- Roles, Error Codes, Finish Reasons ---
   */
  export type Role = "user" | "assistant" | "system" | "tool";
  export type ErrorCode = "invalid-request" | "authentication-failed" | "rate-limit-exceeded" | "internal-error" | "unsupported" | "unknown";
  export type FinishReason = "stop" | "length" | "tool-calls" | "content-filter" | "error" | "other";
  export type ImageDetail = "low" | "high" | "auto";
  /**
   * --- Message Content ---
   */
  export type ImageUrl = {
    url: string;
    detail: ImageDetail | undefined;
  };
  export type ImageSource = {
    data: Uint8Array;
    mimeType: string;
    detail: ImageDetail | undefined;
  };
  export type ImageReference = {
    tag: 'url'
    val: ImageUrl
  } |
  {
    tag: 'inline'
    val: ImageSource
  };
  export type ContentPart = {
    tag: 'text'
    val: string
  } |
  {
    tag: 'image'
    val: ImageReference
  };
  export type Message = {
    role: Role;
    name: string | undefined;
    content: ContentPart[];
  };
  /**
   * --- Tooling ---
   */
  export type ToolDefinition = {
    name: string;
    description: string | undefined;
    parametersSchema: string;
  };
  export type ToolCall = {
    id: string;
    name: string;
    argumentsJson: string;
  };
  export type ToolSuccess = {
    id: string;
    name: string;
    resultJson: string;
    executionTimeMs: number | undefined;
  };
  export type ToolFailure = {
    id: string;
    name: string;
    errorMessage: string;
    errorCode: string | undefined;
  };
  export type ToolResult = {
    tag: 'success'
    val: ToolSuccess
  } |
  {
    tag: 'error'
    val: ToolFailure
  };
  /**
   * --- Configuration ---
   */
  export type Kv = {
    key: string;
    value: string;
  };
  export type Config = {
    model: string;
    temperature: number | undefined;
    maxTokens: number | undefined;
    stopSequences: string[] | undefined;
    tools: ToolDefinition[];
    toolChoice: string | undefined;
    providerOptions: Kv[];
  };
  /**
   * --- Usage / Metadata ---
   */
  export type Usage = {
    inputTokens: number | undefined;
    outputTokens: number | undefined;
    totalTokens: number | undefined;
  };
  export type ResponseMetadata = {
    finishReason: FinishReason | undefined;
    usage: Usage | undefined;
    providerId: string | undefined;
    timestamp: string | undefined;
    providerMetadataJson: string | undefined;
  };
  export type CompleteResponse = {
    id: string;
    content: ContentPart[];
    toolCalls: ToolCall[];
    metadata: ResponseMetadata;
  };
  /**
   * --- Error Handling ---
   */
  export type Error = {
    code: ErrorCode;
    message: string;
    providerErrorJson: string | undefined;
  };
  /**
   * --- Chat Response Variants ---
   */
  export type ChatEvent = {
    tag: 'message'
    val: CompleteResponse
  } |
  {
    tag: 'tool-request'
    val: ToolCall[]
  } |
  {
    tag: 'error'
    val: Error
  };
  /**
   * --- Streaming ---
   */
  export type StreamDelta = {
    content: ContentPart[] | undefined;
    toolCalls: ToolCall[] | undefined;
  };
  export type StreamEvent = {
    tag: 'delta'
    val: StreamDelta
  } |
  {
    tag: 'finish'
    val: ResponseMetadata
  } |
  {
    tag: 'error'
    val: Error
  };
}
