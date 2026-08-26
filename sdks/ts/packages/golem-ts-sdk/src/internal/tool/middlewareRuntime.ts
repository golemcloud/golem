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
  UnderlyingTool as WireUnderlyingTool,
} from 'golem:tool/common@0.1.0';
import {
  drainUnconsumedQuotaAndPermissionCardHandles,
  preflightWitTypedSchemaValue,
  typedSchemaValueFromWit,
} from '../schema-model';
import { schemaValueConforms } from './validation';
import type { ExtendedCommandBody } from './model';
import { closeAsyncIterable, isAsyncIterable } from './asyncIterable';
import {
  encodeDeclaredToolErrorPayload,
  encodeToolValue,
  isDeclaredToolError,
} from './invocationResult';
import {
  createToolClient,
  createToolClientForExtendedTool,
  decodeDeclaredToolError,
  ToolInvokeError,
  type AnyToolDefinition,
  type ToolClient,
  type ToolClientFailureContext,
  type UniversalToolMiddlewareInvocation,
  type UniversalToolUnderlying,
} from '../../tool';
import type { Principal } from '../../principal';
import type {
  MonomorphicToolMiddlewareSource,
  UniversalToolMiddlewareSource,
} from '../registry/toolMiddlewareRegistry';
import { resolveToolInvocation } from '../registry/toolRegistry';

type RawUnderlyingTool = Pick<WireUnderlyingTool, 'invoke'>;

export class ToolUnderlyingMisuseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ToolUnderlyingMisuseError';
  }
}

export function decodeUnderlyingToolError<Errors>(
  error: unknown,
  decodeCustomError: (payload: WireTypedSchemaValue) => Errors,
): unknown {
  const cause =
    error instanceof ToolInvokeError ? error.cause : isWireToolError(error) ? error : null;
  if (cause === null) return error;
  if (cause.tag !== 'tool' && cause.tag !== 'custom-error') {
    return error instanceof ToolInvokeError ? error : new ToolInvokeError(cause);
  }

  try {
    const payload = cause.tag === 'tool' ? cause.error : cause.val;
    preflightTypedSchemaValue(payload as WireTypedSchemaValue);
    return ToolInvokeError.tool(decodeCustomError(payload as WireTypedSchemaValue));
  } catch (decodeError) {
    return new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(decodeError) });
  }
}

export function encodeToolInvokeError<Errors>(
  error: unknown,
  encodeCustomError: (error: Errors) => WireTypedSchemaValue,
): WireToolError {
  if (!(error instanceof ToolInvokeError)) throw error;
  if (error.cause.tag !== 'tool') return error.cause;

  try {
    const payload = encodeCustomError(error.cause.error);
    preflightTypedSchemaValue(payload);
    return { tag: 'custom-error', val: payload };
  } catch (encodeError) {
    return { tag: 'invalid-result', val: errorMessage(encodeError) };
  }
}

export function createUnderlyingToolClient<Definition extends AnyToolDefinition>(
  definition: Definition,
  underlying: UniversalToolUnderlying,
): ToolClient<Definition> {
  return createToolClient(
    definition,
    {
      invokeAndAwait(commandPath, input, stdin) {
        return underlying.invoke(commandPath, input, stdin);
      },
    },
    mapUnderlyingClientFailure,
  );
}

export interface MonomorphicToolMiddlewareInvocation {
  readonly toolName: string;
  readonly toolMetadata: WireTool;
  readonly commandPath: readonly string[];
  readonly input: WireTypedSchemaValue;
  readonly stdin: AsyncIterable<number> | undefined;
  readonly principal: Principal;
}

