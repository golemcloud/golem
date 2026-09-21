// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import {
  createStdinFromStream,
  getAllTools,
  getTool,
  ToolRpc,
  type ByteStreamItem,
  type RegisteredTool,
} from 'golem:tool/host@0.1.0';
import type {
  CommandBody,
  CommandNode,
  Constraint,
  Doc,
  OptionSpec,
  Ref as ToolRef,
  StreamSpec,
} from 'golem:tool/common@0.1.0';
import {
  field,
  freezeSchemaGraph,
  schemaGraphRootsFromWit,
  schemaShapesMatch,
  schemaValueEquals,
  schemaValueFromWit,
  t,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
  type TypedSchemaValue,
  v,
} from './internal/schema-model';
import { SchemaRef, type JsonValue } from './schema/ref';
import { ComponentId } from './ids';
import {
  createToolClientRuntime,
  createToolClientTransport,
  isRpcError,
  mapSettledToolResult,
  startedToolInvocation,
  type StartedToolInvocation,
  type ToolClientRuntime,
  type ToolInputStream,
} from './bridge/tool';
import { ToolCallError } from './toolClient';

/** A declared tool argument with its selected value schema. */
export interface ToolArgument {
  readonly kind: 'positional' | 'tail' | 'option' | 'flag';
  readonly name: string;
  readonly aliases: readonly string[];
  readonly short?: string;
  readonly required: boolean;
  readonly default?: SchemaValue;
  readonly optionalCarrier?: true;
  readonly schema: SchemaRef;
}

/** An invalid structured result returned by a remote tool. */
export class ToolRemoteOutputError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ToolRemoteOutputError';
  }
}

/** A declared custom tool error decoded using discovered metadata. */
export interface ReflectedToolFailure {
  readonly name: string;
  readonly value?: JsonValue;
}

/** One callable command in an immutable discovered tool snapshot. */
export class ToolCommand {
  readonly path: readonly string[];
  readonly name: string;
  readonly aliases: readonly string[];
  readonly doc: Doc;
  readonly inputSchema?: SchemaRef;
  readonly arguments: readonly ToolArgument[];
  readonly constraints: readonly Constraint[];
  readonly stdin?: StreamSpec;
  readonly stdout?: StreamSpec;
  readonly result?: SchemaRef;
  readonly errors: readonly { readonly name: string; readonly payload?: SchemaRef }[];
  readonly children: readonly ToolCommand[];
  private readonly wireInputGraph?: SchemaGraph;

  constructor(
    readonly toolName: string,
    node: CommandNode,
    body: CommandBody | undefined,
    path: readonly string[],
    inherited: readonly ToolArgument[],
    graph: SchemaGraph,
    typeAt: (index: number) => SchemaType,
    children: readonly ToolCommand[],
    private readonly runtime: ToolClientRuntime,
  ) {
    this.path = Object.freeze([...path]);
    this.name = node.name;
    this.aliases = Object.freeze([...node.aliases]);
    this.doc = node.doc;
    this.children = Object.freeze([...children]);
    const globals = [
      ...node.globals.options.map((option) => optionArgument(option, graph, typeAt)),
      ...node.globals.flags.map((flag) => flagArgument(flag, graph)),
    ];
    const local =
      body === undefined
        ? []
        : [
            ...body.positionals.fixed.map((item): ToolArgument => {
              const root =
                item.required || item.default_ !== undefined
                  ? typeAt(item.type)
                  : t.option(typeAt(item.type));
              return Object.freeze({
                kind: 'positional',
                name: item.name,
                aliases: [],
                required: item.required,
                default:
                  item.default_ === undefined ? undefined : schemaValueFromWit(item.default_),
                optionalCarrier:
                  !item.required && item.default_ === undefined ? (true as const) : undefined,
                schema: SchemaRef.fromImmutableGraph(graph, root),
              });
            }),
            ...(body.positionals.tail
              ? [
                  Object.freeze({
                    kind: 'tail' as const,
                    name: body.positionals.tail.name,
                    aliases: [] as readonly string[],
                    required: body.positionals.tail.min > 0,
                    schema: SchemaRef.fromImmutableGraph(
                      graph,
                      t.list(typeAt(body.positionals.tail.itemType)),
                    ),
                  }),
                ]
              : []),
            ...body.options.map((option) => optionArgument(option, graph, typeAt)),
            ...body.flags.map((flag) => flagArgument(flag, graph)),
          ];
    const localNames = new Set(local.flatMap((arg) => [arg.name, ...arg.aliases]));
    this.arguments = Object.freeze(
      [...inherited, ...globals]
        .filter((arg) => ![arg.name, ...arg.aliases].some((name) => localNames.has(name)))
        .concat(local),
    );
    this.constraints = Object.freeze([...(body?.constraints ?? [])]);
    this.stdin = body?.stdin;
    this.stdout = body?.stdout;
    this.result = body?.result
      ? SchemaRef.fromImmutableGraph(graph, typeAt(body.result.type))
      : undefined;
    this.errors = Object.freeze(
      (body?.errors ?? []).map((error) =>
        Object.freeze({
          name: error.name,
          payload:
            error.payload === undefined
              ? undefined
              : SchemaRef.fromImmutableGraph(graph, typeAt(error.payload)),
        }),
      ),
    );
    this.inputSchema = body
      ? SchemaRef.fromImmutableGraph(
          graph,
          t.record(this.arguments.map((arg) => field(arg.name, arg.schema.root))),
        )
      : undefined;
    this.wireInputGraph = this.inputSchema?.graph;
    Object.freeze(this);
  }

