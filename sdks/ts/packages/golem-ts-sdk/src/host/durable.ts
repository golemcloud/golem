// Durable functions — wrap a non-deterministic side effect so its typed result
// is persisted to the oplog on the live run and replayed (without re-running the
// body) on recovery. Mirrors effect-golem's `Durability.wrap`/`wrapInfallible`
// over the `golem:durability@1.6.0` host interface. It is re-exported from
// the package root.

import {
  beginCustomDurableInvocation,
  beginCustomDurableInvocationAsync,
  type DurableFunctionType,
  LiveCustomDurableInvocation,
  observeFunctionCall,
  type PersistedDurableFunctionInvocation,
  type TypedSchemaValue,
} from 'golem:durability/durability@1.6.0';
import * as scopes from 'wasm-rquickjs:custom-invocation-context';

import { compileSchema } from '../schema/adapter';
import type { SchemaCodec } from '../schema/codec';
import { buildResultCodec } from '../schema/result';
import { StandardSchemaV1 } from '../schema/standardSchema';
import { typedSchemaValueFromWit, typedSchemaValueToWit } from '../internal/schema-model';
import { Result } from './result';

/**
 * The kind of wrapped function, mirroring the WIT `wrapped-function-type` (and
 * `golem-rust`'s `WrappedFunctionType`). Picks the executor's commit/replay
 * policy:
 * - `readLocal` — a local read-only side effect (PRNG, reading the clock/FS).
 * - `writeLocal` — a local mutating side effect (FS write, spawning a process).
 * - `readRemote` — an idempotent remote read (polling a job, fetching a page).
 * - `writeRemote` — a non-idempotent remote write (sending an LLM prompt, a DB insert).
 */
export const FunctionType = {
  readLocal: { tag: 'read-local' } as const,
  writeLocal: { tag: 'write-local' } as const,
  readRemote: { tag: 'read-remote' } as const,
  writeRemote: { tag: 'write-remote' } as const,
} as const;

/** Static descriptor of a durable function: what it is + how its I/O is typed. */
interface DurableSpec<Req, Ok> {
  /** Interface name, e.g. `"host-features"`. Combined with `function` into the persisted name. */
  readonly iface: string;
  /** Function name, e.g. `"fetchQuote"`. */
  readonly function: string;
  /** The commit/replay policy — see {@link FunctionType}. */
  readonly functionType: DurableFunctionType;
  /** Schema for the `request` value (persisted so oplogs are self-describing). */
  readonly requestSchema: StandardSchemaV1<Req>;
  /** Schema for the body's success value. */
  readonly success: StandardSchemaV1<Ok>;
  /** Force an efficient oplog commit at the end of the region (default false). */
  readonly forcedCommit?: boolean;
}

/** Fallible variant — carries an `error` schema; the body returns a {@link Result}. */
interface DurableFallibleSpec<Req, Ok, Err> extends DurableSpec<Req, Ok> {
  /** Schema for the typed error; the persisted response is a WIT `result<success, error>`. */
  readonly error: StandardSchemaV1<Err>;
}

/**
 * Persists the result of a synchronous body; a throw leaves the operation unfinished without
 * triggering a retry. Use {@link durableAsync} for a body that awaits.
 *
 * Throws a `TypeError` when the body returns a Promise; the operation is then left unfinished.
 */
export function durable<Req, Ok>(spec: DurableSpec<Req, Ok>, request: Req, body: () => Ok): Ok;
/** Result form of {@link durable}: persists either Result variant; a throw is an uncaught defect. */
export function durable<Req, Ok, Err>(
  spec: DurableFallibleSpec<Req, Ok, Err>,
  request: Req,
  body: () => Result<Ok, Err>,
): Result<Ok, Err>;
export function durable(
  spec: DurableSpec<unknown, unknown> & { error?: StandardSchemaV1<unknown> },
  request: unknown,
  body: () => unknown,
): unknown {
  const operation = prepare(spec, request);
  const invocation = beginCustomDurableInvocation(
    operation.functionName,
    operation.request,
    spec.functionType,
  );
  if (invocation.tag === 'replayed') return operation.replay(invocation.val);

  // Live: run the body, persist the typed response, then finish the invocation.
  // A throw drops the unfinished resource. If later recovery replays this incomplete
  // invocation, it re-runs the body.
  const live = invocation.val;
  let response;
  try {
    const result = body();
    if (result instanceof Promise) {
      // The detached rest of the body is no longer part of the operation.
      result.then(undefined, () => {});
      throw new TypeError(
        `durable(): the body of '${operation.functionName}' returned a Promise; use durableAsync() for asynchronous bodies`,
      );
    }
    response = operation.encode(result);
    LiveCustomDurableInvocation.finish(live, response.wire, spec.forcedCommit ?? false);
    return response.result;
  } catch (error) {
    if (response === undefined) dispose(live);
    throw error;
  }
}

/**
 * Persists the result of an asynchronous body; a throw or rejection leaves the operation
 * unfinished without triggering a retry.
 *
 * The body runs in its own custom durability scope. Host calls that the body starts, also after
 * an `await`, in timers or in callbacks it registers, belong to this operation, and operations it
 * starts are its children. Concurrent operations and unrelated work running during the wait keep
 * their own scopes. The operation finishes only after the operations that started inside it have
 * finished, also when the body did not await them. Work that the body detaches and that runs after
 * the operation finished belongs to the nearest enclosing open operation, or to the invocation.
 */