export async function invokeMonomorphicToolMiddleware(
  source: MonomorphicToolMiddlewareSource,
  invocation: MonomorphicToolMiddlewareInvocation,
  raw: RawUnderlyingTool,
): Promise<WireInvocationResult> {
  const { commandPath, input, stdin, principal } = invocation;
  return withInvocationScopedUnderlying(raw, stdin, async (underlying) => {
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
      underlying: createToolClientForExtendedTool(
        source.expected,
        {
          invokeAndAwait(path, expectedInput, expectedStdin) {
            return underlying.invoke(path, expectedInput, expectedStdin);
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
      return encodePresentedMiddlewareResult(body, outcome, presentedCallName(source, commandPath));
    } catch (error) {
      await closeAsyncIterable(presentedOutcomeStdout(body, outcome));
      if (isWireToolError(error) && error.tag === 'invalid-result') {
        throw new ToolInvokeError(error);
      }
      throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
    }
  });
}

export async function invokeUniversalToolMiddleware(
  source: UniversalToolMiddlewareSource,
  invocation: UniversalToolMiddlewareInvocation,
  raw: RawUnderlyingTool,
): Promise<WireInvocationResult> {
  try {
    return await withInvocationScopedUnderlying(raw, invocation.stdin, (underlying) => {
      try {
        preflightTypedSchemaValue(invocation.input);
      } catch (error) {
        throw new ToolInvokeError({ tag: 'invalid-input', val: errorMessage(error) });
      }
      return source.invoke(invocation, { underlying });
    });
  } catch (error) {
    throw encodeRawMiddlewareError(error);
  }
}

export async function withInvocationScopedUnderlying(
  raw: RawUnderlyingTool,
  stdin: AsyncIterable<number> | undefined,
  invoke: (
    underlying: UniversalToolUnderlying,
  ) => WireInvocationResult | Promise<WireInvocationResult>,
): Promise<WireInvocationResult> {
  const ownership = new InvocationOwnership(stdin);
  const underlying = InvocationScopedUnderlying.create(raw, ownership);
  try {
    const result = await invoke(underlying);
    try {
      validateInvocationResult(result);
    } catch (error) {
      await closeAsyncIterable(invocationStdout(result));
      throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
    }
    ownership.forwardStdout(result.stdout);
    return result;
  } finally {
    await underlying.revoke();
    await ownership.dispose();
  }
}

class InvocationScopedUnderlying implements UniversalToolUnderlying {
  private revoked = false;
  private inFlight = false;
  private activeInvocation: Promise<unknown> | undefined;

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
  ): Promise<WireInvocationResult> => {
    if (this.revoked) {
      throw new ToolUnderlyingMisuseError(
        'underlying tool is no longer available after its middleware invocation returned',
      );
    }
    if (this.inFlight) {
      throw new ToolUnderlyingMisuseError('an underlying tool invocation is already in progress');
    }

    this.inFlight = true;
    const invocation = Promise.resolve().then(() => {
      if (this.revoked) {
        throw new ToolUnderlyingMisuseError(
          'underlying tool is no longer available after its middleware invocation returned',
        );
      }
      this.ownership.forwardStdin(stdin);
      return this.invokeRaw(commandPath, input, stdin);
    });
    this.activeInvocation = invocation;
    const settled = () => {
      if (this.activeInvocation === invocation) this.activeInvocation = undefined;
      this.inFlight = false;
    };
    void invocation.then(settled, settled);
    return invocation;
  }) as UniversalToolUnderlying['invoke'];

  async revoke(): Promise<void> {
    this.revoked = true;
    await this.activeInvocation?.catch(() => undefined);
  }

  private async invokeRaw(
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: AsyncIterable<number> | undefined,
  ): Promise<WireInvocationResult> {
    try {
      const result = await this.raw.invoke([...commandPath], input, stdin);
      if (!isObject(result) || Array.isArray(result)) {
        throw new ToolInvokeError({
          tag: 'invalid-result',
          val: 'tool invocation result must be an object',
        });
      }
      const tracked = this.ownership.trackStdout(result);
      try {
        validateInvocationResult(tracked);
      } catch (error) {
        throw new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
      }
      return tracked;
    } catch (error) {
      throw decodeUnderlyingToolError(error, (payload) => payload);
    }
  }
}

class InvocationOwnership {
  private readonly transferredStreams = new Set<AsyncIterable<number>>();
  private readonly stdout = new Map<AsyncIterable<number>, TrackedOutputStream>();
  private outerStdinTransferred = false;

  constructor(private readonly outerStdin: AsyncIterable<number> | undefined) {}

  forwardStdin(stdin: AsyncIterable<number> | undefined): void {
    this.transfer(stdin);
  }

  trackStdout(result: WireInvocationResult): WireInvocationResult {
    if (!isAsyncIterable(result.stdout)) return result;
    const tracked = new TrackedOutputStream(result.stdout);
    this.stdout.set(tracked, tracked);
    return { ...result, stdout: tracked };
  }

  forwardStdout(stdout: AsyncIterable<number> | undefined): void {
    this.transfer(stdout);
  }

  async dispose(): Promise<void> {
    await Promise.allSettled([
      this.outerStdinTransferred ? undefined : closeAsyncIterable(this.outerStdin),
      ...Array.from(this.stdout.values(), (stream) => stream.dispose()),
    ]);
  }