  /** Validate and pack canonical JSON input before opening a tool RPC. */
  packJson(input: JsonValue): SchemaValue {
    if (!this.inputSchema) throw new TypeError(`Command '${this.path.join(' ')}' has no body`);
    const value = this.inputSchema.packJson(input);
    if (!this.inputSchema.validateValue(value).success) throw new TypeError('Invalid tool input');
    this.validateConstraints(value);
    return value;
  }

  /** Validate canonical JSON and command constraints without dispatching. */
  validateJson(input: JsonValue): ReturnType<SchemaRef['validateJson']> {
    try {
      return { success: true, value: this.packJson(input) };
    } catch (error) {
      return {
        success: false,
        issues: [{ path: [], message: error instanceof Error ? error.message : String(error) }],
      };
    }
  }

  private validateConstraints(value: SchemaValue): void {
    if (value.tag !== 'record') throw new TypeError('Tool input must be a record');
    const values = new Map(this.arguments.map((arg, index) => [arg.name, value.fields[index]]));
    const matches = (reference: ToolRef): boolean => {
      const name = reference.tag === 'present' ? reference.val : reference.val.name;
      const argument = this.arguments.find(
        (arg) => arg.name === name || arg.aliases.includes(name),
      );
      if (!argument) throw new TypeError(`Unknown tool constraint argument '${name}'`);
      const actual = values.get(argument.name)!;
      if (reference.tag === 'value-is')
        return valueMatches(actual, schemaValueFromWit(reference.val.value));
      return valuePresent(actual, argument.default, argument.kind === 'flag');
    };
    const quantify = (refs: readonly ToolRef[], quantifier: 'all' | 'any'): boolean =>
      quantifier === 'all' ? refs.every(matches) : refs.some(matches);
    this.constraints.forEach((constraint, index) => {
      const satisfied = (() => {
        switch (constraint.tag) {
          case 'requires-all':
            return quantify(constraint.val, 'all');
          case 'requires-any':
            return quantify(constraint.val, 'any');
          case 'all-or-none': {
            const count = constraint.val.filter(matches).length;
            return count === 0 || count === constraint.val.length;
          }
          case 'mutex-groups':
            return constraint.val.filter((group) => quantify(group.refs, 'all')).length <= 1;
          case 'implies':
            return (
              !quantify(constraint.val.lhs, constraint.val.lhsQuant) ||
              quantify(constraint.val.rhs, constraint.val.rhsQuant)
            );
          case 'forbids':
            return (
              !quantify(constraint.val.lhs, constraint.val.lhsQuant) ||
              !quantify(constraint.val.rhs, 'any')
            );
        }
      })();
      if (!satisfied) throw new TypeError(`Tool command constraint ${index} is not satisfied`);
    });
  }

  private inputValue(value: SchemaValue): TypedSchemaValue {
    if (!this.inputSchema) throw new TypeError(`Command '${this.path.join(' ')}' has no body`);
    if (!this.inputSchema.validateValue(value).success) throw new TypeError('Invalid tool input');
    this.validateConstraints(value);
    return { graph: this.wireInputGraph!, value };
  }

