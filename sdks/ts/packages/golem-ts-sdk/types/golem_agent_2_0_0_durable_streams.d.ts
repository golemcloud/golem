/**
 * Finite external Durable Streams operations. Callers own checkpoints, buffered
 * items, producer progress and retries; the host records each complete attempt.
 */
declare module 'golem:agent/durable-streams@2.0.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  /**
   * One protocol read attempt. Replay returns the recorded batch without HTTP.
   * Auth is a string secret used as a Bearer token only at the live wire boundary.
   * @throws DurableStreamError
   */
  export function readDurableStreamBatch(request: DurableStreamReadRequest, auth: Secret | undefined): Promise<DurableStreamBatch>;
  /**
   * One protocol-idempotent append attempt. Retry with the identical tuple and body.
   * The caller, not this function, owns producer progress and pending append state.
   * @throws DurableStreamError
   */
  export function appendDurableStreamBatch(request: DurableStreamAppendRequest, auth: Secret | undefined): Promise<DurableStreamAppendReceipt>;
  export type Secret = golemCore200Types.Secret;
  export type DurableStreamMode = "json" | "bytes";
  export type DurableStreamTransport = "catch-up" | "long-poll" | "sse";
  /**
   * Both values are server-generated opaque tokens, not Golem stream offsets.
   */
  export type DurableStreamCheckpoint = {
    offset: string;
    cursor?: string;
  };
  export type DurableStreamReadRequest = {
    url: string;
    checkpoint: DurableStreamCheckpoint;
    mode: DurableStreamMode;
    transport: DurableStreamTransport;
    /**
     * Pin the original stream media type after the initial catch-up read.
     * Required for SSE, whose HTTP media type is text/event-stream.
     */
    contentType?: string;
    /** Whole-attempt deadline, in milliseconds, from 1 through 300000. */
    timeoutMs: bigint;
  };
  /**
   * A complete HTTP body or SSE data/control pair. Empty is not EOF unless closed.
   */
  export type DurableStreamBatch = {
    payload: Uint8Array;
    contentType: string;
    next: DurableStreamCheckpoint;
    upToDate: boolean;
    closed: boolean;
  };
  export type DurableStreamProducer = {
    id: string;
    epoch: bigint;
    sequence: bigint;
  };
  export type DurableStreamAppendPayload =
  /** Individually encoded complete JSON values; the host frames the outer array. */
  {
    tag: 'json'
    val: string[]
  } |
  {
    tag: 'bytes'
    val: Uint8Array
  };
  export type DurableStreamAppendRequest = {
    url: string;
    contentType: string;
    payload: DurableStreamAppendPayload;
    producer: DurableStreamProducer;
    close: boolean;
    timeoutMs: bigint;
  };
  export type DurableStreamAppendReceipt = {
    /** Duplicate acknowledgements may omit the offset. */
    nextOffset?: string;
    epoch: bigint;
    sequence: bigint;
    closed: boolean;
  };
  export type DurableStreamErrorKind = "invalid-request" | "permission-denied" | "not-found" | "gone" | "closed" | "sequence-conflict" | "fenced" | "producer-diverged" | "protocol-error" | "payload-too-large" | "timeout" | "transport" | "rate-limited" | "unavailable";
  export type DurableStreamError = {
    kind: DurableStreamErrorKind;
    message: string;
    retryAfterMs?: bigint;
    producerEpoch?: bigint;
    expectedSequence?: bigint;
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
