// Durable functions — wrap a non-deterministic side effect so its typed result
// is persisted to the oplog on the live run and replayed (without re-running the
// body) on recovery. Mirrors effect-golem's `Durability.wrap`/`wrapInfallible`
// over the `golem:durability@1.6.0` host interface. It is re-exported from
// the package root.

import {
  beginCustomDurableInvocation,
  type DurableFunctionType,
  LiveCustomDurableInvocation,
  observeFunctionCall,
} from 'golem:durability/durability@1.6.0';

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
 * Persists the body's result; a throw leaves the operation unfinished without triggering a retry.
 *
 * Sequential execution only, including properly nested calls. An async body may await host calls
 * only when no unrelated guest work runs during the wait. Do not overlap this operation with other
 * promises or callbacks, even those using ordinary host APIs: sibling effects can be attributed to
 * the wrong operation, causing incorrect replay or invocation failure. Misuse is not reliably detected.
 * Await each nested operation and do not detach work. Stop and join outstanding work before the
 * body exits, including on failure; Promise.all rejection does not join surviving siblings.
 */
export function durable<Req, Ok>(
  spec: DurableSpec<Req, Ok>,
  request: Req,
  body: () => Promise<Ok>,
): Promise<Ok>;
/** Synchronous form of {@link durable}; the same sequential-only and no-detached-work limits apply. */
export function durable<Req, Ok>(spec: DurableSpec<Req, Ok>, request: Req, body: () => Ok): Ok;
/**
 * Persists either Result variant; a throw is an uncaught defect.
 * The same sequential-only and no-detached-work limits as {@link durable} apply.
 */
export function durable<Req, Ok, Err>(
  spec: DurableFallibleSpec<Req, Ok, Err>,
  request: Req,
  body: () => Promise<Result<Ok, Err>>,
): Promise<Result<Ok, Err>>;
/** Synchronous Result form of {@link durable}; the same sequential-only and no-detached-work limits apply. */
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

  const invocation = beginCustomDurableInvocation(functionName, requestTsv, spec.functionType);
  if (invocation.tag === 'replayed') {
    // Replay: return the persisted response without running the body.
    const persisted = invocation.val;
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
    const decoded = responseCodec.fromValue(typedSchemaValueFromWit(persisted.response).value);
    return decoded;
  }

  // Live: run the body, persist the typed response, then finish the invocation.
  // A thrown/rejected body drops the unfinished resource. If later recovery replays
  // this incomplete invocation, it re-runs the body.
  const liveInvocation = invocation.val;
  let consumed = false;
  const dropUnfinished = () => {
    if (consumed) return;
    consumed = true;
    const resource = liveInvocation as unknown as {
      [Symbol.dispose]: () => void;
    };
    resource[Symbol.dispose]();
  };
  const persistAndEnd = (result: unknown): unknown => {
    let responseTsv;
    try {
      responseTsv = typedSchemaValueToWit({
        graph: responseCodec.graph,
        value: responseCodec.toValue(result),
      });
    } catch (error) {
      dropUnfinished();
      throw error;
    }
    consumed = true;
    LiveCustomDurableInvocation.finish(liveInvocation, responseTsv, spec.forcedCommit ?? false);
    return result;
  };

  try {
    const ran = body();
    if (ran instanceof Promise) {
      return ran.then(persistAndEnd, (error) => {
        dropUnfinished();
        throw error;
      });
    }
    return persistAndEnd(ran);
  } catch (error) {
    dropUnfinished();
    throw error;
  }
}