  private decodeResult(value: { readonly result?: TypedSchemaValue }): SchemaValue | undefined {
    if (!this.result) {
      if (value.result !== undefined)
        throw new ToolRemoteOutputError('Remote tool returned an unexpected result');
      return undefined;
    }
    if (
      value.result === undefined ||
      !schemaShapesMatch(this.result.graph, value.result.graph) ||
      !this.result.validateValue(value.result.value).success
    ) {
      throw new ToolRemoteOutputError('Remote tool returned a missing or malformed result');
    }
    return value.result.value;
  }

  private unpackResult(value: SchemaValue): JsonValue {
    try {
      return this.result!.unpackJson(value);
    } catch (cause) {
      throw new ToolRemoteOutputError(
        `Remote tool result is not canonical JSON: ${cause instanceof Error ? cause.message : String(cause)}`,
      );
    }
  }

  /** Start a cancelable invocation with a schema-native input. */
  startValue(
    input: SchemaValue,
    stdin?: ToolInputStream,
  ): StartedToolInvocation<SchemaValue | undefined> {
    if (this.stdin?.required && !stdin) throw new TypeError('Command requires stdin');
    const encoded = this.inputValue(input);
    const started = this.runtime.start(this.path, encoded, stdin, this.stdout !== undefined);
    const settled = mapSettledToolResult(
      started.settledResult,
      (value) => this.decodeResult(value),
      (error) => this.mapFailure(error),
    );
    return startedToolInvocation(started.stdout ?? emptyStdout(), settled, started.cancel);
  }

  /** Start a cancelable invocation with canonical JSON input. */
  startJson(
    input: JsonValue,
    stdin?: ToolInputStream,
  ): StartedToolInvocation<JsonValue | undefined> {
    const started = this.startValue(this.packJson(input), stdin);
    const unpackResult = (value: SchemaValue | undefined) =>
      value === undefined ? undefined : this.unpackResult(value);
    return {
      stdout: started.stdout,
      get result() {
        return started.result.then(unpackResult);
      },
      cancel: () => started.cancel(),
      collect: async () => {
        const collected = await started.collect();
        return {
          result: unpackResult(collected.result),
          stdout: collected.stdout,
        };
      },
    };
  }

  /** Await a structured schema-native result. */
  invokeValue(input: SchemaValue, stdin?: ToolInputStream): Promise<SchemaValue | undefined> {
    if (this.stdout?.required)
      return Promise.reject(
        new TypeError('Command requires caller-readable stdout; use startValue'),
      );
    const started = this.startValue(input, stdin);
    return started.collect().then((collected) => collected.result);
  }

  /** Await a structured canonical JSON result. */
  async invokeJson(input: JsonValue, stdin?: ToolInputStream): Promise<JsonValue | undefined> {
    const value = await this.invokeValue(this.packJson(input), stdin);
    return value === undefined ? undefined : this.unpackResult(value);
  }

  /** Admit a fire-and-forget invocation without a stdout attachment. */
  triggerValue(input: SchemaValue, stdin?: ToolInputStream): void {
    if (this.stdout?.required) throw new TypeError('Command requires caller-readable stdout');
    if (this.stdin?.required && !stdin) throw new TypeError('Command requires stdin');
    const encoded = typedSchemaValueToWit(this.inputValue(input));
    try {
      const rpc = ToolRpc.create(this.toolName);
      rpc.invoke(
        [...this.path],
        encoded,
        stdin ? createStdinFromStream(byteItems(stdin)) : undefined,
      );
    } catch (error) {
      throw this.mapFailure(error);
    }
  }

  /** Admit a fire-and-forget invocation from canonical JSON. */
  triggerJson(input: JsonValue, stdin?: ToolInputStream): void {
    this.triggerValue(this.packJson(input), stdin);
  }

  private mapFailure(error: unknown): unknown {
    if (!isRpcError(error)) return error;
    if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error') {
      return new ToolCallError({ tag: 'rpc', error });
    }
    const custom = error.val.val;
    const declared = this.errors.find((candidate) => candidate.name === custom.name);
    if (!declared)
      return new ToolCallError({
        tag: 'unknown-error',
        name: custom.name,
        payload: custom.payload,
      });
    try {
      const payload = typedSchemaValueFromWit(custom.payload);
      if (!declared.payload) {
        if (payload.value.tag !== 'tuple' || payload.value.elements.length !== 0) {
          throw new ToolRemoteOutputError('Custom error has an unexpected payload');
        }
        return new ToolCallError({ tag: 'tool', error: { name: custom.name } });
      }
      if (
        !schemaShapesMatch(declared.payload.graph, payload.graph) ||
        !declared.payload.validateValue(payload.value).success
      ) {
        throw new ToolRemoteOutputError('Custom error payload does not match its declaration');
      }
      return new ToolCallError({
        tag: 'tool',
        error: {
          name: custom.name,
          value: declared.payload.unpackJson(payload.value),
        },
      });
    } catch (cause) {
      return cause;
    }
  }
}

