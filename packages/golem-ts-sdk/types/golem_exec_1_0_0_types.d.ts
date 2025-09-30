declare module 'golem:exec/types@1.0.0' {
  export type LanguageKind = "javascript" | "python";
  /**
   * Supported language types and optional version
   */
  export type Language = {
    kind: LanguageKind;
    version?: string;
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
    encoding?: Encoding;
  };
  /**
   * Resource limits
   */
  export type Limits = {
    timeMs?: bigint;
    memoryBytes?: bigint;
    fileSizeBytes?: bigint;
    maxProcesses?: number;
  };
  /**
   * Execution outcome per stage
   */
  export type StageResult = {
    stdout: string;
    stderr: string;
    exitCode?: number;
    signal?: string;
  };
  /**
   * Complete execution result
   */
  export type ExecResult = {
    compile?: StageResult;
    run: StageResult;
    timeMs?: bigint;
    memoryBytes?: bigint;
  };
  /**
   * Execution error types
   */
  export type Error = 
  {
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
  /**
   * Options for controlling the script runner environment
   * - `stdin` is optional input to provide to the program.
   * - `args` are command line arguments passed to the program.
   * - `env` is a list of environment variables to set for the execution.
   * - `limits` are optional resource limits for the execution.
   */
  export type RunOptions = {
    stdin?: string;
    args?: string[];
    env?: [string, string][];
    limits?: Limits;
  };
}
