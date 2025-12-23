declare module 'golem:exec/types@1.0.0' {
  /**
   * Supported languages
   */
  export type LanguageKind = "javascript" | "python";
  /**
   * Supported language types and optional version
   */
  export type Language = {
    /** The language to use */
    kind: LanguageKind;
    /** Optionally further narrow down the language version */
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
    /** File name */
    name: string;
    /** Raw file contents */
    content: Uint8Array;
    /** Encoding of `content`, defaults to `utf8` */
    encoding?: Encoding;
  };
  /**
   * Resource limits
   */
  export type Limits = {
    /** Limit the execution time, in milliseconds */
    timeMs?: bigint;
    /** Limit the memory usage, in bytes */
    memoryBytes?: bigint;
    /** Limit the maximum file size, in bytes */
    fileSizeBytes?: bigint;
    /** Limit the number of spawned processes */
    maxProcesses?: number;
  };
  /**
   * Execution outcome per stage
   */
  export type StageResult = {
    /** Standard output */
    stdout: string;
    /** Standard error output */
    stderr: string;
    /** Exit code of the process, if any */
    exitCode?: number;
    /** Signal that caused the process to terminate, if any */
    signal?: string;
  };
  /**
   * Complete execution result
   */
  export type ExecResult = {
    /** Result of the compilation stage, if any */
    compile?: StageResult;
    /** Result of the execution stage */
    run: StageResult;
    /** Execution time in milliseconds */
    timeMs?: bigint;
    /** Consumed memory in bytes */
    memoryBytes?: bigint;
  };
  /**
   * Execution error types
   */
  export type Error = 
  /** The chosen langauge is not supported */
  {
    tag: 'unsupported-language'
  } |
  /** Compilation failed */
  {
    tag: 'compilation-failed'
    val: StageResult
  } |
  /** Execution failed */
  {
    tag: 'runtime-failed'
    val: StageResult
  } |
  /** Timed out */
  {
    tag: 'timeout'
  } |
  /** Resource limits exceeded */
  {
    tag: 'resource-exceeded'
  } |
  /** Internal execution error */
  {
    tag: 'internal'
    val: string
  };
  /**
   * Options for controlling the script runner environment
   */
  export type RunOptions = {
    /** optional input to provide to the program. */
    stdin?: string;
    /** command line arguments passed to the program */
    args?: string[];
    /** a list of environment variables to set for the execution */
    env?: [string, string][];
    /** optional resource limits for the execution */
    limits?: Limits;
  };
}