/** An immutable snapshot of one accessible tool registration. */
export class ToolType {
  readonly name: string;
  readonly lookupName: string;
  readonly version: string;
  readonly implementedBy: ComponentId;
  readonly commands: readonly ToolCommand[];
  readonly client: ReflectedToolClient;

  constructor(registered: RegisteredTool, runtime?: ToolClientRuntime) {
    const snapshot = immutableSnapshot(registered);
    const raw = snapshot.definition;
    if (raw.commands.nodes.length === 0) throw new TypeError('Tool has no root command');
    this.name = raw.commands.nodes[0].name;
    this.lookupName = registered.lookupName;
    this.version = raw.version;
    this.implementedBy = ComponentId.from(registered.implementedBy);
    const decoded = schemaGraphRootsFromWit(
      raw.schema,
      raw.schema.typeNodes.map((_, index) => index),
    );
    const graph = freezeSchemaGraph({ defs: decoded.defs, root: decoded.roots[raw.schema.root] });
    const typeAt = (index: number): SchemaType => {
      const type = decoded.roots[index];
      if (!type) throw new TypeError(`Tool schema node ${index} is missing`);
      return type;
    };
    const transport =
      runtime ??
      createToolClientRuntime(this.lookupName, createToolClientTransport(this.lookupName, true));
    const visited = new Set<number>();
    const build = (
      index: number,
      path: readonly string[],
      inherited: readonly ToolArgument[],
    ): ToolCommand => {
      if (visited.has(index)) throw new TypeError('Tool command tree has a cycle or shared child');
      const node = raw.commands.nodes[index];
      if (!node) throw new TypeError(`Tool command index ${index} is missing`);
      visited.add(index);
      const globals = [
        ...node.globals.options.map((option) => optionArgument(option, graph, typeAt)),
        ...node.globals.flags.map((flag) => flagArgument(flag, graph)),
      ];
      const children = node.subcommands.map((child) =>
        build(child, [...path, raw.commands.nodes[child]?.name ?? ''], [...inherited, ...globals]),
      );
      return new ToolCommand(
        this.lookupName,
        node,
        node.body,
        path,
        inherited,
        graph,
        typeAt,
        children,
        transport,
      );
    };
    this.commands = Object.freeze([build(0, [], [])]);
    if (visited.size !== raw.commands.nodes.length)
      throw new TypeError('Tool has unreachable commands');
    this.client = Object.freeze({
      command: (path: readonly string[]) => {
        const command = this.command(path);
        if (!command || !command.inputSchema)
          throw new TypeError(`Unknown callable command '${path.join(' ')}'`);
        return command;
      },
    });
    Object.freeze(this);
  }

  /** Resolve names and aliases to a canonical command handle. */
  command(path: readonly string[]): ToolCommand | undefined {
    let current = this.commands[0];
    for (const segment of path) {
      current = current.children.find(
        (child) => child.name === segment || child.aliases.includes(segment),
      )!;
      if (!current) return undefined;
    }
    return current;
  }
}

function immutableSnapshot<T>(value: T): T {
  if (Array.isArray(value)) return Object.freeze(value.map(immutableSnapshot)) as T;
  if (value !== null && typeof value === 'object') {
    const copy = Object.fromEntries(
      Object.entries(value).map(([key, child]) => [key, immutableSnapshot(child)]),
    );
    return Object.freeze(copy) as T;
  }
  return value;
}

/** Strict callable lookup for a discovered tool. */
export interface ReflectedToolClient {
  command(path: readonly string[]): ToolCommand;
}

/** List the current accessible tools. */
export function getAllToolTypes(): ToolType[] {
  return getAllTools().map((registered) => new ToolType(registered));
}

/** Look up one accessible tool; missing and inaccessible are indistinguishable. */
export function getToolType(name: string): ToolType | undefined {
  const tool = getTool(name);
  return tool === undefined ? undefined : new ToolType(tool);
}

