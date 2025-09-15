declare module 'golem:durability/durability@1.2.1' {
  import * as golemApi117Host from 'golem:api/host@1.1.7';
  import * as golemApi117Oplog from 'golem:api/oplog@1.1.7';
  import * as golemRpc022Types from 'golem:rpc/types@0.2.2';
  import * as wasiClocks023WallClock from 'wasi:clocks/wall-clock@0.2.3';
  import * as wasiIo023Poll from 'wasi:io/poll@0.2.3';
  /**
   * Observes a function call (produces logs and metrics)
   */
  export function observeFunctionCall(iface: string, function_: string): void;
  /**
   * Marks the beginning of a durable function.
   * There must be a corresponding call to `end-durable-function` after the function has
   * performed its work (it can be ended in a different context, for example after an async
   * pollable operation has been completed)
   */
  export function beginDurableFunction(functionType: DurableFunctionType): OplogIndex;
  /**
   * Marks the end of a durable function
   * This is a pair of `begin-durable-function` and should be called after the durable function
   * has performed and persisted or replayed its work. The `begin-index` should be the index
   * returned by `begin-durable-function`.
   * Normally commit behavior is decided by the executor based on the `function-type`. However, in special
   * cases the `forced-commit` parameter can be used to force commit the oplog in an efficient way.
   */
  export function endDurableFunction(functionType: DurableFunctionType, beginIndex: OplogIndex, forcedCommit: boolean): void;
  /**
   * Gets the current durable execution state
   */
  export function currentDurableExecutionState(): DurableExecutionState;
  /**
   * Writes a record to the worker's oplog representing a durable function invocation
   */
  export function persistDurableFunctionInvocation(functionName: string, request: Uint8Array, response: Uint8Array, functionType: DurableFunctionType): void;
  /**
   * Writes a record to the worker's oplog representing a durable function invocation
   * The request and response are defined as pairs of value and type, which makes it
   * self-describing for observers of oplogs. This is the recommended way to persist
   * third-party function invocations.
   */
  export function persistTypedDurableFunctionInvocation(functionName: string, request: ValueAndType, response: ValueAndType, functionType: DurableFunctionType): void;
  /**
   * Reads the next persisted durable function invocation from the oplog during replay
   */
  export function readPersistedDurableFunctionInvocation(): PersistedDurableFunctionInvocation;
  /**
   * Reads the next persisted durable function invocation from the oplog during replay, assuming it
   * was created with `persist-typed-durable-function-invocation`
   */
  export function readPersistedTypedDurableFunctionInvocation(): PersistedTypedDurableFunctionInvocation;
  export class LazyInitializedPollable {
    /**
     * Creates a `pollable` that is never ready until it gets attached to a real `pollable` implementation
     * using `set-lazy-initialized-pollable`.
     */
    constructor();
    /**
     * Sets the underlying `pollable` for a pollable created with `create-lazy-initialized-pollable`.
     */
    set(pollable: Pollable): void;
    subscribe(): Pollable;
  }
  export type PersistenceLevel = golemApi117Host.PersistenceLevel;
  export type OplogIndex = golemApi117Oplog.OplogIndex;
  export type WrappedFunctionType = golemApi117Oplog.WrappedFunctionType;
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
  export type ValueAndType = golemRpc022Types.ValueAndType;
  export type DurableFunctionType = WrappedFunctionType;
  export type DurableExecutionState = {
    isLive: boolean;
    persistenceLevel: PersistenceLevel;
  };
  export type OplogEntryVersion = "v1" | "v2";
  export type PersistedDurableFunctionInvocation = {
    timestamp: Datetime;
    functionName: string;
    response: Uint8Array;
    functionType: DurableFunctionType;
    entryVersion: OplogEntryVersion;
  };
  export type PersistedTypedDurableFunctionInvocation = {
    timestamp: Datetime;
    functionName: string;
    response: ValueAndType;
    functionType: DurableFunctionType;
    entryVersion: OplogEntryVersion;
  };
}
