/**
 * Runtime-owned next-layer capabilities, independent of ambient tool dispatch.
 */
declare module 'golem:tool/underlying@0.1.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as golemTool010Common from 'golem:tool/common@0.1.0';
  import * as golemTool010Streams from 'golem:tool/streams@0.1.0';
  export class UnderlyingTool {
    /**
     * Admits one call to the pinned next layer. Calls on this capability may
     * overlap. The stdout reader can be consumed before awaiting the result.
     * Invoking after the owning handler returned traps.
     */
    invoke(commandPath: string[], input: TypedSchemaValue, stdin: AsyncIterable<ByteStreamItem> | undefined): Promise<[UnderlyingInvokeResult, AsyncIterable<ByteStreamItem> | undefined]>;
  }
  export class UnderlyingInvokeResult {
    /**
     * Returns the immutable terminal. Only one get may be outstanding.
     * @throws UnderlyingError
     */
    get(): Promise<TypedSchemaValue | undefined>;
    /**
     * Requests cancellation of this call only. Dropping this resource detaches
     * observation; it does not cancel work admitted by the runtime.
     */
    cancel(): void;
  }
  export type ToolError = golemTool010Common.ToolError;
  export type ByteStreamItem = golemTool010Streams.ByteStreamItem;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type UnderlyingError =
  {
    tag: 'tool-error'
    val: ToolError
  } |
  {
    tag: 'protocol-error'
    val: string
  } |
  {
    tag: 'denied'
    val: string
  } |
  {
    tag: 'internal-error'
    val: string
  } |
  /** This call's explicit cancellation won terminal arbitration. */
  {
    tag: 'cancelled'
  } |
  {
    tag: 'resource-exhausted'
    val: string
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