/** A fully dynamic client that sends caller-owned packed values. */
export class DynamicToolClient {
  private readonly runtime: ToolClientRuntime;
  constructor(
    readonly name: string,
    runtime?: ToolClientRuntime,
  ) {
    this.runtime = runtime ?? createToolClientRuntime(name, createToolClientTransport(name, true));
  }

  start(
    path: readonly string[],
    input: TypedSchemaValue,
    stdin?: ToolInputStream,
    stdout = false,
  ): StartedToolInvocation<TypedSchemaValue | undefined> {
    const started = this.runtime.start(path, input, stdin, stdout);
    const settled = mapSettledToolResult(started.settledResult, (result) => result.result);
    return startedToolInvocation(started.stdout ?? emptyStdout(), settled, started.cancel);
  }

  invoke(
    path: readonly string[],
    input: TypedSchemaValue,
    stdin?: ToolInputStream,
  ): Promise<TypedSchemaValue | undefined> {
    const started = this.start(path, input, stdin);
    return started.stdout.cancel().then(() => started.result);
  }

  trigger(path: readonly string[], input: TypedSchemaValue, stdin?: ToolInputStream): void {
    const encoded = typedSchemaValueToWit(input);
    const rpc = ToolRpc.create(this.name);
    rpc.invoke([...path], encoded, stdin ? createStdinFromStream(byteItems(stdin)) : undefined);
  }
}

function optionArgument(
  option: OptionSpec,
  graph: SchemaGraph,
  typeAt: (index: number) => SchemaType,
): ToolArgument {
  const root =
    option.shape.tag === 'repeatable-list'
      ? t.list(typeAt(option.shape.val.itemType))
      : option.shape.tag === 'repeatable-map'
        ? typeAt(option.shape.val.mapType)
        : typeAt(option.shape.val);
  const optional =
    !option.required &&
    option.default_ === undefined &&
    option.shape.tag !== 'repeatable-list' &&
    option.shape.tag !== 'repeatable-map';
  return Object.freeze({
    kind: 'option',
    name: option.long,
    aliases: Object.freeze([...option.aliases]),
    short: option.short,
    required: option.required,
    default: option.default_ === undefined ? undefined : schemaValueFromWit(option.default_),
    optionalCarrier: optional ? (true as const) : undefined,
    schema: SchemaRef.fromImmutableGraph(graph, optional ? t.option(root) : root),
  });
}

function flagArgument(
  flag: CommandNode['globals']['flags'][number],
  graph: SchemaGraph,
): ToolArgument {
  return Object.freeze({
    kind: 'flag',
    name: flag.long,
    aliases: Object.freeze([...flag.aliases]),
    short: flag.short,
    required: false,
    default: flag.shape.tag === 'bool-flag' ? v.bool(flag.shape.val.default_) : v.u32(0),
    schema: SchemaRef.fromImmutableGraph(
      graph,
      flag.shape.tag === 'bool-flag' ? t.bool() : t.u32(),
    ),
  });
}

function valuePresent(value: SchemaValue, defaultValue?: SchemaValue, flag = false): boolean {
  if (flag) return defaultValue !== undefined && !schemaValueEquals(value, defaultValue);
  if (defaultValue !== undefined && schemaValueEquals(value, defaultValue)) return false;
  switch (value.tag) {
    case 'option':
      return value.value !== undefined;
    case 'list':
    case 'fixed-list':
      return value.elements.length > 0;
    case 'map':
      return value.entries.length > 0;
    case 'bool':
      return value.value;
    case 'u32':
      return value.value !== 0;
    default:
      return true;
  }
}

function valueMatches(value: SchemaValue, expected: SchemaValue): boolean {
  if (schemaValueEquals(value, expected)) return true;
  switch (value.tag) {
    case 'option':
      return value.value !== undefined && valueMatches(value.value, expected);
    case 'list':
    case 'fixed-list':
      return value.elements.some((item) => valueMatches(item, expected));
    case 'map':
      return value.entries.some((entry) => valueMatches(entry.value, expected));
    default:
      return false;
  }
}

async function* emptyStdout(): AsyncIterable<never> {}

async function* byteItems(source: ToolInputStream): AsyncIterable<ByteStreamItem> {
  for await (const value of source) {
    if (!(value instanceof Uint8Array)) throw new TypeError('Tool stdin yielded a non-byte chunk');
    if (value.byteLength === 0) throw new TypeError('Tool stdin yielded an empty chunk');
    yield { tag: 'ok', val: value };
  }
}
