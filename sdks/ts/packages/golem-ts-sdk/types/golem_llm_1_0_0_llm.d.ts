declare module 'golem:llm/llm@1.0.0' {
  /**
   * --- Core Functions ---
   * Make a single call to the LLM.
   * To continue the conversation:
   * - append tool responses and new messages to the events and use send again
   * - or use the chat-session wrapper, which help in maintaining the chat events
   * @throws Error
   */
  export function send(events: Event[], config: Config): Response;
  /**
   * Makes a single call to the LLM and gets back a streaming API to receive the response in chunks.
   */
  export function stream(events: Event[], config: Config): ChatStream;
  export class ChatStream {
    /**
     * Polls for the next chunk of stream events
     */
    pollNext(): Result<StreamEvent, Error>[] | undefined;
    /**
     * Blocks until the next chunk of stream events is available
     */
    getNext(): Result<StreamEvent, Error>[];
  }
  /**
   * --- Roles, Error Codes, Finish Reasons ---
   * Roles of the conversation
   */
  export type Role = "user" | "assistant" | "system" | "tool";
  /**
   * Possible error cases for LLM calls
   */
  export type ErrorCode = "invalid-request" | "authentication-failed" | "rate-limit-exceeded" | "internal-error" | "unsupported" | "unknown";
  /**
   * Reasons for finishing a conversation
   */
  export type FinishReason = "stop" | "length" | "tool-calls" | "content-filter" | "error" | "other";
  /**
   * Image detail levels
   */
  export type ImageDetail = "low" | "high" | "auto";
  /**
   * --- Message Content ---
   * Points to an image by an URL and an optional image detail level
   */
  export type ImageUrl = {
    /** The URL of the image */
    url: string;
    /** Level of detail of the image */
    detail?: ImageDetail;
  };
  /**
   * Contains an inline image
   */
  export type ImageSource = {
    /** Raw image data */
    data: Uint8Array;
    /** MIME type of the image */
    mimeType: string;
    /** Level of detail of the image */
    detail?: ImageDetail;
  };
  /**
   * Contains an image, either a remote or an inlined one
   */
  export type ImageReference = 
  /** A remote image */
  {
    tag: 'url'
    val: ImageUrl
  } |
  /** An inlined image */
  {
    tag: 'inline'
    val: ImageSource
  };
  /**
   * One part of the conversation
   */
  export type ContentPart = 
  /** Text content */
  {
    tag: 'text'
    val: string
  } |
  /** Image content */
  {
    tag: 'image'
    val: ImageReference
  };
  /**
   * A message in the conversation
   */
  export type Message = {
    /** Role of this message */
    role: Role;
    /** Name of the sender */
    name?: string;
    /** Content of the message */
    content: ContentPart[];
  };
  /**
   * --- Tooling ---
   * Describes a tool callable by the LLM
   */
  export type ToolDefinition = {
    /** Name of the tool */
    name: string;
    /** Description of the tool */
    description?: string;
    /** Schema of the tool's parameters - usually a JSON schema */
    parametersSchema: string;
  };
  /**
   * Describes a tool call request
   */
  export type ToolCall = {
    /** Call identifier */
    id: string;
    /** Name of the tool */
    name: string;
    /** Arguments of the tool call */
    argumentsJson: string;
  };
  /**
   * Describes a successful tool call
   */
  export type ToolSuccess = {
    /** Call identifier */
    id: string;
    /** Name of the tool */
    name: string;
    /** Result of the tool call in JSON */
    resultJson: string;
    /** Execution time of the tool call in milliseconds */
    executionTimeMs?: number;
  };
  /**
   * Describes a failed tool call
   */
  export type ToolFailure = {
    /** Call identifier */
    id: string;
    /** Name of the tool */
    name: string;
    /** Error message of the tool call */
    errorMessage: string;
    /** Error code of the tool call */
    errorCode?: string;
  };
  /**
   * Result of a tool call
   */
  export type ToolResult = 
  /** The tool call succeeded */
  {
    tag: 'success'
    val: ToolSuccess
  } |
  /** The tool call failed */
  {
    tag: 'error'
    val: ToolFailure
  };
  /**
   * --- Configuration ---
   * Simple key-value pair
   */
  export type Kv = {
    key: string;
    value: string;
  };
  /**
   * LLM configuration
   */
  export type Config = {
    /** The model to use */
    model: string;
    /** Temperature */
    temperature?: number;
    /** Maximum number of tokens */
    maxTokens?: number;
    /** A sequence where the model stops generating tokens */
    stopSequences?: string[];
    /** List of available tools */
    tools?: ToolDefinition[];
    /** Tool choice policy */
    toolChoice?: string;
    /** Additional LLM provider specific key-value pairs */
    providerOptions?: Kv[];
  };
  /**
   * --- Usage / Metadata ---
   * Token usage statistics
   */
  export type Usage = {
    /** Number of input tokens used */
    inputTokens?: number;
    /** Number of output tokens generated */
    outputTokens?: number;
    /** Total number of tokens used */
    totalTokens?: number;
  };
  /**
   * Metadata about an LLM response
   */
  export type ResponseMetadata = {
    /** Reason for finishing the conversation */
    finishReason?: FinishReason;
    /** Usage statistics */
    usage?: Usage;
    /** Provider-specific ID */
    providerId?: string;
    /** Timestamp */
    timestamp?: string;
    /** Provider-specific additional metadata in JSON */
    providerMetadataJson?: string;
  };
  /**
   * --- Error Handling ---
   * LLM error
   */
  export type Error = {
    /** Error code */
    code: ErrorCode;
    /** Error message */
    message: string;
    /** More details in JSON, in a provider-specific format */
    providerErrorJson?: string;
  };
  /**
   * --- Chat Response ---
   * Response from an LLM
   */
  export type Response = {
    /** Response ID */
    id: string;
    /** Result contents */
    content: ContentPart[];
    /** Tool call requests */
    toolCalls: ToolCall[];
    /** Response metadata */
    metadata: ResponseMetadata;
  };
  /**
   * --- Chat event  ---
   * Chat events that can happen during a chat session
   */
  export type Event = 
  /** Message asked by the user */
  {
    tag: 'message'
    val: Message
  } |
  /** Response from the LLM */
  {
    tag: 'response'
    val: Response
  } |
  /** Provided tool results */
  {
    tag: 'tool-results'
    val: ToolResult[]
  };
  /**
   * --- Streaming ---
   * Changes in a streaming conversation
   */
  export type StreamDelta = {
    /** New content parts */
    content?: ContentPart[];
    /** New tool calls */
    toolCalls?: ToolCall[];
  };
  /**
   * Event in a streaming conversation
   */
  export type StreamEvent = 
  /** New incoming response content or tool call requests */
  {
    tag: 'delta'
    val: StreamDelta
  } |
  /** Converstation finished */
  {
    tag: 'finish'
    val: ResponseMetadata
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
