declare module 'golem:rpc/types@0.2.2' {
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
  /**
   * Parses a UUID from a string
   * @throws string
   */
  export function parseUuid(uuid: string): Uuid;
  /**
   * Converts a UUID to a string
   */
  export function uuidToString(uuid: Uuid): string;
  export class WasmRpc {
    /**
     * Constructs the RPC client connecting to the given target agent
     */
    constructor(agentId: AgentId);
    /**
     * Invokes a remote function with the given parameters, and awaits the result
     * @throws RpcError
     */
    invokeAndAwait(functionName: string, functionParams: WitValue[]): WitValue;
    /**
     * Triggers the invocation of a remote function with the given parameters, and returns immediately.
     * @throws RpcError
     */
    invoke(functionName: string, functionParams: WitValue[]): void;
    /**
     * Invokes a remote function with the given parameters, and returns a `future-invoke-result` value which can
     * be polled for the result.
     * With this function it is possible to call multiple (different) agents simultaneously.
     */
    asyncInvokeAndAwait(functionName: string, functionParams: WitValue[]): FutureInvokeResult;
    /**
     * Schedule invocation for later
     */
    scheduleInvocation(scheduledTime: Datetime, functionName: string, functionParams: WitValue[]): void;
    /**
     * Schedule invocation for later. Call cancel on the returned resource to cancel the invocation before the scheduled time.
     */
    scheduleCancelableInvocation(scheduledTime: Datetime, functionName: string, functionParams: WitValue[]): CancellationToken;
  }
  export class FutureInvokeResult {
    /**
     * Subscribes to the result of the invocation
     */
    subscribe(): Pollable;
    /**
     * Poll for the invocation. If the invocation has not completed yet, returns `none`.
     */
    get(): Result<WitValue, RpcError> | undefined;
  }
  export class CancellationToken {
    /**
     * Cancel the scheduled invocation
     */
    cancel(): void;
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
  /**
   * An index into the persistent log storing all performed operations of an agent
   * FIXME: move into golem:api/host
   */
  export type OplogIndex = bigint;
  /**
   * UUID
   */
  export type Uuid = {
    highBits: bigint;
    lowBits: bigint;
  };
  /**
   * Represents a Golem component
   */
  export type ComponentId = {
    uuid: Uuid;
  };
  /**
   * Represents a Golem agent
   */
  export type AgentId = {
    /** Identifies the component the agent belongs to */
    componentId: ComponentId;
    /** String representation of the agent ID (agent type and constructor parameters) */
    agentId: string;
  };
  /**
   * Represents a Golem account
   * FIXME: move into golem:api/host
   */
  export type AccountId = {
    uuid: Uuid;
  };
  /**
   * A promise ID is a value that can be passed to an external Golem API to complete that promise
   * from an arbitrary external source, while Golem agents can await for this completion.
   * FIXME: move into golem:api/host
   */
  export type PromiseId = {
    agentId: AgentId;
    oplogIdx: OplogIndex;
  };
  /**
   * The index type used in `wit-value` and `wit-type` to identify nodes
   */
  export type NodeIndex = number;
  /**
   * Represents a WIT resource in an instance
   */
  export type ResourceId = bigint;
  /**
   * Resource handle modes
   */
  export type ResourceMode = "owned" | "borrowed";
  /**
   * Represents a type within a `wit-type` definition. `node-index` values are indices into the
   * parent `wit-type`'s `nodes` list.
   */
  export type WitTypeNode = 
  /** Record type, defined by a list of name-type pairs. */
  {
    tag: 'record-type'
    val: [string, NodeIndex][]
  } |
  /**
   * Variant type, defined by a list of name-type pairs. The type is optional, in case it is not defined, the case
   * is a unit case.
   */
  {
    tag: 'variant-type'
    val: [string, NodeIndex | undefined][]
  } |
  /** Enum type, defined by a list of its cases. */
  {
    tag: 'enum-type'
    val: string[]
  } |
  /** Flags type, defined by a list of its flags. */
  {
    tag: 'flags-type'
    val: string[]
  } |
  /** Tuple type, defined by a list of its field's types. */
  {
    tag: 'tuple-type'
    val: NodeIndex[]
  } |
  /** List type, defined by the element type */
  {
    tag: 'list-type'
    val: NodeIndex
  } |
  /** Option type, defined by the element type */
  {
    tag: 'option-type'
    val: NodeIndex
  } |
  /**
   * Result type, defined by the success and error types. Both types are optional, in case they are not defined, the
   * ok or error case is a unit case.
   */
  {
    tag: 'result-type'
    val: [NodeIndex | undefined, NodeIndex | undefined]
  } |
  /** Unsigned 8-bit integer */
  {
    tag: 'prim-u8-type'
  } |
  /** Unsigned 16-bit integer */
  {
    tag: 'prim-u16-type'
  } |
  /** Unsigned 32-bit integer */
  {
    tag: 'prim-u32-type'
  } |
  /** Unsigned 64-bit integer */
  {
    tag: 'prim-u64-type'
  } |
  /** Signed 8-bit integer */
  {
    tag: 'prim-s8-type'
  } |
  /** Signed 16-bit integer */
  {
    tag: 'prim-s16-type'
  } |
  /** Signed 32-bit integer */
  {
    tag: 'prim-s32-type'
  } |
  /** Signed 64-bit integer */
  {
    tag: 'prim-s64-type'
  } |
  /** 32-bit floating point number */
  {
    tag: 'prim-f32-type'
  } |
  /** 64-bit floating point number */
  {
    tag: 'prim-f64-type'
  } |
  /** Unicode character */
  {
    tag: 'prim-char-type'
  } |
  /** Boolean value */
  {
    tag: 'prim-bool-type'
  } |
  /** String value */
  {
    tag: 'prim-string-type'
  } |
  /** Handle type, defined by a resource ID and borrowing mode */
  {
    tag: 'handle-type'
    val: [ResourceId, ResourceMode]
  };
  /**
   * Represents a node of `wit-type`, with attached metadata
   */
  export type NamedWitTypeNode = {
    /** Name of the type */
    name?: string;
    /** Owner of the type (usually pointing to a WIT package and interface) */
    owner?: string;
    /** The node representing a type */
    type: WitTypeNode;
  };
  /**
   * Describes a type of a `wit-value`
   */
  export type WitType = {
    /**
     * The nodes consisting of the type definition. Because WIT does not support recursive types, the nodes are represented as a list of named nodes.
     * The list is always non-empty, and the first item is the root node.
     */
    nodes: NamedWitTypeNode[];
  };
  /**
   * URI value
   */
  export type Uri = {
    value: string;
  };
  /**
   * One node of a `wit-value`
   */
  export type WitNode = 
  /** A record value defined by a list of its field values */
  {
    tag: 'record-value'
    val: NodeIndex[]
  } |
  /** A variant value defined by a pair of the case index and its inner value */
  {
    tag: 'variant-value'
    val: [number, NodeIndex | undefined]
  } |
  /** An enum value defined by a case index */
  {
    tag: 'enum-value'
    val: number
  } |
  /** A flags value defined by a list of its flag states */
  {
    tag: 'flags-value'
    val: boolean[]
  } |
  /** A tuple value defined by a list of its item values */
  {
    tag: 'tuple-value'
    val: NodeIndex[]
  } |
  /** A list value defined by a list of its item values */
  {
    tag: 'list-value'
    val: NodeIndex[]
  } |
  /** An option value defined by an optional inner value */
  {
    tag: 'option-value'
    val: NodeIndex | undefined
  } |
  /**
   * A result value defined by either an ok value or an error value. Both values are optional,
   * where the `none` case represents the absence of a value.
   */
  {
    tag: 'result-value'
    val: Result<NodeIndex | undefined, NodeIndex | undefined>
  } |
  /** Primitive unsigned 8-bit integer */
  {
    tag: 'prim-u8'
    val: number
  } |
  /** Primitive unsigned 16-bit integer */
  {
    tag: 'prim-u16'
    val: number
  } |
  /** Primitive unsigned 32-bit integer */
  {
    tag: 'prim-u32'
    val: number
  } |
  /** Primitive unsigned 64-bit integer */
  {
    tag: 'prim-u64'
    val: bigint
  } |
  /** Primitive signed 8-bit integer */
  {
    tag: 'prim-s8'
    val: number
  } |
  /** Primitive signed 16-bit integer */
  {
    tag: 'prim-s16'
    val: number
  } |
  /** Primitive signed 32-bit integer */
  {
    tag: 'prim-s32'
    val: number
  } |
  /** Primitive signed 64-bit integer */
  {
    tag: 'prim-s64'
    val: bigint
  } |
  /** Primitive 32-bit floating point number */
  {
    tag: 'prim-float32'
    val: number
  } |
  /** Primitive 64-bit floating point number */
  {
    tag: 'prim-float64'
    val: number
  } |
  /** Primitive character */
  {
    tag: 'prim-char'
    val: string
  } |
  /** Primitive boolean */
  {
    tag: 'prim-bool'
    val: boolean
  } |
  /** Primitive string */
  {
    tag: 'prim-string'
    val: string
  } |
  /** Resource handle pointing to a URI and a resource ID */
  {
    tag: 'handle'
    val: [Uri, bigint]
  };
  /**
   * Describes an arbitrary value
   */
  export type WitValue = {
    /**
     * The list of `wit-node` values that make up the value. The list is always non-empty,
     * and the first element is the root node describing the value. Because WIT does not support
     * recursive types, further nodes are pushed into this list, and referenced by index from their parent node.
     */
    nodes: WitNode[];
  };
  /**
   * A value and its type
   */
  export type ValueAndType = {
    /** Value */
    value: WitValue;
    /** Type of `value` */
    typ: WitType;
  };
  /**
   * Possible failures of an RPC call
   */
  export type RpcError = 
  /** Protocol level error */
  {
    tag: 'protocol-error'
    val: string
  } |
  /** Access denied */
  {
    tag: 'denied'
    val: string
  } |
  /** Target agent or function not found */
  {
    tag: 'not-found'
    val: string
  } |
  /** Internal error on the remote side */
  {
    tag: 'remote-internal-error'
    val: string
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
