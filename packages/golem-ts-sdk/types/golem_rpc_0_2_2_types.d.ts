declare module 'golem:rpc/types@0.2.2' {
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
  /**
   * Parses a UUID from a string
   */
  export function parseUuid(uuid: string): Result<Uuid, string>;
  /**
   * Converts a UUID to a string
   */
  export function uuidToString(uuid: Uuid): string;
  export function extractValue(vnt: ValueAndType): WitValue;
  export function extractType(vnt: ValueAndType): WitType;
  export class WasmRpc {
    constructor(workerId: WorkerId);
    invokeAndAwait(functionName: string, functionParams: WitValue[]): Result<WitValue, RpcError>;
    invoke(functionName: string, functionParams: WitValue[]): Result<void, RpcError>;
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
    subscribe(): Pollable;
    get(): Result<WitValue, RpcError> | undefined;
  }
  export class CancellationToken {
    cancel(): void;
  }
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
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
   * Represents a Golem worker
   */
  export type WorkerId = {
    componentId: ComponentId;
    workerName: string;
  };
  export type NodeIndex = number;
  export type ResourceId = bigint;
  export type ResourceMode = "owned" | "borrowed";
  export type WitTypeNode = {
    tag: 'record-type'
    val: [string, NodeIndex][]
  } |
  {
    tag: 'variant-type'
    val: [string, NodeIndex | undefined][]
  } |
  {
    tag: 'enum-type'
    val: string[]
  } |
  {
    tag: 'flags-type'
    val: string[]
  } |
  {
    tag: 'tuple-type'
    val: NodeIndex[]
  } |
  {
    tag: 'list-type'
    val: NodeIndex
  } |
  {
    tag: 'option-type'
    val: NodeIndex
  } |
  {
    tag: 'result-type'
    val: [NodeIndex | undefined, NodeIndex | undefined]
  } |
  {
    tag: 'prim-u8-type'
  } |
  {
    tag: 'prim-u16-type'
  } |
  {
    tag: 'prim-u32-type'
  } |
  {
    tag: 'prim-u64-type'
  } |
  {
    tag: 'prim-s8-type'
  } |
  {
    tag: 'prim-s16-type'
  } |
  {
    tag: 'prim-s32-type'
  } |
  {
    tag: 'prim-s64-type'
  } |
  {
    tag: 'prim-f32-type'
  } |
  {
    tag: 'prim-f64-type'
  } |
  {
    tag: 'prim-char-type'
  } |
  {
    tag: 'prim-bool-type'
  } |
  {
    tag: 'prim-string-type'
  } |
  {
    tag: 'handle-type'
    val: [ResourceId, ResourceMode]
  };
  export type NamedWitTypeNode = {
    name: string | undefined;
    owner: string | undefined;
    type: WitTypeNode;
  };
  export type WitType = {
    nodes: NamedWitTypeNode[];
  };
  export type Uri = {
    value: string;
  };
  export type WitNode = {
    tag: 'record-value'
    val: NodeIndex[]
  } |
  {
    tag: 'variant-value'
    val: [number, NodeIndex | undefined]
  } |
  {
    tag: 'enum-value'
    val: number
  } |
  {
    tag: 'flags-value'
    val: boolean[]
  } |
  {
    tag: 'tuple-value'
    val: NodeIndex[]
  } |
  {
    tag: 'list-value'
    val: NodeIndex[]
  } |
  {
    tag: 'option-value'
    val: NodeIndex | undefined
  } |
  {
    tag: 'result-value'
    val: Result<NodeIndex | undefined, NodeIndex | undefined>
  } |
  {
    tag: 'prim-u8'
    val: number
  } |
  {
    tag: 'prim-u16'
    val: number
  } |
  {
    tag: 'prim-u32'
    val: number
  } |
  {
    tag: 'prim-u64'
    val: bigint
  } |
  {
    tag: 'prim-s8'
    val: number
  } |
  {
    tag: 'prim-s16'
    val: number
  } |
  {
    tag: 'prim-s32'
    val: number
  } |
  {
    tag: 'prim-s64'
    val: bigint
  } |
  {
    tag: 'prim-float32'
    val: number
  } |
  {
    tag: 'prim-float64'
    val: number
  } |
  {
    tag: 'prim-char'
    val: string
  } |
  {
    tag: 'prim-bool'
    val: boolean
  } |
  {
    tag: 'prim-string'
    val: string
  } |
  {
    tag: 'handle'
    val: [Uri, bigint]
  };
  export type WitValue = {
    nodes: WitNode[];
  };
  export type ValueAndType = {
    value: WitValue;
    typ: WitType;
  };
  export type RpcError = {
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
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
