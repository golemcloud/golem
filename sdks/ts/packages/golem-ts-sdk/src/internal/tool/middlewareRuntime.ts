// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import type {
  InvocationResult as WireInvocationResult,
  Tool as WireTool,
  ToolError as WireToolError,
  TypedSchemaValue as WireTypedSchemaValue,
} from 'golem:tool/common@0.1.0';
import type {
  UnderlyingError,
  UnderlyingInvokeResult,
  UnderlyingTool as WireUnderlyingTool,
} from 'golem:tool/underlying@0.1.0';
import type { ByteStreamItem, ToolOutputWriter } from 'golem:tool/streams@0.1.0';
import {
  deepEqual,
  drainUnconsumedQuotaAndPermissionCardHandles,
  preflightWitTypedSchemaValue,
  typedSchemaValueFromWit,
} from '../schema-model';
import type { SchemaCodec } from '../../schema/codec';
import { schemaValueConforms } from './validation';
import type { ExtendedCommandBody } from './model';
import { closeAsyncIterable, isAsyncIterable } from './asyncIterable';
import {
  encodeDeclaredToolErrorPayload,
  encodeToolValue,
  isDeclaredToolError,
} from './invocationResult';
import {
  createToolUnderlyingForExtendedTool,
  decodeDeclaredToolError,
  getExtendedToolDefinition,
  ToolInvokeError,
  type AnyToolDefinition,
  type ToolClientFailureContext,
  type ToolUnderlying,
  type UniversalToolMiddlewareInvocation,
  type UniversalToolUnderlying,
} from '../../tool';
import type { Principal } from '../../principal';
import type {
  MonomorphicToolMiddlewareSource,
  UniversalToolMiddlewareSource,
} from '../registry/toolMiddlewareRegistry';
import { resolveToolInvocation } from '../registry/toolRegistry';

async function collectAsyncIterable(
  source: AsyncIterable<number> | undefined,
): Promise<readonly number[] | undefined> {
  if (source === undefined) return undefined;
  const values: number[] = [];
  try {
    for await (const value of source) values.push(value);
    return values;
  } finally {
    await closeAsyncIterable(source);
  }
}

function replayAsyncIterable(values: readonly number[]): AsyncIterable<number> {
  return {
    async *[Symbol.asyncIterator]() {
      yield* values;
    },
  };
}

type RawUnderlyingTool = Pick<WireUnderlyingTool, 'invoke'>;

export class ToolUnderlyingMisuseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ToolUnderlyingMisuseError';
  }
}

export function decodeUnderlyingToolError<Errors>(
  error: unknown,
  decodeCustomError: (error: Extract<WireToolError, { readonly tag: 'custom-error' }>['val']) =>
    | Errors
    | {
        readonly tag: 'unknown-error';
        readonly name: string;
        readonly payload: WireTypedSchemaValue;
      },
): unknown {
  const cause =
    error instanceof ToolInvokeError ? error.cause : isWireToolError(error) ? error : null;
  if (cause === null) return error;
  if (cause.tag !== 'tool' && cause.tag !== 'custom-error') {
    return error instanceof ToolInvokeError ? error : new ToolInvokeError(cause);
  }

  try {
    const customError = cause.tag === 'tool' ? cause.error : cause.val;
    if (!isCustomToolError(customError)) {
      if (cause.tag === 'custom-error') throw new Error('malformed named custom error payload');
      return ToolInvokeError.tool(customError as Errors);
    }
    preflightTypedSchemaValue(customError.payload);
    const decoded = decodeCustomError(customError);
    return isUnknownToolError(decoded)
      ? new ToolInvokeError(decoded)
      : ToolInvokeError.tool(decoded);
  } catch (decodeError) {
    return new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(decodeError) });
  }
}

