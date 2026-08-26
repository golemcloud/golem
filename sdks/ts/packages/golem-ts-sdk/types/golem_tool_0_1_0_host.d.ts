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
   * `definition.commands.nodes[0].name`.
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
   * Creates caller stdin endpoints. The writer and closure watcher remain
   * with the caller; the source is moved into one invocation.
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
   * Creates the output target and caller-readable stream before invocation.
   * Dropping the reader cancels only this attachment's consumer role.
   */
  export function createStdout(): [ToolStdout, AsyncIterable<ByteStreamItem>];
  /**
   * Waits for an explicit set of result observers as one causal batch. The
   * input order controls the result order; filesystem-capable bodies become
   * eligible together and execute in durable Start order.
   */
  export function getInvokeResults(futures: FutureInvokeResult[]): Promise<Result<InvocationResult, RpcError>[]>;
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
  export class ToolStdout {
  }
  export class ToolStdoutWriter {
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
  export class ToolRpc {
    constructor(toolName: string);
    /**
     * Waits for the structured terminal. Callers that supplied stdout must
     * drive this wait and the already-created reader concurrently.
     * @throws RpcError
     */
    invokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined, stdout: ToolStdout | undefined): Promise<InvocationResult>;
    /**
     * Durably admits fire-and-forget work. Declared output is discarded by
     * the host so an absent caller reader cannot apply backpressure.
     * @throws RpcError
     */
    invoke(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined): void;
    /**
     * Durably admits work and returns an independently owned structured
     * result observer. Stdout, when declared, was created by the caller and
     * is observed independently from this future.
     */
    asyncInvokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: ToolStdin | undefined, stdout: ToolStdout | undefined): FutureInvokeResult;
  }
  export class FutureInvokeResult {
    /**
     * Sequential calls return the same immutable terminal. Only one call may
     * be outstanding at a time.
     * @throws RpcError
     */
    get(): Promise<InvocationResult>;
    /**
     * Explicitly cancels the operation. Dropping this resource only detaches
     * result observation and does not cancel accepted work.
     */
    cancel(): void;
  }
  export type Tool = golemTool010Common.Tool;
  export type ToolError = golemTool010Common.ToolError;
  export type InvocationResult = golemTool010Common.InvocationResult;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type ComponentId = golemCore200Types.ComponentId;
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
    definition: Tool;
    implementedBy: ComponentId;
  };
  export type RpcError =
  {
    tag: 'protocol-error'
    val: string
  } |
  {
    tag: 'denied'
    val: string
  } |
  {
    tag: 'not-found'
    val: string
  } |
  {
    tag: 'remote-internal-error'
    val: string
  } |
  {
    tag: 'remote-tool-error'
    val: ToolError
  } |
  /** The operation's explicit cancellation won terminal arbitration. */
  {
    tag: 'cancelled'
  } |
  /**
   * A filesystem-capable input or output attachment exceeded the
   * configured per-direction retained-byte limit.
   */
  {
    tag: 'resource-exhausted'
    val: string
  };
  /**
   * Recoverable attachment failures are stream values rather than Component
   * Model stream errors. A producer emits one final failure item and then
   * closes the underlying stream. Clean EOF is represented only by closure.
   */
  export type ByteStreamFailure =
  {
    tag: 'cancelled'
  } |
  {
    tag: 'abandoned'
  } |
  {
    tag: 'resource-exhausted'
  } |
  {
    tag: 'failed'
    val: string
  };
  /**
   * Every successful item contains a non-empty byte chunk.
   */
  export type ByteStreamItem = Result<Uint8Array, ByteStreamFailure>;
  export type ByteStreamCloseCause =
  {
    tag: 'finished'
  } |
  {
    tag: 'failed'
    val: ByteStreamFailure
  } |
  {
    tag: 'consumer-cancelled'
  };
  export type StreamWriteError =
  {
    tag: 'closed'
    val: ByteStreamCloseCause
  } |
  {
    tag: 'concurrent-operation'
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
