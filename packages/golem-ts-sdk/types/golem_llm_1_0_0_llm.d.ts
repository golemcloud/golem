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
    detail?: ImageDetail;
  };
  export type ImageSource = {
    data: Uint8Array;
    mimeType: string;
    detail?: ImageDetail;
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
    name?: string;
    content: ContentPart[];
  };
  /**
   * --- Tooling ---
   */
  export type ToolDefinition = {
    name: string;
    description?: string;
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
    executionTimeMs?: number;
  };
  export type ToolFailure = {
    id: string;
    name: string;
    errorMessage: string;
    errorCode?: string;
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
    temperature?: number;
    maxTokens?: number;
    stopSequences?: string[];
    tools: ToolDefinition[];
    toolChoice?: string;
    providerOptions: Kv[];
  };
  /**
   * --- Usage / Metadata ---
   */
  export type Usage = {
    inputTokens?: number;
    outputTokens?: number;
    totalTokens?: number;
  };
  export type ResponseMetadata = {
    finishReason?: FinishReason;
    usage?: Usage;
    providerId?: string;
    timestamp?: string;
    providerMetadataJson?: string;
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
    providerErrorJson?: string;
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
    content?: ContentPart[];
    toolCalls?: ToolCall[];
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
