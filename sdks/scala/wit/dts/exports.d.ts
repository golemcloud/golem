declare module 'agent-guest' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemApi150Host from 'golem:api/host@1.5.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
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
  export namespace guest {
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
}