export function durableAsync<Req, Ok>(
  spec: DurableSpec<Req, Ok>,
  request: Req,
  body: () => Promise<Ok>,
): Promise<Ok>;
/** Result form of {@link durableAsync}: persists either Result variant; a rejection is an uncaught defect. */
export function durableAsync<Req, Ok, Err>(
  spec: DurableFallibleSpec<Req, Ok, Err>,
  request: Req,
  body: () => Promise<Result<Ok, Err>>,
): Promise<Result<Ok, Err>>;
export async function durableAsync(
  spec: DurableSpec<unknown, unknown> & { error?: StandardSchemaV1<unknown> },
  request: unknown,
  body: () => Promise<unknown>,
): Promise<unknown> {
  const operation = prepare(spec, request);
  const parentToken = scopes.capture();
  // Register with the enclosing operation before admission, so the parent cannot finish while
  // this operation still waits for it.
  const parent = openOperation(parentToken);
  let settle!: () => void;
  const settled = new Promise<void>((resolve) => (settle = resolve));
  parent?.children.add(settled);
  try {
    const invocation = await beginCustomDurableInvocationAsync(
      operation.functionName,
      operation.request,
      spec.functionType,
    );
    if (invocation.tag === 'replayed') return operation.replay(invocation.val);
    return await runLive(operation, spec.forcedCommit ?? false, parentToken, invocation.val, body);
  } finally {
    parent?.children.delete(settled);
    settle();
  }
}

/** An open live operation of {@link durableAsync}: its parent and the children it waits for. */
interface OpenOperation {
  readonly parent: OpenOperation | undefined;
  readonly children: Set<Promise<void>>;
  closed: boolean;
}

const openOperations = new WeakMap<scopes.Token, OpenOperation>();

/** The open operation that `token` stands for: its own, else the nearest open ancestor. */
function openOperation(token: scopes.Token): OpenOperation | undefined {
  let operation = openOperations.get(token);
  while (operation?.closed) operation = operation.parent;
  return operation;
}

async function runLive(
  operation: Prepared,
  forcedCommit: boolean,
  parentToken: scopes.Token,
  live: LiveCustomDurableInvocation,
  body: () => Promise<unknown>,
): Promise<unknown> {
  // Enter in the parent's scope, so the child scope does not stay current in the parent.
  const token = scopes.run(parentToken, () => scopes.enterLive(live));
  const open: OpenOperation = {
    parent: openOperation(parentToken),
    children: new Set(),
    closed: false,
  };
  openOperations.set(token, open);
  try {
    let outcome: { ok: true; value: unknown } | { ok: false; error: unknown };
    try {
      outcome = { ok: true, value: await scopes.run(token, body) };
    } catch (error) {
      outcome = { ok: false, error };
    }
    while (open.children.size > 0) await Promise.allSettled([...open.children]);
    if (!outcome.ok) throw outcome.error;
    const response = operation.encode(outcome.value);
    await scopes.run(token, () =>
      LiveCustomDurableInvocation.finishAsync(live, response.wire, forcedCommit),
    );
    return response.result;
  } finally {
    // An unfinished drop leaves the invocation incomplete, so recovery re-runs the body.
    scopes.run(token, () => dispose(live));
    open.closed = true;
    scopes.close(token);
  }
}

interface Prepared {
  readonly functionName: string;
  readonly request: TypedSchemaValue;
  /** Decodes the recorded response of a completed invocation. */
  replay(persisted: PersistedDurableFunctionInvocation): unknown;
  /** Encodes the body's result as the persisted response. */
  encode(result: unknown): { wire: TypedSchemaValue; result: unknown };
}

function prepare(
  spec: DurableSpec<unknown, unknown> & { error?: StandardSchemaV1<unknown> },
  request: unknown,
): Prepared {
  const functionName = spec.iface ? `${spec.iface}::${spec.function}` : spec.function;

  const requestCodec = compileSchema(spec.requestSchema);
  const okCodec = compileSchema(spec.success);
  // With an `error` schema the response is a `result<ok, err>` value (reuses the
  // Item-3 result codec); otherwise it's the bare success value.
  const responseCodec: SchemaCodec = spec.error
    ? buildResultCodec(okCodec, compileSchema(spec.error))
    : okCodec;

  observeFunctionCall(spec.iface, spec.function);
  const requestTsv = typedSchemaValueToWit({
    graph: requestCodec.graph,
    value: requestCodec.toValue(request),
  });

  return {
    functionName,
    request: requestTsv,
    replay(persisted) {
      if (persisted.functionName !== functionName) {
        throw new Error(
          `durable replay mismatch: expected function '${functionName}', oplog has '${persisted.functionName}'`,
        );
      }
      if (persisted.functionType.tag !== spec.functionType.tag) {
        throw new Error(
          `durable replay mismatch for '${functionName}': expected function-type '${spec.functionType.tag}', oplog has '${persisted.functionType.tag}'`,
        );
      }
      return responseCodec.fromValue(typedSchemaValueFromWit(persisted.response).value);
    },
    encode(result) {
      const wire = typedSchemaValueToWit({
        graph: responseCodec.graph,
        value: responseCodec.toValue(result),
      });
      return { wire, result };
    },
  };
}

function dispose(live: LiveCustomDurableInvocation): void {
  (live as unknown as { [Symbol.dispose]: () => void })[Symbol.dispose]();
}