export function encodeToolInvokeError<Errors>(
  error: unknown,
  encodeCustomError: (error: Errors) => WireTypedSchemaValue,
): WireToolError {
  if (!(error instanceof ToolInvokeError)) throw error;
  if (error.cause.tag === 'unknown-error') {
    try {
      preflightTypedSchemaValue(error.cause.payload);
      return {
        tag: 'custom-error',
        val: { name: error.cause.name, payload: error.cause.payload },
      };
    } catch (encodeError) {
      return { tag: 'invalid-result', val: errorMessage(encodeError) };
    }
  }
  if (error.cause.tag === 'protocol-error' || error.cause.tag === 'internal-error') {
    return { tag: 'invalid-result', val: `${error.cause.tag}: ${error.cause.val}` };
  }
  if (error.cause.tag === 'denied') {
    return { tag: 'constraint-violation', val: error.cause.val };
  }
  if (error.cause.tag === 'cancelled') {
    return { tag: 'constraint-violation', val: 'underlying invocation was cancelled' };
  }
  if (error.cause.tag === 'resource-exhausted') {
    return { tag: 'constraint-violation', val: error.cause.val };
  }
  if (error.cause.tag !== 'tool') return error.cause;

  try {
    const payload = encodeCustomError(error.cause.error);
    preflightTypedSchemaValue(payload);
    const name = isDeclaredToolError(error.cause.error) ? error.cause.error.name : undefined;
    if (name === undefined) throw new Error('custom tool error is missing its declared name');
    return { tag: 'custom-error', val: { name, payload } };
  } catch (encodeError) {
    return { tag: 'invalid-result', val: errorMessage(encodeError) };
  }
}

export function createUnderlyingToolClient<Definition extends AnyToolDefinition>(
  definition: Definition,
  underlying: UniversalToolUnderlying,
): ToolUnderlying<Definition> {
  return createToolUnderlyingForExtendedTool(
    getExtendedToolDefinition(definition),
    {
      invoke(commandPath, input, stdin) {
        return adaptUniversalInvocation(underlying.invoke(commandPath, input, stdin));
      },
    },
    mapUnderlyingClientFailure,
  ) as ToolUnderlying<Definition>;
}

async function adaptUniversalInvocation(
  invocationPromise: ReturnType<UniversalToolUnderlying['invoke']>,
) {
  const invocation = await invocationPromise;
  let result: Promise<WireTypedSchemaValue | undefined> | undefined;
  return {
    stdout: invocation.stdout,
    stderr: invocation.stderr,
    cancel: () => invocation.cancel(),
    get result() {
      return (result ??= invocation.result.then((carrier) => carrier.result));
    },
  };
}

export interface MonomorphicToolMiddlewareInvocation {
  readonly toolName: string;
  readonly toolMetadata: WireTool;
  readonly parameters: WireTypedSchemaValue;
  readonly commandPath: readonly string[];
  readonly input: WireTypedSchemaValue;
  readonly stdin: AsyncIterable<number> | undefined;
  readonly stdout?: ToolOutputWriter;
  readonly stderr?: ToolOutputWriter;
  readonly principal: Principal;
}

export async function invokeMonomorphicToolMiddleware(
  source: MonomorphicToolMiddlewareSource,
  invocation: MonomorphicToolMiddlewareInvocation,
  raw: RawUnderlyingTool,
): Promise<WireInvocationResult> {
  const { commandPath, input, parameters, stdin: rawStdin, principal } = invocation;
  return withInvocationScopedUnderlying(
    raw,
    rawStdin,
    async (underlying, stdin) => {
      let resolved;
      try {
        resolved = resolveToolInvocation(source.presented, source.runtime, commandPath);
      } catch (error) {
        throw decodeUnderlyingToolError(error, (payload) => payload);
      }

      let prepared;
      try {
        preflightTypedSchemaValue(input);
        prepared = resolved.prepare(typedSchemaValueFromWit(input));
      } catch (error) {
        const protocolError = decodeUnderlyingToolError(error, (payload) => payload);
        throw protocolError instanceof ToolInvokeError
          ? protocolError
          : new ToolInvokeError({ tag: 'invalid-input', val: errorMessage(error) });
      }

      const body = resolved.command.body;
      if (!body) {
        throw new ToolInvokeError({ tag: 'invalid-command-path', val: [...commandPath] });
      }
      validatePresentedStdin(body, stdin);

      const context: Record<string, unknown> = {
        principal,
        parameters: decodeMiddlewareParameters(source.parameterCodec, parameters),
        underlying: createToolUnderlyingForExtendedTool(
          source.expected,
          {
            invoke(path, expectedInput, expectedStdin) {
              return adaptUniversalInvocation(
                underlying.invoke(path, expectedInput, expectedStdin),
              );
            },
          },
          mapUnderlyingClientFailure,
        ),
      };
      if (body.stdin && stdin !== undefined) context.stdin = stdin;

      let outcome: unknown;
      try {
        outcome = await prepared.invoke(context);
      } catch (error) {
        throw encodePresentedMiddlewareError(error, body, presentedCallName(source, commandPath));
      }

      try {
        return encodePresentedMiddlewareResult(
          body,
          outcome,
          presentedCallName(source, commandPath),
        );
      } catch (error) {
        await Promise.all([
          closeAsyncIterable(presentedOutcomeOutput(body, outcome, 'stdout')),
          closeAsyncIterable(presentedOutcomeOutput(body, outcome, 'stderr')),
        ]);
        if (isWireToolError(error) && error.tag === 'invalid-result') {
          throw new ToolInvokeError(error);
        }
        throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
      }
    },
    invocation.stdout,
    invocation.stderr,
  );
}

