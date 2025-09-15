declare module 'golem:exec/executor@1.0.0' {
  import * as golemExec100Types from 'golem:exec/types@1.0.0';
  /**
   * Blocking, non-streaming execution
   * - `lang` specifies the programming language and version.
   * - `snippet` is the top level code to execute.
   * - `modules` are additional code files to include in the execution context. these can be imported in `snippet` in a language-specific way.
   * - `stdin` is optional input to provide to the program.
   * - `args` are command line arguments passed to the program.
   * - `env` is a list of environment variables to set for the execution.
   * - `constraints` are optional resource limits for the execution.
   * The returned value captures the stdout and stderr of the executed snippet.
   * @throws Error
   */
  export function run(lang: Language, snippet: string, modules: File[], stdin: string | undefined, args: string[], env: [string, string][], constraints: Limits | undefined): ExecResult;
  export class Session {
    /**
     * Create a new session for executing code snippets in the specified language, with a set of additional
     * code files that can be imported in the executed snippets.
     */
    constructor(lang: Language, modules: File[]);
    /**
     * Upload a data file to the session, which can be accessed in the executed snippets through standard file system APIs.
     * @throws Error
     */
    upload(file: File): void;
    /**
     * Execute a code snippet in the session in a blocking way
     * - `snippet` is the top level code to execute.
     * - `args` are command line arguments passed to the program.
     * - `stdin` is optional input to provide to the program.
     * - `env` is a list of environment variables to set for the execution.
     * - `constraints` are optional resource limits for the execution.
     * The returned value captures the stdout and stderr of the executed snippet.
     * @throws Error
     */
    run(snippet: string, args: string[], stdin: string | undefined, env: [string, string][], constraints: Limits | undefined): ExecResult;
    /**
     * Downloads a data file from the session.
     * @throws Error
     */
    download(path: string): Uint8Array;
    /**
     * Lists all the data files available in the session. These will include the ones that were `upload`ed and also
     * any other file created by the executed snippets.
     * @throws Error
     */
    listFiles(dir: string): string[];
    /**
     * Sets the current working directory within the session.
     * @throws Error
     */
    setWorkingDir(path: string): void;
  }
  export type Language = golemExec100Types.Language;
  export type File = golemExec100Types.File;
  export type Limits = golemExec100Types.Limits;
  export type ExecResult = golemExec100Types.ExecResult;
  export type Error = golemExec100Types.Error;
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
