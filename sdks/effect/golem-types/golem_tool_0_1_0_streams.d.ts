/**
 * Byte-stream endpoints shared by tool implementations and middleware without
 * granting access to ambient tool discovery or dispatch.
 */
declare module 'golem:tool/streams@0.1.0' {
  export class ToolStdoutWriter {
    /**
     * @throws StreamWriteError
     */
    write(bytes: Uint8Array): Promise<void>;
    /**
     * @throws StreamWriteError
     */
    finish(): Promise<void>;
    /**
     * @throws StreamWriteError
     */
    fail(reason: ByteStreamFailure): Promise<void>;
  }
  /**
   * Recoverable attachment failures are stream values. Clean EOF is closure.
   */
  export type ByteStreamFailure =
  {
    tag: 'cancelled'
  } |
  {
    tag: 'abandoned'
  } |
  {
    tag: 'resource-exhausted'
  } |
  {
    tag: 'failed'
    val: string
  };
  /**
   * Every successful item contains a non-empty byte chunk.
   */
  export type ByteStreamItem = Result<Uint8Array, ByteStreamFailure>;
  export type ByteStreamCloseCause =
  {
    tag: 'finished'
  } |
  {
    tag: 'failed'
    val: ByteStreamFailure
  } |
  {
    tag: 'consumer-cancelled'
  };
  export type StreamWriteError =
  {
    tag: 'closed'
    val: ByteStreamCloseCause
  } |
  {
    tag: 'concurrent-operation'
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
