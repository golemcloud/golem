declare module 'agent-guest' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemApi150Host from 'golem:api/host@1.5.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemTool010Common from 'golem:tool/common@0.1.0';
  import * as wasiIo023Streams from 'wasi:io/streams@0.2.3';
  /**
   * Interface exported by a component that provides tools. The component
   * declares which tools it exposes, supplies their metadata, and accepts
   * invocations against a chosen leaf command of any tool.
   * Tools are stateless from the host's perspective: each invocation is
   * independent. State accumulated by the underlying agent (file-system
   * writes, config-store updates, etc.) persists per the agent's normal
   * rules and is independent of the tool calling convention.
   */
  export namespace golemTool010Guest {
    /**
     * Enumerate the tools this component exposes. The returned metadata
     * is complete (full command tree and schema graph).
     * @throws ToolError
     */
    export function discoverTools(): Promise<Tool[]>;
    /**
     * Look up a single tool by name.
     * @throws ToolError
     */
    export function getTool(name: string): Promise<Tool>;
    /**
     * Invoke a command of a tool.
     * `command-path` selects the command body to execute. An empty list
     * targets the root command's body; non-empty lists walk the
     * `subcommands` field, each segment matching a `command-node.name`
     * or alias.
     * `input` is a self-contained `typed-schema-value` whose root must
     * structurally match the selected body's input schema — a record with
     * one field per positional, option, flag, and inherited global declared
     * on or above the body, each field typed by the matching type node in
     * the body's schema.
     * `stdin` is supplied when the selected body declared a stdin
     * `stream-spec`. Stream ownership: the `stdin` resource handle is moved
     * into the callee for the duration of the call.
     * `principal` carries the caller's authenticated identity for
     * authorization and audit, identical in semantics to the parameter
     * of the same name in `golem:agent/guest.invoke`.
     * @throws ToolError
     */
    export function invokeTool(toolName: string, commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined, principal: Principal): Promise<InvocationResult>;
    export type Tool = golemTool010Common.Tool;
    export type ToolError = golemTool010Common.ToolError;
    export type InvocationResult = golemTool010Common.InvocationResult;
    export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
    export type Principal = golemAgent200Common.Principal;
    export type InputStream = wasiIo023Streams.InputStream;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
  /**
   * Interface providing user-defined snapshotting capability. This can be used to perform manual update of agents
   * when the new component incompatible with the old one.
   */
  export namespace saveSnapshot {
    /**
     * Saves the component's state into a user-defined snapshot
     */
    export function save(): Promise<Snapshot>;
    export type Snapshot = golemApi150Host.Snapshot;
  }
  /**
   * Interface providing user-defined snapshotting capability. This can be used to perform manual update of agents
   * when the new component incompatible with the old one.
   */
  export namespace loadSnapshot {
    /**
     * Tries to load a user-defined snapshot, setting up the agent's state based on it.
     * The function can return with a failure to indicate that the update is not possible.
     * @throws string
     */
    export function load(snapshot: Snapshot): Promise<void>;
    export type Snapshot = golemApi150Host.Snapshot;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
  export namespace golemAgent200Guest {
    /**
     * Initializes the agent of a given type with the given constructor parameters.
     * If called a second time, it fails.
     * `input` is a value tree whose root encodes the constructor's parameter
     * list (one record field per declared `named-field`, in declaration order).
     * The guest interprets it against its own constructor `input-schema`.
     * @throws AgentError
     */
    export function initialize(agentType: string, input: SchemaValueTree, principal: Principal): Promise<void>;
    /**
     * Invokes an agent. If create was not called before, it fails.
     * `input` is a value tree whose root encodes the method's parameter list.
     * The result is `none` when the method's `output-schema` is `unit`, and
     * `some(value)` for a `single` output.
     * @throws AgentError
     */
    export function invoke(methodName: string, input: SchemaValueTree, principal: Principal): Promise<SchemaValueTree | undefined>;
    /**
     * Gets the agent type. If create was not called before, it fails
     */
    export function getDefinition(): Promise<AgentType>;
    /**
     * Gets the agent types defined by this component
     * @throws AgentError
     */
    export function discoverAgentTypes(): Promise<AgentType[]>;
    export type SchemaValueTree = golemCore200Types.SchemaValueTree;
    export type AgentError = golemAgent200Common.AgentError;
    export type AgentType = golemAgent200Common.AgentType;
    export type Principal = golemAgent200Common.Principal;
    export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
  }
  export namespace guest {
    export function initialize(agentType: string, input: SchemaValueTree, principal: Principal): Promise<void>;
    export function invoke(methodName: string, input: SchemaValueTree, principal: Principal): Promise<SchemaValueTree | undefined>;
    export function getDefinition(): Promise<AgentType>;
    export function discoverAgentTypes(): Promise<AgentType[]>;
    export function discoverTools(): Promise<Tool[]>;
    export function getTool(name: string): Promise<Tool>;
    export function invokeTool(toolName: string, commandPath: string[], input: TypedSchemaValue, stdin: InputStream | undefined, principal: Principal): Promise<InvocationResult>;
    export type SchemaValueTree = golemAgent200Guest.SchemaValueTree;
    export type AgentError = golemAgent200Guest.AgentError;
    export type AgentType = golemAgent200Guest.AgentType;
    export type Principal = golemAgent200Guest.Principal;
    export type Tool = golemTool010Guest.Tool;
    export type ToolError = golemTool010Guest.ToolError;
    export type InvocationResult = golemTool010Guest.InvocationResult;
    export type TypedSchemaValue = golemTool010Guest.TypedSchemaValue;
    export type InputStream = golemTool010Guest.InputStream;
  }
}
