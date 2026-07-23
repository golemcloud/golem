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
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
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
  export class ToolRpc {
    constructor(toolName: string);
    /**
     * @throws RpcError
     */
    invokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined): InvocationResult;
    /**
     * @throws RpcError
     */
    invoke(commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined): void;
    asyncInvokeAndAwait(commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined): FutureInvokeResult;
  }
  export class FutureInvokeResult {
    subscribe(): Pollable;
    get(): Result<InvocationResult, RpcError> | undefined;
    cancel(): void;
  }
  export type Tool = golemTool010Common.Tool;
  export type ToolError = golemTool010Common.ToolError;
  export type InvocationResult = golemTool010Common.InvocationResult;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type ComponentId = golemCore200Types.ComponentId;
  export type InputStream = wasiIo023Streams.InputStream;
  export type Pollable = wasiIo023Poll.Pollable;
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
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
