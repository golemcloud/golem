declare module 'golem:agent/host@2.0.0' {
  import * as golemAgent200Common from 'golem:agent/common@2.0.0';
  import * as golemCore200Types from 'golem:core/types@2.0.0';
  import * as wasiClocks030SystemClock from 'wasi:clocks/system-clock@0.3.0';
  /**
   * Gets all the registered agent types
   */
  export function getAllAgentTypes(): RegisteredAgentType[];
  /**
   * Get a specific registered agent type by name
   */
  export function getAgentType(agentTypeName: string): RegisteredAgentType | undefined;
  /**
   * Gets the registered agent type used by an existing agent, identified by its agent ID.
   */
  export function getAgentTypeByAgentId(agentId: string): RegisteredAgentType | undefined;
  /**
   * Constructs a string agent-id from the agent type and its constructor parameters
   * and an optional phantom ID.
   * `input` is a value tree whose root encodes the constructor's parameter list.
   * @throws AgentError
   */
  export function makeAgentId(agentTypeName: string, input: SchemaValueTree, phantomId: Uuid | undefined): string;
  /**
   * Parses an agent-id (created by `make-agent-id`) into an agent type name and its constructor parameters
   * and an optional phantom ID.
   * The constructor parameters are returned as a self-contained typed value
   * (graph + value tree) so the receiver can interpret them without an
   * external schema registry.
   * @throws AgentError
   */
  export function parseAgentId(agentId: string): [string, TypedSchemaValue, Uuid | undefined];
  /**
   * @throws WebhookError
   */
  export function createWebhook(promiseId: PromiseId): string;
  /**
   * @throws ConfigValueError
   */
  export function getConfigValue(key: string[], expected: SchemaGraph): SchemaValueTree;
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
  export class WasmRpc {
    /**
     * Creates an RPC client connecting to the given target agent.
     * `constructor` is a value tree whose root encodes the target agent
     * constructor's parameter list. This fail-fast form traps if the client
     * cannot be created and is intended for statically generated clients.
     */
    constructor(agentTypeName: string, constructor: SchemaValueTree, phantomId: Uuid | undefined, agentConfig: TypedAgentConfigValue[]);
    /**
     * Creates an RPC client connecting to the given target agent.
     * `constructor` is a value tree whose root encodes the target agent
     * constructor's parameter list. This fallible form returns an RPC error
     * if the client cannot be created and is intended for reflective and
     * other dynamic clients.
     * @throws RpcError
     */
    static create(agentTypeName: string, constructor: SchemaValueTree, phantomId: Uuid | undefined, agentConfig: TypedAgentConfigValue[]): WasmRpc;
    /**
     * Invokes a remote method with the given parameters, and awaits the result.
     * `input` encodes the method's parameter list. The returned result is
     * `none` for a `unit` output and `some(value)` for a `single` output.
     * @throws RpcError
     */
    invokeAndAwait(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): InvocationResultWithMetadata;
    /**
     * Triggers the invocation of a remote method with the given parameters,
     * and returns its final identity immediately.
     * @throws RpcError
     */
    invoke(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): InvocationMetadata;
    /**
     * Invokes a remote method with the given parameters, and returns a `future-invoke-result` value which can
     * be polled for the result together with the final invocation identity.
     * With this function it is possible to call multiple (different) agents simultaneously.
     */
    asyncInvokeAndAwait(methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): AsyncInvocationWithMetadata;
    /**
     * Schedules an invocation for later and returns its final identity.
     * @throws RpcError
     */
    scheduleInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): ScheduledInvocationReceipt;
    /**
     * Schedules an invocation for later and returns its final identity and
     * cancellation capability. Call cancel on the returned resource to
     * cancel the invocation before the scheduled time.
     * @throws RpcError
     */
    scheduleCancelableInvocation(scheduledTime: Datetime, methodName: string, input: SchemaValueTree, scopeCard: PermissionCard | undefined): CancelableScheduledInvocationReceipt;
  }
  export class FutureInvokeResult {
    /**
     * Awaits the result of the invocation.
     * @throws RpcError
     */
    get(): Promise<SchemaValueTree | undefined>;
    /**
     * Best-effort attempt to cancel the remote invocation by idempotency key.
     * If the invocation has already started or completed, this is a no-op.
     */
    cancel(): void;
  }
  export class CancellationToken {
    /**
     * Cancel the scheduled invocation
     */
    cancel(): void;
  }
  export type Uuid = golemCore200Types.Uuid;
  export type PromiseId = golemCore200Types.PromiseId;
  export type SchemaGraph = golemCore200Types.SchemaGraph;
  export type SchemaValueTree = golemCore200Types.SchemaValueTree;
  export type TypedSchemaValue = golemCore200Types.TypedSchemaValue;
  export type PermissionCard = golemCore200Types.PermissionCard;
  export type Secret = golemCore200Types.Secret;
  export type Datetime = wasiClocks030SystemClock.Instant;
  export type AgentError = golemAgent200Common.AgentError;
  export type AgentType = golemAgent200Common.AgentType;
  export type RegisteredAgentType = golemAgent200Common.RegisteredAgentType;
  export type TypedAgentConfigValue = golemAgent200Common.TypedAgentConfigValue;
  /**
   * Creates a webhook that can be used to integrate with webhook driven apis.
   * When the created url is called with a post request, the provided promise-id is completed with the body of the post request.
   * Note the following behaviours:
   * * Only agents whoose agent types are _currently_ deployed via an http api are allowed to create a webhook. Calling this function while the agent
   *    is not deployed via an http api will trap.
   * * Only the agent type that created the promise is allowed to create a webhook for it. Using this host function
   *   from a different agent type will trap.
   */
  export type WebhookError =
  {
    tag: 'permission-denied'
  } |
  {
    tag: 'internal-error'
    val: string
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
  } |
  /** The remote endpoint returned an agent-domain error */
  {
    tag: 'remote-agent-error'
    val: AgentError
  };
  /**
   * Final identity allocated for one remote invocation. For an ephemeral
   * target, `agent-id` contains the generated one-shot phantom ID.
   */
  export type InvocationMetadata = {
    agentId: string;
    idempotencyKey: string;
  };
  /**
   * Result of an awaited invocation together with its final identity.
   */
  export type InvocationResultWithMetadata = {
    metadata: InvocationMetadata;
    result?: SchemaValueTree;
  };
  /**
   * Receipt returned when an invocation has been scheduled.
   */
  export type ScheduledInvocationReceipt = {
    metadata: InvocationMetadata;
  };
  /**
   * Asynchronous invocation handle together with its final identity.
   */
  export type AsyncInvocationWithMetadata = {
    metadata: InvocationMetadata;
    future: FutureInvokeResult;
  };
  /**
   * Receipt for a scheduled invocation that can still be cancelled.
   */
  export type CancelableScheduledInvocationReceipt = {
    metadata: InvocationMetadata;
    cancellationToken: CancellationToken;
  };
  /**
   * Get the current value of the config key.
   * The expected schema is a hint to the host what type of value is expected by the guest and can be used
   * by the host to automatically migrate config values to fit the expected schema.
   * Only keys that are declared by the agent-type are allowed to be accessed. Trying
   * to access an undeclared key will trap, unless the expected type is an option. In that case
   * none is returned.
   * Getting a local key will get values defined as part of the current
   * component revision + overrides declared during agent creation.
   * Getting a shared key will get the current value of the key in the environment.
   */
  export type ConfigValueError =
  {
    tag: 'permission-denied'
  };
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
