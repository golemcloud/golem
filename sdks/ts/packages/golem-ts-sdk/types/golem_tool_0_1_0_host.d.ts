/**
 * Interface the runtime exposes to agents and tools for discovering and
 * invoking ambient tools — tools registered by other components in the
 * same Golem environment.
 * Mirrors the structure of `golem:agent/host`, but keyed on tool name
 * (rather than agent-id), and without the agent-instance constructor
 * step (tools are stateless invocables).
 */
declare module 'golem:tool/host@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemTool010Common from 'golem:tool/common@0.1.0';
  import * as golemTool010Streams from 'golem:tool/streams@0.1.0';
  /**
   * Returns every tool **the calling agent has access to** in
   * the current environment, per the manifest's per-env and
   * per-agent binding rules (§5.6 / §6.4.3). The set returned
   * is exactly the set the calling agent could `tool-rpc.invoke`
   * against; tools the calling agent has no binding for are
   * excluded. Mirrors the *function shape* of
   * `golem:agent/host`'s `get-all-agent-types`, with the
   * addition of per-caller access filtering. Order is
   * unspecified; callers that
   * want a stable ordering should sort by
   * `lookup-name`.
   */
  export function getAllTools(): RegisteredTool[];
  /**
   * Returns the registered tool with the given name iff the
   * calling agent has access to it (per the same per-env and
   * per-agent binding rules as `get-all-tools`). Returns `none`
   * either if the tool is not registered or if the calling agent
   * has no binding for it; the two cases are not distinguished.
   */
  export function getTool(name: string): RegisteredTool | undefined;
  /**
   * Discovers a currently bound tool and uses the canonical CLI parser.
   * `none` means the name is unavailable. Variables supply only declared
   * environment defaults; they are not forwarded as a process environment.
   * Discovery uses its ordinary durable read; parsing itself has no effects.
   */
  export function prepareCommand(name: string, args: string[], variables: [string, string][]): CommandPreparation | undefined;
  /**
   * Creates caller stdin endpoints. The writer and closure watcher remain
   * with the caller; the source is moved into one invocation. Before passing
   * the source to `invoke-and-await`, either select a writer terminal or drive
   * the writer concurrently with the invocation. An open source is valid
   * while a concurrent producer can make progress; writing only after the
   * synchronous invocation returns can deadlock. SDK convenience adapters
   * must start their producer pump before awaiting the invocation terminal.
   */
  export function createStdin(): [ToolStdinWriter, ToolStdin, ToolStdinClosed];
  /**
   * Transfers an SDK-native input stream across the component boundary and
   * pumps it into a directional stdin attachment. This is equivalent to
   * `create-stdin` plus a caller-owned pump, but also supports non-numeric
   * stream items that cannot rendezvous within one Component Model task.
   */
  export function createStdinFromStream(source: AsyncIterable<ByteStreamItem>): ToolStdin;
  /**
   * Creates an immutable, host-owned stdin source. The complete input is
   * admitted to host memory before this returns. The source requires no
   * caller-side pump and can exceed the streaming attachment buffer size.
   * Admission failure creates no endpoint. Replay preserves that decision.
   * @throws ToolRpcError
   */
  export function createStdinFromBytes(bytes: Uint8Array): ToolStdin;
  /**
   * Creates the output target and caller-readable stream before invocation.
   * Dropping the reader cancels only this attachment's consumer role.
   */
  export function createOutput(): [ToolOutput, AsyncIterable<ByteStreamItem>];
  /**
   * Reserves the retained prefix before dispatch. The limit is capped by the
   * host's attachment byte limit. Admission failure creates no endpoints.
   * @throws ToolRpcError
   */
  export function createCollectedOutput(maxBytes: bigint): [ToolOutput, OutputCollection];
  /**
   * Waits for an explicit set of result observers as one causal batch. The
   * input order controls the result order; filesystem-capable bodies become
   * eligible together and execute in durable Start order.
   */
  export function getInvokeResults(futures: FutureInvokeResult[]): Promise<Result<InvocationResult, ToolRpcError>[]>;
  export class PreparedCommand {
    input(): CommandInput;
    /**
     * Projects a terminal using named ProcessExitStatus, declared error
     * statuses and redacted result text. Append this output after the
     * collected streams. Preserve any prior stream failure or cancellation.
     */
    renderResult(terminal: Result<InvocationResult, ToolRpcError>): CommandOutput;
  }
  export class ToolStdinWriter {
    /**
     * @throws StreamWriteError
     */
    write(bytes: Uint8Array): Promise<void>;
    /**
     * @throws StreamWriteError
     */
    finish(): Promise<void>;
    /**
     * @throws StreamWriteError
     */
    fail(reason: ByteStreamFailure): Promise<void>;
  }
  export class ToolStdin {
  }
  export class ToolStdinClosed {
    wait(): Promise<ByteStreamCloseCause>;
  }
  export class ToolOutput {
  }
  export class OutputCollection {
  }
  export class ToolRpc {
    /**
     * Binds an ordinary tool client to its configured tool name.
     */
    constructor(toolName: string);
    /**
     * Creates a tool RPC resource without trapping on invalid caller input.
     * @throws ToolRpcError
     */
    static create(toolName: string): ToolRpc;
    /**
     * Waits for the structured terminal. Callers that supplied stdout or
     * stderr must drive this wait and every already-created reader
     * concurrently. Callers that manually created an open stdin must likewise
     * drive its writer concurrently; see `create-stdin`.
     * @throws ToolRpcError
     */
    invokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined, stdout: ToolOutput | undefined, stderr: ToolOutput | undefined): Promise<InvocationResult>;
    /**
     * Durably admits fire-and-forget work. Declared output is discarded by
     * the host so an absent caller reader cannot apply backpressure.
     * @throws ToolRpcError
     */
    invoke(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined): void;
    /**
     * Durably admits work and returns an independently owned structured
     * result observer. Each requested output was created by the caller and is
     * observed independently from this future.
     */
    asyncInvokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined, stdout: ToolOutput | undefined, stderr: ToolOutput | undefined): FutureInvokeResult;
  }
  export class FutureInvokeResult {
    /**
     * Sequential calls return the same immutable terminal. Only one call may
     * be outstanding at a time.
     * @throws ToolRpcError
     */
    get(): Promise<InvocationResult>;
    /**
     * Waits for the terminal while the host drains both supplied streams.
     * This does not require concurrent progress in the calling guest. Pass
     * collection handles created before async-invoke-and-await. Each channel
     * retains at most the limit reserved at creation. Exceeding that limit
     * cancels the invocation and reports overflow on that channel.
     * Dropping the wait does not itself cancel the invocation.
     */
    collect(stdout: OutputCollection | undefined, stderr: OutputCollection | undefined): CollectedInvocation;
    /**
     * The same collection with an asynchronous guest boundary, so the
     * calling runtime can continue to drive timers and explicit cancellation.
     */
    collectAsync(stdout: OutputCollection | undefined, stderr: OutputCollection | undefined): Promise<CollectedInvocation>;
    /**
     * Explicitly cancels the operation. Dropping this resource only detaches
     * result observation and does not cancel accepted work.
     */
    cancel(): void;
  }
  export type Tool = golemTool010Common.Tool;
  export type InvocationResult = golemTool010Common.InvocationResult;
  export type ByteStreamFailure = golemTool010Streams.ByteStreamFailure;
  export type ByteStreamItem = golemTool010Streams.ByteStreamItem;
  export type ByteStreamCloseCause = golemTool010Streams.ByteStreamCloseCause;
  export type StreamWriteError = golemTool010Streams.StreamWriteError;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type ComponentId = golemCore200Types.ComponentId;
  export type ToolRpcError = golemCore200Types.ToolRpcError;
  /**
   * A tool registered in the environment, addressable by name from
   * any agent or other tool. `definition` carries the full metadata;
   * `implemented-by` identifies the component that registers the
   * tool with the runtime — a Golem component exporting
   * `golem:tool/guest` for native tools, the runtime-internal
   * MCP-import bridge component for tools projected from
   * `mcp.imports` (§5.7.2), or the runtime itself (a synthesized
   * component-id) for host-implemented privileged tools (§4.6).
   */
  export type RegisteredTool = {
    /** Stable leaf name used to construct tool-rpc, independent of adapters. */
    lookupName: string;
    definition: Tool;
    implementedBy: ComponentId;
  };
  export type CommandInput = {
    commandPath: string[];
    input: TypedSchemaValue;
    stdin: boolean;
    stdout: boolean;
    stderr: boolean;
  };
  export type CommandOutput = {
    exitCode: number;
    stdout: Uint8Array;
    stderr: Uint8Array;
  };
  export type CommandPreparation =
  /** Help or a CLI diagnostic; no tool invocation is needed. */
  {
    tag: 'output'
    val: CommandOutput
  } |
  {
    tag: 'invoke'
    val: PreparedCommand
  };
  /**
   * Bounded bytes collected from one output channel. A stream failure is
   * separate from the tool's structured result. Overflow retains the prefix.
   */
  export type CollectedOutput = {
    bytes: Uint8Array;
    failure?: ByteStreamFailure;
    overflow: boolean;
  };
  export type CollectedInvocation = {
    result: Result<InvocationResult, ToolRpcError>;
    /**
     * True when the invocation's recorded terminal says its body executed.
     * A tool error alone does not distinguish refused admission from a
     * failure after execution. Middleware counts as part of the invocation.
     */
    bodyExecuted: boolean;
    stdout?: CollectedOutput;
    stderr?: CollectedOutput;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
