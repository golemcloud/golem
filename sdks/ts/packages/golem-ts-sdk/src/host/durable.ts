// Durable functions — wrap a non-deterministic side effect so its typed result
// is persisted to the oplog on the live run and replayed (without re-running the
// body) on recovery. Mirrors effect-golem's `Durability.wrap`/`wrapInfallible`
// over the same `golem:durability@1.5.0` host interface; the base decorator SDK
// has no equivalent. Lives in the shared `host/` layer, so both the fluent and
// decorator surfaces get it (re-exported from the package root).

import {
  beginDurableFunction,
  currentDurableExecutionState,
  type DurableFunctionType,
  endDurableFunction,
  observeFunctionCall,
  persistDurableFunctionInvocation,
  readPersistedDurableFunctionInvocation,
} from 'golem:durability/durability@1.5.0';

import type { OplogIndex } from 'golem:api/oplog@1.5.0';

import { compileSchema, type FluentCodec } from '../fluent/schema/adapter';
import { buildResultCodec } from '../fluent/schema/result';
import { StandardSchemaV1 } from '../fluent/schema/standardSchema';
import { typedSchemaValueFromWit, typedSchemaValueToWit } from '../internal/schema-model';
import { withPersistenceLevel } from './guard';
import { Result } from './result';

/**
 * The kind of wrapped function, mirroring the WIT `wrapped-function-type` (and
 * `golem-rust`'s `WrappedFunctionType`). Picks the executor's commit/replay
 * policy:
 * - `readLocal` — a local read-only side effect (PRNG, reading the clock/FS).
 * - `writeLocal` — a local mutating side effect (FS write, spawning a process).
 * - `readRemote` — an idempotent remote read (polling a job, fetching a page).
 * - `writeRemote` — a non-idempotent remote write (sending an LLM prompt, a DB insert).
 * - `writeRemoteBatched(begin?)` / `writeRemoteTransaction(begin?)` — multi-step
 *   remote writes; the optional oplog index links follow-up steps to the batch.
 */
export const FunctionType = {
  readLocal: { tag: 'read-local' } as const,
  writeLocal: { tag: 'write-local' } as const,
  readRemote: { tag: 'read-remote' } as const,
  writeRemote: { tag: 'write-remote' } as const,
  writeRemoteBatched: (begin?: OplogIndex) => ({ tag: 'write-remote-batched', val: begin }) as const,
  writeRemoteTransaction: (begin?: OplogIndex) =>
    ({ tag: 'write-remote-transaction', val: begin }) as const,
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

const PERSIST_NOTHING = { tag: 'persist-nothing' } as const;

// Infallible — body returns the success value directly; a throw is an uncaught
// defect (the worker is retried, the region left open).
export function durable<Req, Ok>(spec: DurableSpec<Req, Ok>, request: Req, body: () => Promise<Ok>): Promise<Ok>;
export function durable<Req, Ok>(spec: DurableSpec<Req, Ok>, request: Req, body: () => Ok): Ok;
// Fallible — body returns a `Result<Ok, Err>`; the Result (ok OR err) is persisted
// and replayed. A throw is still an uncaught defect.
export function durable<Req, Ok, Err>(
  spec: DurableFallibleSpec<Req, Ok, Err>,
  request: Req,
  body: () => Promise<Result<Ok, Err>>,
): Promise<Result<Ok, Err>>;
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
  const functionName = `${spec.iface}::${spec.function}`;

  const requestCodec = compileSchema(spec.requestSchema);
  const okCodec = compileSchema(spec.success);
  // With an `error` schema the response is a `result<ok, err>` value (reuses the
  // Item-3 result codec); otherwise it's the bare success value.
  const responseCodec: FluentCodec = spec.error
    ? buildResultCodec(okCodec, compileSchema(spec.error))
    : okCodec;

  observeFunctionCall(spec.iface, spec.function);
  const beginIndex = beginDurableFunction(spec.functionType);
  const state = currentDurableExecutionState();
  const live = state.isLive || state.persistenceLevel.tag === 'persist-nothing';

  const requestTsv = typedSchemaValueToWit({
    graph: requestCodec.graph,
    value: requestCodec.toValue(request),
  });

  if (!live) {
    // Replay: return the persisted response without running the body.
    const persisted = readPersistedDurableFunctionInvocation();
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
    endDurableFunction(spec.functionType, beginIndex, false);
    return decoded;
  }

  // Live: run the body (with nested host I/O NOT re-recorded), persist the typed
  // response, then close the region. A thrown/rejected body propagates with the
  // region left open (no persist, no end) so recovery re-runs it.
  const persistAndEnd = (result: unknown): unknown => {
    const responseTsv = typedSchemaValueToWit({
      graph: responseCodec.graph,
      value: responseCodec.toValue(result),
    });
    persistDurableFunctionInvocation(functionName, requestTsv, responseTsv, spec.functionType);
    endDurableFunction(spec.functionType, beginIndex, spec.forcedCommit ?? false);
    return result;
  };

  const ran = withPersistenceLevel(PERSIST_NOTHING, body as () => unknown);
  return ran instanceof Promise ? ran.then(persistAndEnd) : persistAndEnd(ran);
}
