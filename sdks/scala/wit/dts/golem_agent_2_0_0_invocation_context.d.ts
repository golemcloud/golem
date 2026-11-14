/**
 * Describes the invocation that runs the calling code. Every function reads values recorded
 * with the start of the invocation, so no function is a durable call.
 */
declare module 'golem:agent/invocation-context@2.0.0' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  /**
   * Returns the ID of the running invocation. A nested tool call returns the ID of the
   * enclosing invocation. Returns none outside an invocation: during component initialization,
   * in snapshot save and load, and in oplog processing.
   */
  export function invocationId(): CurrentInvocationId | undefined;
  /**
   * Returns the principals that caused the running invocation. Code that runs without a
   * recorded principal, such as snapshot save and load, gets the agent itself as both principals.
   */
  export function caller(): InvocationCaller;
  export type CurrentInvocationId = golemCore200Types.InvocationId;
  export type InvocationCaller = golemAgent200Common.Caller;
}