  private transfer(stream: AsyncIterable<number> | undefined): void {
    if (stream === undefined) return;
    if (this.transferredStreams.has(stream)) {
      throw new ToolUnderlyingMisuseError('stream was already transferred');
    }
    this.transferredStreams.add(stream);
    if (stream === this.outerStdin) this.outerStdinTransferred = true;
    this.stdout.get(stream)?.transfer();
  }
}

class TrackedOutputStream implements AsyncIterableIterator<number> {
  private readonly iterator: AsyncIterator<number>;
  private transferred = false;
  private closed = false;

  constructor(stdout: AsyncIterable<number>) {
    this.iterator = stdout[Symbol.asyncIterator]();
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<number> {
    return this;
  }

  async next(): Promise<IteratorResult<number>> {
    const result = await this.iterator.next();
    if (result.done) this.closed = true;
    return result;
  }

  async return(): Promise<IteratorResult<number>> {
    if (this.closed) return { done: true, value: undefined };
    this.closed = true;
    return (await this.iterator.return?.()) ?? { done: true, value: undefined };
  }

  async throw(error?: unknown): Promise<IteratorResult<number>> {
    if (this.closed) throw error;
    this.closed = true;
    if (this.iterator.throw) return this.iterator.throw(error);
    await this.iterator.return?.();
    throw error;
  }

  transfer(): void {
    this.transferred = true;
  }

  async dispose(): Promise<void> {
    if (!this.transferred) await closeAsyncIterable(this);
  }
}

function mapUnderlyingClientFailure(error: unknown, context: ToolClientFailureContext): unknown {
  switch (context.phase) {
    case 'input':
      return new ToolInvokeError({ tag: 'invalid-input', val: errorMessage(error) });
    case 'result':
      return new ToolInvokeError({ tag: 'invalid-result', val: errorMessage(error) });
    case 'invoke':
      return decodeUnderlyingToolError(error, (payload) =>
        decodeDeclaredToolError(context.body, payload, context.callName),
      );
  }
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
  if (!body.stdout) {
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
  if (body.result) {
    if (!isObject(outcome) || Array.isArray(outcome) || !hasOwn(outcome, 'result')) {
      throw new Error(
        'middleware command with a structured result and stdout must return an object',
      );
    }
    for (const key of Object.keys(outcome)) {
      if (key !== 'result' && key !== 'stdout') {
        throw new Error(`middleware command returned unexpected result field "${key}"`);
      }
    }
    result = outcome.result;
    stdout = outcome.stdout;
  } else {
    stdout = outcome;
  }

  if (stdout !== undefined && !isAsyncIterable(stdout)) {
    throw new Error('middleware stdout must be an async iterable');
  }
  if (body.stdout.required && stdout === undefined) {
    throw new Error('required middleware stdout stream is missing');
  }

  return {
    result: body.result
      ? encodeToolValue(body.result.codec, result, `${callName} result`)
      : undefined,
    stdout,
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
    ? ToolInvokeError.tool(encoded.val)
    : new ToolInvokeError(encoded);
}

function encodeRawMiddlewareError(error: unknown): unknown {
  const encoded = encodeToolInvokeError(error, (payload) => payload as WireTypedSchemaValue);
  return encoded.tag === 'custom-error'
    ? ToolInvokeError.tool(encoded.val)
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

function presentedOutcomeStdout(
  body: ExtendedCommandBody,
  outcome: unknown,
): AsyncIterable<number> | undefined {
  try {
    if (!body.stdout) return isAsyncIterable(outcome) ? outcome : undefined;
    const stdout = body.result && isObject(outcome) ? outcome.stdout : outcome;
    return isAsyncIterable(stdout) ? stdout : undefined;
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

function validateInvocationResult(value: unknown): asserts value is WireInvocationResult {
  if (!isObject(value) || Array.isArray(value)) {
    throw new Error('tool invocation result must be an object');
  }
  if (value.result !== undefined) {
    preflightTypedSchemaValue(value.result as WireTypedSchemaValue);
  }
  if (value.stdout !== undefined && !isAsyncIterable(value.stdout)) {
    throw new Error('tool invocation stdout must be an async iterable');
  }
}

function invocationStdout(value: unknown): AsyncIterable<number> | undefined {
  try {
    if (!isObject(value) || Array.isArray(value)) return undefined;
    return isAsyncIterable(value.stdout) ? value.stdout : undefined;
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
