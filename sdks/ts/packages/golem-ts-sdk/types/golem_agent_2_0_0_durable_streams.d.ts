/**
 * External Durable Streams descriptors are recorded when resources are created.
 * Callers own checkpoints, buffered items, producer progress and retries.
 */
declare module 'golem:agent/durable-streams@2.0.0' {
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  export class DurableStreamReader {
    constructor(options: DurableStreamReaderOptions, auth: Secret | undefined);
    /**
     * One protocol attempt. Replay returns the recorded batch without HTTP.
     * @throws DurableStreamError
     */
    read(request: DurableStreamReadRequest): Promise<DurableStreamBatch>;
  }
  export class DurableStreamWriter {
    constructor(options: DurableStreamWriterOptions, auth: Secret | undefined);
    /**
     * One protocol-idempotent attempt. Retry the identical sequence, body and close flag.
     * @throws DurableStreamError
     */
    append(request: DurableStreamAppendRequest): Promise<DurableStreamAppendReceipt>;
  }
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
  export type DurableStreamReaderOptions = {
    url: string;
    mode: DurableStreamMode;
    /** Whole-attempt deadline, in milliseconds, from 1 through 300000. */
    timeoutMs: bigint;
  };
  export type DurableStreamReadRequest = {
    checkpoint: DurableStreamCheckpoint;
    transport: DurableStreamTransport;
    /**
     * Pin the original stream media type after the initial catch-up read.
     * Required for SSE, whose HTTP media type is text/event-stream.
     */
    contentType?: string;
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
  export type DurableStreamWriterOptions = {
    url: string;
    contentType: string;
    producerId: string;
    producerEpoch: bigint;
    /** Whole-attempt deadline, in milliseconds, from 1 through 300000. */
    timeoutMs: bigint;
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
    payload: DurableStreamAppendPayload;
    sequence: bigint;
    close: boolean;
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