export async function invokeUniversalToolMiddleware(
  source: UniversalToolMiddlewareSource,
  invocation: UniversalToolMiddlewareInvocation & { readonly parameters: WireTypedSchemaValue },
  raw: RawUnderlyingTool,
): Promise<WireInvocationResult> {
  try {
    return await withInvocationScopedUnderlying(
      raw,
      invocation.stdin,
      (underlying, stdin) => {
        try {
          preflightTypedSchemaValue(invocation.input);
        } catch (error) {
          throw new ToolInvokeError({ tag: 'invalid-input', val: errorMessage(error) });
        }
        return source.invoke(
          stdin === undefined
            ? invocation
            : {
                toolName: invocation.toolName,
                toolMetadata: invocation.toolMetadata,
                commandPath: invocation.commandPath,
                input: invocation.input,
                stdin,
                stdout: invocation.stdout,
                stderr: invocation.stderr,
                principal: invocation.principal,
              },
          {
            underlying,
            parameters: decodeMiddlewareParameters(
              source.parameterCodec,
              invocation.parameters,
            ) as {},
          },
        );
      },
      invocation.stdout,
      invocation.stderr,
    );
  } catch (error) {
    throw encodeRawMiddlewareError(error);
  }
}

function decodeMiddlewareParameters(codec: SchemaCodec, wire: WireTypedSchemaValue): unknown {
  preflightTypedSchemaValue(wire);
  const typed = typedSchemaValueFromWit(wire);
  if (!deepEqual(typed.graph, codec.graph)) {
    throw new ToolInvokeError({
      tag: 'invalid-input',
      val: 'middleware parameter schema does not match the local definition',
    });
  }
  if (!schemaValueConforms(codec.graph, codec.graph.root, typed.value)) {
    throw new ToolInvokeError({
      tag: 'invalid-input',
      val: 'middleware parameters do not conform to the local definition',
    });
  }
  return codec.fromValue(typed.value);
}

export async function withInvocationScopedUnderlying(
  raw: RawUnderlyingTool,
  stdin: AsyncIterable<number> | undefined,
  invoke: (
    underlying: UniversalToolUnderlying,
    stdin: AsyncIterable<number> | undefined,
  ) => WireInvocationResult | Promise<WireInvocationResult>,
  stdoutWriter?: ToolOutputWriter,
  stderrWriter?: ToolOutputWriter,
): Promise<WireInvocationResult> {
  const ownership = new InvocationOwnership(stdin);
  const underlying = InvocationScopedUnderlying.create(raw, ownership);
  try {
    const pendingCarrier = invoke(underlying, ownership.stdin);
    if (!isPromiseLike(pendingCarrier)) underlying.revoke();
    const carrier = await pendingCarrier;
    underlying.revoke();
    let result: WireInvocationResult;
    try {
      result = validateInvocationResult(carrier);
    } catch (error) {
      await Promise.all([
        closeAsyncIterable(invocationOutput(carrier, 'stdout')),
        closeAsyncIterable(invocationOutput(carrier, 'stderr')),
      ]);
      throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
    }
    const { stdout, stderr } = ownership.forwardOutputs(result.stdout, result.stderr);
    const settled = await Promise.allSettled([
      stdoutWriter ? pumpMiddlewareOutput(stdout, stdoutWriter) : undefined,
      stderrWriter ? pumpMiddlewareOutput(stderr, stderrWriter) : undefined,
      underlying.cleanup(),
    ]);
    const failure = settled.find(
      (entry): entry is PromiseRejectedResult => entry.status === 'rejected',
    );
    if (failure) {
      await Promise.allSettled([closeAsyncIterable(stdout), closeAsyncIterable(stderr)]);
      throw failure.reason;
    }
    return {
      result: result.result,
      ...(stdoutWriter || stdout === undefined ? {} : { stdout }),
      ...(stderrWriter || stderr === undefined ? {} : { stderr }),
    };
  } finally {
    underlying.revoke();
    await ownership.dispose();
    await underlying.cleanup();
  }
}

