declare module 'golem:exec/types@1.0.0' {
  export type LanguageKind = "javascript" | "python";
  /**
   * Supported language types and optional version
   */
  export type Language = {
    kind: LanguageKind;
    version: string | undefined;
  };
  /**
   * Supported encodings for file contents
   */
  export type Encoding = "utf8" | "base64" | "hex";
  /**
   * Code or data file
   */
  export type File = {
    name: string;
    content: Uint8Array;
    encoding: Encoding | undefined;
  };
  /**
   * Resource limits and execution constraints
   */
  export type Limits = {
    timeMs: bigint | undefined;
    memoryBytes: bigint | undefined;
    fileSizeBytes: bigint | undefined;
    maxProcesses: number | undefined;
  };
  /**
   * Execution outcome per stage
   */
  export type StageResult = {
    stdout: string;
    stderr: string;
    exitCode: number | undefined;
    signal: string | undefined;
  };
  /**
   * Complete execution result
   */
  export type ExecResult = {
    compile: StageResult | undefined;
    run: StageResult;
    timeMs: bigint | undefined;
    memoryBytes: bigint | undefined;
  };
  /**
   * Execution error types
   */
  export type Error = {
    tag: 'unsupported-language'
  } |
  {
    tag: 'compilation-failed'
    val: StageResult
  } |
  {
    tag: 'runtime-failed'
    val: StageResult
  } |
  {
    tag: 'timeout'
  } |
  {
    tag: 'resource-exceeded'
  } |
  {
    tag: 'internal'
    val: string
  };
}
