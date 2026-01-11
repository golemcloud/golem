declare module 'golem:embed/embed@1.0.0' {
  /**
   * --- Core Functions ---
   * @throws Error
   */
  export function generate(inputs: ContentPart[], config: Config): EmbeddingResponse;
  /**
   * @throws Error
   */
  export function rerank(query: string, documents: string[], config: Config): RerankResponse;
  /**
   * --- Enums ---
   */
  export type TaskType = "retrieval-query" | "retrieval-document" | "semantic-similarity" | "classification" | "clustering" | "question-answering" | "fact-verification" | "code-retrieval";
  export type OutputFormat = "float-array" | "binary" | "base64";
  export type OutputDtype = "float-array" | "int8" | "uint8" | "binary" | "ubinary";
  export type ErrorCode = "invalid-request" | "model-not-found" | "unsupported" | "authentication-failed" | "provider-error" | "rate-limit-exceeded" | "internal-error" | "unknown";
  /**
   * --- Content ---
   */
  export type ImageUrl = {
    url: string;
  };
  export type ContentPart = 
  {
    tag: 'text'
    val: string
  } |
  {
    tag: 'image'
    val: ImageUrl
  };
  /**
   * --- Configuration ---
   */
  export type Kv = {
    key: string;
    value: string;
  };
  export type Config = {
    model?: string;
    taskType?: TaskType;
    dimensions?: number;
    truncation?: boolean;
    outputFormat?: OutputFormat;
    outputDtype?: OutputDtype;
    user?: string;
    providerOptions: Kv[];
  };
  /**
   * --- Embedding Response ---
   */
  export type Usage = {
    inputTokens?: number;
    totalTokens?: number;
  };
  /**
   * Supported encoding types by the provider
   * Cohere:       float-array, int8, uint8, binary, ubinary, base64.
   * VoyageAI:     float-array, int8, uint8, binary, ubinary, base64.
   * Hugging Face: float-array.
   * OpenAI :      float-array, base64.
   */
  export type VectorData = 
  {
    tag: 'float'
    val: number[]
  } |
  {
    tag: 'int8'
    val: number[]
  } |
  {
    tag: 'uint8'
    val: Uint8Array
  } |
  {
    tag: 'binary'
    val: number[]
  } |
  {
    tag: 'ubinary'
    val: Uint8Array
  } |
  {
    tag: 'base64'
    val: string
  };
  export type Embedding = {
    index: number;
    vector: VectorData;
  };
  export type EmbeddingResponse = {
    embeddings: Embedding[];
    usage?: Usage;
    model: string;
    providerMetadataJson?: string;
  };
  /**
   * --- Rerank Response ---
   */
  export type RerankResult = {
    index: number;
    relevanceScore: number;
    document?: string;
  };
  export type RerankResponse = {
    results: RerankResult[];
    usage?: Usage;
    model: string;
    providerMetadataJson?: string;
  };
  /**
   * --- Error Handling ---
   */
  export type Error = {
    code: ErrorCode;
    message: string;
    providerErrorJson?: string;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