async function pumpMiddlewareOutput(
  output: AsyncIterable<number> | undefined,
  writer: ToolOutputWriter,
): Promise<void> {
  try {
    const chunk: number[] = [];
    if (output) {
      for await (const byte of output) {
        chunk.push(byte);
        if (chunk.length === 16 * 1024) {
          await writer.write(Uint8Array.from(chunk));
          chunk.length = 0;
        }
      }
    }
    if (chunk.length) await writer.write(Uint8Array.from(chunk));
    await writer.finish();
  } catch (error) {
    try {
      await writer.fail({ tag: 'failed', val: errorMessage(error) });
    } catch {
      // Preserve the output pump failure.
    }
    throw error;
  }
}

class InvocationScopedUnderlying implements UniversalToolUnderlying {
  private revoked = false;
  private readonly admissions = new Set<Promise<unknown>>();
  private readonly observers = new Set<ObserverLease>();

  private constructor(
    private readonly raw: RawUnderlyingTool,
    private readonly ownership: InvocationOwnership,
  ) {}

  static create(
    raw: RawUnderlyingTool,
    ownership: InvocationOwnership,
  ): InvocationScopedUnderlying {
    return new InvocationScopedUnderlying(raw, ownership);
  }

  readonly invoke = (async (
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
  ) => {
    if (this.revoked) {
      throw new ToolUnderlyingMisuseError(
        'underlying tool is no longer available after its middleware invocation returned',
      );
    }
    const admission = Promise.resolve().then(() => {
      this.ownership.forwardStdin(stdin);
      return this.invokeRaw(commandPath, input, stdin);
    });
    this.admissions.add(admission);
    void admission.then(
      () => this.admissions.delete(admission),
      () => this.admissions.delete(admission),
    );
    return admission;
  }) as UniversalToolUnderlying['invoke'];

  async invokeAndAwait(
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
  ): Promise<WireInvocationResult> {
    const invocation = await this.invoke(commandPath, input, stdin);
    const [result, stdout, stderr] = await Promise.allSettled([
      invocation.result,
      collectAsyncIterable(invocation.stdout),
      collectAsyncIterable(invocation.stderr),
    ]);
    if (result.status === 'rejected') throw result.reason;
    if (stdout.status === 'rejected') throw stdout.reason;
    if (stderr.status === 'rejected') throw stderr.reason;
    const replay = this.ownership.trackOutputs({
      stdout: stdout.value === undefined ? undefined : replayAsyncIterable(stdout.value),
      stderr: stderr.value === undefined ? undefined : replayAsyncIterable(stderr.value),
    });
    return {
      result: result.value.result,
      ...(replay.stdout === undefined ? {} : { stdout: replay.stdout }),
      ...(replay.stderr === undefined ? {} : { stderr: replay.stderr }),
    };
  }

  revoke(): void {
    this.revoked = true;
  }

  async cleanup(): Promise<void> {
    await Promise.allSettled(this.admissions);
    for (const observer of this.observers) observer.release();
    this.observers.clear();
  }

  private async invokeRaw(
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
  ) {
    try {
      const [observer, stdout, stderr] = await this.raw.invoke(
        [...commandPath],
        input,
        stdin === undefined ? undefined : encodeByteStream(stdin),
      );
      const lease = new ObserverLease(observer);
      this.observers.add(lease);
      const decodedStdout = stdout === undefined ? undefined : decodeByteStream(stdout);
      const decodedStderr = stderr === undefined ? undefined : decodeByteStream(stderr);
      try {
        const tracked = this.ownership.trackOutputs({
          stdout: decodedStdout,
          stderr: decodedStderr,
        });
        let result: Promise<WireInvocationResult> | undefined;
        return {
          stdout: tracked.stdout,
          stderr: tracked.stderr,
          cancel: () => lease.cancel(),
          get result() {
            return (result ??= lease.observe().then(
              (structured) => {
                try {
                  return validateInvocationResult({ result: structured });
                } catch (error) {
                  throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
                }
              },
              (error) => Promise.reject(decodeUnderlyingTerminal(error)),
            ));
          },
        };
      } catch (error) {
        await Promise.all([closeAsyncIterable(decodedStdout), closeAsyncIterable(decodedStderr)]);
        throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
      }
    } catch (error) {
      throw decodeUnderlyingToolError(error, (payload) => payload);
    }
  }
}

