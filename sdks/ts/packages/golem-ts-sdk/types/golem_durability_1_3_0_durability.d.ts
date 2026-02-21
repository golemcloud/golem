declare module 'golem:durability/durability@1.3.0' {
  import * as golemApi130Host from 'golem:api/host@1.3.0';
  import * as golemApi130Oplog from 'golem:api/oplog@1.3.0';
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
   * Writes a record to the agent's oplog representing a durable function invocation
   * The request and response are defined as pairs of value and type, which makes it
   * self-describing for observers of oplogs.
   */
  export function persistDurableFunctionInvocation(functionName: string, request: ValueAndType, response: ValueAndType, functionType: DurableFunctionType): void;
  /**
   * Reads the next persisted durable function invocation from the oplog during replay
   */
  export function readPersistedDurableFunctionInvocation(): PersistedDurableFunctionInvocation;
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
  export type PersistenceLevel = golemApi130Host.PersistenceLevel;
  export type OplogIndex = golemApi130Oplog.OplogIndex;
  export type WrappedFunctionType = golemApi130Oplog.WrappedFunctionType;
  export type Datetime = wasiClocks023WallClock.Datetime;
  export type Pollable = wasiIo023Poll.Pollable;
  export type ValueAndType = golemRpc022Types.ValueAndType;
  export type DurableFunctionType = WrappedFunctionType;
  /**
   * Represents the current durable execution state
   */
  export type DurableExecutionState = {
    /**
     * If true, the executor is in live mode, side-effects should be performed and persisted.
     * If false, the executor is in replay mode, side-effects should be replayed from the persisted data.
     */
    isLive: boolean;
    /** The currently active persistence level */
    persistenceLevel: PersistenceLevel;
  };
  /**
   * Represents the oplog entry version; this is for backward compatibility and most use cases should always use
   * (and expect) the latest version.
   */
  export type OplogEntryVersion = "v1" | "v2";
  /**
   * Represents a persisted durable function invocation. The `response` field
   * contains a value and its type information together, making the user-defined payload observable by external tools.
   */
  export type PersistedDurableFunctionInvocation = {
    /** The timestamp of the invocation. */
    timestamp: Datetime;
    /** The invoked function's unique name */
    functionName: string;
    /** Arbitrary structured value (and type) describing the invocation's result */
    response: ValueAndType;
    /** Type of the durable function invocation */
    functionType: DurableFunctionType;
    /** Oplog entry version */
    entryVersion: OplogEntryVersion;
  };
}
