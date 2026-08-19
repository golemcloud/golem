declare module 'golem:durability/durability@1.6.0' {
  import * as golemApi150Oplog from 'golem:api/oplog@1.5.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiClocks030SystemClock from 'wasi:clocks/system-clock@0.3.0';
  /**
   * Observes a function call (produces logs and metrics)
   */
  export function observeFunctionCall(iface: string, function_: string): void;
  /**
   * Opens a custom durable invocation.
   * The executor allocates a deterministic identity synchronously when the call begins. A fresh
   * or incomplete invocation returns an owned `live-custom-durable-invocation`. A completed
   * invocation returns `replayed` with its recorded response.
   */
  export function beginCustomDurableInvocation(functionName: string, request: TypedSchemaValue, functionType: DurableFunctionType): CustomDurableInvocation;
  export class LiveCustomDurableInvocation {
    /**
     * Completes the invocation with its schema-carrying response.
     */
    static finish(this_: LiveCustomDurableInvocation, response: TypedSchemaValue, forcedCommit: boolean): void;
  }
  export type WrappedFunctionType = golemApi150Oplog.WrappedFunctionType;
  export type Datetime = wasiClocks030SystemClock.Instant;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type DurableFunctionType = WrappedFunctionType;
  /**
   * Represents the oplog entry version; this is for backward compatibility and most use cases should always use
   * (and expect) the latest version.
   */
  export type OplogEntryVersion = "v1" | "v2";
  /**
   * Represents a persisted durable function invocation. The `response` field
   * contains a value and its schema graph together, making the user-defined payload observable by external tools.
   */
  export type PersistedDurableFunctionInvocation = {
    /** The timestamp of the invocation. */
    timestamp: Datetime;
    /** The invoked function's unique name */
    functionName: string;
    /** Arbitrary structured value and schema graph describing the invocation's result */
    response: TypedSchemaValue;
    /** Type of the durable function invocation */
    functionType: DurableFunctionType;
    /** Oplog entry version */
    entryVersion: OplogEntryVersion;
  };
  /**
   * The result of opening a custom durable invocation.
   */
  export type CustomDurableInvocation =
  /** Execute the operation body and finish the returned live invocation. */
  {
    tag: 'live'
    val: LiveCustomDurableInvocation
  } |
  /** Return the recorded response without executing the operation body. */
  {
    tag: 'replayed'
    val: PersistedDurableFunctionInvocation
  };
}