class ObserverLease {
  private result: Promise<WireTypedSchemaValue | undefined> | undefined;
  private settled = false;
  private released = false;
  private disposed = false;

  constructor(private readonly observer: UnderlyingInvokeResult) {}

  observe(): Promise<WireTypedSchemaValue | undefined> {
    if (this.result !== undefined) return this.result;
    if (this.released) {
      return Promise.reject(
        new ToolUnderlyingMisuseError('underlying invocation observer was released'),
      );
    }
    return (this.result ??= this.observer.get().finally(() => {
      this.settled = true;
      if (this.released) this.dispose();
    }));
  }

  cancel(): void {
    if (!this.released) this.observer.cancel();
  }

  release(): void {
    this.released = true;
    if (this.result === undefined || this.settled) this.dispose();
  }

  private dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    const disposable = this.observer as typeof this.observer & { [Symbol.dispose]?: () => void };
    disposable[Symbol.dispose]?.();
  }
}

function encodeByteStream(source: AsyncIterable<number>): AsyncIterable<ByteStreamItem> {
  const iterator = source[Symbol.asyncIterator]();
  return {
    [Symbol.asyncIterator]() {
      return {
        async next(): Promise<IteratorResult<ByteStreamItem>> {
          const next = await iterator.next();
          return next.done
            ? { done: true, value: undefined }
            : { done: false, value: { tag: 'ok', val: Uint8Array.of(next.value) } };
        },
        async return() {
          await iterator.return?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
}

export function decodeByteStream(source: AsyncIterable<ByteStreamItem>): AsyncIterable<number> {
  const iterator = source[Symbol.asyncIterator]();
  let chunk: Uint8Array | undefined;
  let offset = 0;
  return {
    [Symbol.asyncIterator]() {
      return {
        async next(): Promise<IteratorResult<number>> {
          while (chunk === undefined || offset === chunk.byteLength) {
            const next = await iterator.next();
            if (next.done) return { done: true, value: undefined };
            if (next.value.tag === 'err') throw next.value.val;
            chunk = next.value.val;
            offset = 0;
          }
          return { done: false, value: chunk[offset++] };
        },
        async return() {
          await iterator.return?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
}

function decodeUnderlyingTerminal(error: unknown): unknown {
  const terminal = error as UnderlyingError;
  if (terminal?.tag === 'tool-error') {
    return decodeUnderlyingToolError(terminal.val, (payload) => payload);
  }
  if (
    terminal?.tag === 'protocol-error' ||
    terminal?.tag === 'denied' ||
    terminal?.tag === 'internal-error' ||
    terminal?.tag === 'cancelled' ||
    terminal?.tag === 'resource-exhausted'
  ) {
    return new ToolInvokeError(terminal);
  }
  return error;
}

class InvocationOwnership {
  private readonly transferredStreams = new Set<AsyncIterable<number>>();
  private readonly outputs = new Map<AsyncIterable<number>, TrackedOutputStream>();
  private outerStdinTransferred = false;
  private forwardedOutputs = 0;
  private disposed = false;
  readonly stdin: TrackedOutputStream | undefined;

  constructor(stdin: AsyncIterable<number> | undefined) {
    this.stdin = stdin === undefined ? undefined : new TrackedOutputStream(stdin);
  }

  forwardStdin(stdin: AsyncIterable<number> | undefined): void {
    this.transfer(stdin);
  }

  trackOutputs(result: WireInvocationResult): WireInvocationResult {
    const track = (output: AsyncIterable<number> | undefined) => {
      if (output === undefined) return undefined;
      const tracked = new TrackedOutputStream(output);
      this.outputs.set(tracked, tracked);
      if (this.disposed) void tracked.dispose().catch(() => undefined);
      return tracked;
    };
    return {
      result: result.result,
      stdout: track(result.stdout),
      stderr: track(result.stderr),
    };
  }

  forwardOutputs(
    stdout: AsyncIterable<number> | undefined,
    stderr: AsyncIterable<number> | undefined,
  ): Pick<WireInvocationResult, 'stdout' | 'stderr'> {
    if (stdout !== undefined) this.assertTransferable(stdout);
    if (stderr !== undefined) {
      if (stderr === stdout) throw new ToolUnderlyingMisuseError('stream was already transferred');
      this.assertTransferable(stderr);
    }
    const forward = (output: AsyncIterable<number> | undefined) => {
      if (output === undefined) return undefined;
      this.transfer(output);
      this.forwardedOutputs += 1;
      return new TrackedOutputStream(output, async () => {
        this.forwardedOutputs -= 1;
        if (this.forwardedOutputs === 0) await this.disposeUntransferredStreams();
      });
    };
    return { stdout: forward(stdout), stderr: forward(stderr) };
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    if (this.forwardedOutputs === 0) await this.disposeUntransferredStreams();
  }

  private async disposeUntransferredStreams(): Promise<void> {
    await Promise.allSettled([
      this.outerStdinTransferred ? undefined : this.stdin?.dispose(),
      ...Array.from(this.outputs.values(), (stream) => stream.dispose()),
    ]);
  }

  private transfer(stream: AsyncIterable<number> | undefined): void {
    if (stream === undefined) return;
    this.assertTransferable(stream);
    this.transferredStreams.add(stream);
    if (stream === this.stdin) this.outerStdinTransferred = true;
    this.outputs.get(stream)?.transfer();
  }

  private assertTransferable(stream: AsyncIterable<number>): void {
    if (this.transferredStreams.has(stream)) {
      throw new ToolUnderlyingMisuseError('stream was already transferred');
    }
  }
}

class TrackedOutputStream implements AsyncIterableIterator<number> {
  private iterator: AsyncIterator<number> | undefined;
  private closed = false;
  private disposed = false;
  private finalized = false;
  private transferred = false;

  constructor(
    private readonly output: AsyncIterable<number>,
    private readonly onClose?: () => Promise<void>,
  ) {}

  [Symbol.asyncIterator](): AsyncIterableIterator<number> {
    return this;
  }

  async next(): Promise<IteratorResult<number>> {
    if (this.disposed) {
      throw new ToolUnderlyingMisuseError('stream is no longer available after disposal');
    }
    if (this.closed) return { done: true, value: undefined };
    try {
      const result = await this.getIterator().next();
      if (result.done) {
        this.closed = true;
        await this.finalize();
      }
      return result;
    } catch (error) {
      this.closed = true;
      try {
        await this.iterator?.return?.();
      } catch {
        // Preserve the stream read failure.
      }
      await this.finalize();
      throw error;
    }
  }

  async return(): Promise<IteratorResult<number>> {
    if (this.closed) return { done: true, value: undefined };
    this.closed = true;
    try {
      return (await this.getIterator().return?.()) ?? { done: true, value: undefined };
    } finally {
      await this.finalize();
    }
  }

  async throw(error?: unknown): Promise<IteratorResult<number>> {
    if (this.closed) throw error;
    let iterator: AsyncIterator<number>;
    try {
      iterator = this.getIterator();
    } catch (failure) {
      this.closed = true;
      await this.finalize();
      throw failure;
    }
    if (!iterator.throw) {
      this.closed = true;
      try {
        await iterator.return?.();
      } finally {
        await this.finalize();
      }
      throw error;
    }
    try {
      const result = await iterator.throw(error);
      if (result.done) {
        this.closed = true;
        await this.finalize();
      }
      return result;
    } catch (failure) {
      this.closed = true;
      try {
        await iterator.return?.();
      } catch {
        // Preserve the stream failure.
      }
      await this.finalize();
      throw failure;
    }
  }

  transfer(): void {
    this.transferred = true;
  }

  async dispose(): Promise<void> {
    if (this.closed || this.transferred) return;
    this.disposed = true;
    await closeAsyncIterable(this);
  }

  private getIterator(): AsyncIterator<number> {
    return (this.iterator ??= this.output[Symbol.asyncIterator]());
  }

  private async finalize(): Promise<void> {
    if (this.finalized) return;
    this.finalized = true;
    await this.onClose?.();
  }
}

function mapUnderlyingClientFailure(error: unknown, context: ToolClientFailureContext): unknown {
  switch (context.phase) {
    case 'input':
      return new ToolInvokeError({ tag: 'invalid-input', val: errorMessage(error) });
    case 'result':
      if (error instanceof ToolInvokeError) {
        return decodeUnderlyingToolError(error, (payload) =>
          decodeDeclaredToolError(context.body, payload, context.callName),
        );
      }
      return new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
    case 'invoke':
      return decodeUnderlyingToolError(error, (payload) =>
        decodeDeclaredToolError(context.body, payload, context.callName),
      );
  }
}

function isPromiseLike(value: unknown): value is PromiseLike<unknown> {
  return isObject(value) && typeof (value as { then?: unknown }).then === 'function';
}

function validatePresentedStdin(
  body: ExtendedCommandBody,
  stdin: AsyncIterable<number> | undefined,
): void {
  if (!body.stdin && stdin !== undefined) {
    throw new ToolInvokeError({
      tag: 'invalid-input',
      val: 'tool invocation contained an unexpected stdin stream',
    });
  }
  if (body.stdin?.required && stdin === undefined) {
    throw new ToolInvokeError({
      tag: 'invalid-input',
      val: 'tool invocation did not contain declared stdin stream',
    });
  }
}

function encodePresentedMiddlewareResult(
  body: ExtendedCommandBody,
  outcome: unknown,
  callName: string,
): WireInvocationResult {
  if (!body.stdout && !body.stderr) {
    if (!body.result) {
      if (outcome !== undefined) {
        throw new Error('unit middleware command returned a structured result');
      }
      return {};
    }
    return { result: encodeToolValue(body.result.codec, outcome, `${callName} result`) };
  }

  let result: unknown;
  let stdout: unknown;
  let stderr: unknown;
  if (body.result || body.stderr) {
    if (
      !isObject(outcome) ||
      Array.isArray(outcome) ||
      (body.result && !hasOwn(outcome, 'result'))
    ) {
      throw new Error(
        'middleware command with named outputs or a structured result must return an object',
      );
    }
    for (const key of Object.keys(outcome)) {
      if (key !== 'result' && key !== 'stdout' && key !== 'stderr') {
        throw new Error(`middleware command returned unexpected result field "${key}"`);
      }
    }
    result = outcome.result;
    stdout = outcome.stdout;
    stderr = outcome.stderr;
  } else {
    stdout = outcome;
  }

  if (stdout !== undefined && !isAsyncIterable(stdout)) {
    throw new Error('middleware stdout must be an async iterable');
  }
  if (stdout !== undefined && !body.stdout) {
    throw new Error('middleware command returned undeclared stdout');
  }
  if (body.stdout?.required && stdout === undefined) {
    throw new Error('required middleware stdout stream is missing');
  }
  if (stderr !== undefined && !isAsyncIterable(stderr)) {
    throw new Error('middleware stderr must be an async iterable');
  }
  if (stderr !== undefined && !body.stderr) {
    throw new Error('middleware command returned undeclared stderr');
  }
  if (body.stderr?.required && stderr === undefined) {
    throw new Error('required middleware stderr stream is missing');
  }

  return {
    result: body.result
      ? encodeToolValue(body.result.codec, result, `${callName} result`)
      : undefined,
    stdout,
    stderr,
  };
}

function encodePresentedMiddlewareError(
  error: unknown,
  body: ExtendedCommandBody,
  callName: string,
): unknown {
  const encoded = encodeToolInvokeError(error, (declaredError) =>
    encodeDeclaredMiddlewareError(body, declaredError, callName),
  );
  return encoded.tag === 'custom-error'
    ? new ToolInvokeError({
        tag: 'unknown-error',
        name: encoded.val.name,
        payload: encoded.val.payload,
      })
    : new ToolInvokeError(encoded);
}

function encodeRawMiddlewareError(error: unknown): unknown {
  if (error instanceof ToolInvokeError && error.cause.tag === 'tool') {
    const raw = error.cause.error;
    if (isCustomToolError(raw)) {
      error = new ToolInvokeError({
        tag: 'unknown-error',
        name: raw.name,
        payload: raw.payload,
      });
    }
  }
  const encoded = encodeToolInvokeError(error, () => {
    throw new Error('raw middleware custom error is missing its named envelope');
  });
  return encoded.tag === 'custom-error'
    ? new ToolInvokeError({
        tag: 'unknown-error',
        name: encoded.val.name,
        payload: encoded.val.payload,
      })
    : new ToolInvokeError(encoded);
}

function encodeDeclaredMiddlewareError(
  body: ExtendedCommandBody,
  error: unknown,
  callName: string,
): WireTypedSchemaValue {
  if (!isDeclaredToolError(error)) {
    throw new Error('middleware returned an invalid declared error');
  }
  const errorCase = body.errors.find((candidate) => candidate.name === error.name);
  if (!errorCase) {
    throw new Error(`middleware returned undeclared error "${error.name}"`);
  }
  return encodeDeclaredToolErrorPayload(
    errorCase,
    error,
    `middleware error "${error.name}"`,
    `${callName} custom error "${error.name}"`,
  );
}

function presentedCallName(
  source: MonomorphicToolMiddlewareSource,
  commandPath: readonly string[],
): string {
  return [source.presented.toolName, ...commandPath].join(' ');
}

function presentedOutcomeOutput(
  body: ExtendedCommandBody,
  outcome: unknown,
  channel: 'stdout' | 'stderr',
): AsyncIterable<number> | undefined {
  try {
    if (channel === 'stdout' && body.stdout && !body.result && !body.stderr) {
      return isAsyncIterable(outcome) ? outcome : undefined;
    }
    const output = isObject(outcome) ? outcome[channel] : undefined;
    return isAsyncIterable(output) ? output : undefined;
  } catch {
    return undefined;
  }
}

function isWireToolError(value: unknown): value is WireToolError {
  if (!isObject(value) || typeof value.tag !== 'string') return false;
  if (value.tag === 'custom-error') return true;
  if (!hasOwn(value, 'val')) return false;
  switch (value.tag) {
    case 'invalid-tool-name':
    case 'invalid-input':
    case 'constraint-violation':
    case 'invalid-result':
      return typeof value.val === 'string';
    case 'invalid-command-path':
      return isDenseStringList(value.val);
    default:
      return false;
  }
}

function isUnknownToolError(value: unknown): value is {
  readonly tag: 'unknown-error';
  readonly name: string;
  readonly payload: WireTypedSchemaValue;
} {
  return isObject(value) && value.tag === 'unknown-error';
}

function isCustomToolError(
  value: unknown,
): value is Extract<WireToolError, { readonly tag: 'custom-error' }>['val'] {
  return isObject(value) && typeof value.name === 'string' && isObject(value.payload);
}

function isObject(value: unknown): value is Record<PropertyKey, unknown> {
  return (typeof value === 'object' && value !== null) || typeof value === 'function';
}

function hasOwn(value: object, key: PropertyKey): boolean {
  return Object.prototype.hasOwnProperty.call(value, key);
}

function isDenseStringList(value: unknown): value is string[] {
  if (!Array.isArray(value)) return false;
  for (let index = 0; index < value.length; index++) {
    if (!(index in value) || typeof value[index] !== 'string') return false;
  }
  return true;
}

function validateInvocationResult(value: unknown): WireInvocationResult {
  if (!isObject(value) || Array.isArray(value)) {
    throw new Error('tool invocation result must be an object');
  }
  const result = value.result as WireTypedSchemaValue | undefined;
  const stdout = value.stdout;
  const stderr = value.stderr;
  if (result !== undefined) {
    preflightTypedSchemaValue(result);
  }
  if (stdout !== undefined && !isAsyncIterable(stdout)) {
    throw new Error('tool invocation stdout must be an async iterable');
  }
  if (stderr !== undefined && !isAsyncIterable(stderr)) {
    throw new Error('tool invocation stderr must be an async iterable');
  }
  return { result, stdout, stderr };
}

function invocationOutput(
  value: unknown,
  channel: 'stdout' | 'stderr',
): AsyncIterable<number> | undefined {
  try {
    if (!isObject(value) || Array.isArray(value)) return undefined;
    return isAsyncIterable(value[channel]) ? value[channel] : undefined;
  } catch {
    return undefined;
  }
}

function preflightTypedSchemaValue(value: WireTypedSchemaValue): void {
  try {
    preflightWitTypedSchemaValue(value);
    const validationValue = typedSchemaValueFromWit({
      graph: value.graph,
      value: {
        ...value.value,
        valueNodes: value.value.valueNodes.map((node) => {
          if (!isObject(node) || Array.isArray(node)) return node;
          switch (node.tag) {
            case 'secret-value':
            case 'quota-token-handle':
            case 'permission-card-handle':
              return { ...node, val: {} as never };
            default:
              return { ...node };
          }
        }),
      },
    });
    if (
      !schemaValueConforms(validationValue.graph, validationValue.graph.root, validationValue.value)
    ) {
      throw new Error('typed schema value does not conform to its schema graph');
    }
  } catch (error) {
    const tree = isObject(value) ? value.value : undefined;
    const nodes = isObject(tree) ? tree.valueNodes : undefined;
    if (Array.isArray(nodes)) {
      drainUnconsumedQuotaAndPermissionCardHandles(
        nodes as Parameters<typeof drainUnconsumedQuotaAndPermissionCardHandles>[0],
      );
    }
    throw error;
  }
}

function errorMessage(error: unknown): string {
  if (isWireToolError(error) && typeof error.val === 'string') return error.val;
  return error instanceof Error ? error.message : String(error);
}
