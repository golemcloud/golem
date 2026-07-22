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
  CommandAnnotations,
  Doc,
  DuplicateKeyPolicy,
  Formatter,
  Quantifier,
  Repetition,
  StreamSpec,
} from 'golem:tool/common@0.1.0';
import {
  ToolRpc,
  type RpcError,
  type ToolError,
  type TypedSchemaValue as WireTypedSchemaValue,
} from 'golem:tool/host@0.1.0';
import type { InputStream, OutputStream } from 'wasi:io/streams@0.2.3';
import type { Principal } from '../principal';
import {
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedConstraint,
  type ExtendedErrorCase,
  type ExtendedGlobals,
  type ExtendedOptionShape,
  type ExtendedOptionSpec,
  type ExtendedRef,
  type ExtendedResultSpec,
  type ExtendedToolRuntime,
  type SubtreeForward,
  type ToolHelpArgument,
  type ToolHelpProjection,
  ExtendedToolType,
  codecValue,
  emptyDoc,
  emptyGlobals,
  emptyPositionals,
  graftSubtree,
  listCodec,
  normalizeExtendedTool,
  parseSourceValue,
  schemaValueConforms,
  toolBuildError,
} from '../internal/tool';
import {
  deepEqual,
  t,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  type TypedSchemaValue,
} from '../internal/schema-model';
import { disposeWitResource } from '../internal/pollableUtils';
import { ToolRegistry } from '../internal/registry/toolRegistry';
import { compileSchema } from './schema/adapter';
import type { FluentCodec } from './schema/codec';
import type { StandardSchemaV1 } from './schema/standardSchema';

export type CamelCase<Name extends string> = Name extends `${infer Head}-${infer Tail}`
  ? `${Head}${Capitalize<CamelCase<Tail>>}`
  : Name;

type Simplify<Value> = { [Key in keyof Value]: Value[Key] } & {};
type SchemaOutput<Schema extends StandardSchemaV1> = StandardSchemaV1.InferOutput<Schema>;
type SuccessOutput<Schema extends StandardSchemaV1> = [SchemaOutput<Schema>] extends [void]
  ? undefined
  : SchemaOutput<Schema>;
type StreamPresence = 'none' | 'optional' | 'required';
declare const TAIL_ARGUMENT: unique symbol;
declare const GLOBAL_SURFACES: unique symbol;
declare const BODY_SURFACES: unique symbol;

type AddArgument<Args, Name extends string, Value, Required extends boolean> = Args &
  (Required extends true
    ? { [Key in CamelCase<Name>]: Value }
    : { [Key in CamelCase<Name>]?: Value });
interface ArgumentSurface<Name extends string = string, Aliases extends string = string> {
  readonly name: Name;
  readonly aliases: Aliases;
}
type GlobalEntries<Globals> = Globals extends {
  readonly [GLOBAL_SURFACES]: infer Entries;
}
  ? Entries
  : never;
type GlobalArguments<Globals> = Omit<Globals, typeof GLOBAL_SURFACES>;
type OptionAliases<Options> = Options extends { readonly aliases: readonly (infer Alias)[] }
  ? Extract<Alias, string>
  : never;
type AddGlobalArgument<
  Globals,
  Name extends string,
  Value,
  Required extends boolean,
  Options,
> = AddArgument<GlobalArguments<Globals>, Name, Value, Required> & {
  readonly [GLOBAL_SURFACES]:
    | GlobalEntries<Globals>
    | ArgumentSurface<Name, OptionAliases<Options>>;
};
type SurfaceIdentifiers<Entry> =
  Entry extends ArgumentSurface<infer Name, infer Aliases> ? Name | Aliases : never;
type CapturedGlobalNames<ChildGlobals, InheritedGlobals> =
  GlobalEntries<ChildGlobals> extends infer Entry
    ? Entry extends ArgumentSurface<infer Name, string>
      ? Extract<
          SurfaceIdentifiers<Entry>,
          SurfaceIdentifiers<GlobalEntries<InheritedGlobals>>
        > extends never
        ? never
        : CamelCase<Name>
      : never
    : never;
type UncapturedGlobalEntries<ChildGlobals, InheritedGlobals> =
  GlobalEntries<ChildGlobals> extends infer Entry
    ? Entry extends ArgumentSurface
      ? Extract<
          SurfaceIdentifiers<Entry>,
          SurfaceIdentifiers<GlobalEntries<InheritedGlobals>>
        > extends never
        ? Entry
        : never
      : never
    : never;
type ReconcileGlobalArguments<ChildGlobals, InheritedGlobals> = Omit<
  GlobalArguments<ChildGlobals>,
  CapturedGlobalNames<ChildGlobals, InheritedGlobals>
> & {
  readonly [GLOBAL_SURFACES]: UncapturedGlobalEntries<ChildGlobals, InheritedGlobals>;
};
type MergeGlobalArguments<InheritedGlobals, LocalGlobals> = GlobalArguments<InheritedGlobals> &
  GlobalArguments<LocalGlobals> & {
    readonly [GLOBAL_SURFACES]: GlobalEntries<InheritedGlobals> | GlobalEntries<LocalGlobals>;
  };
type BodySurfaceEntries<Args> = Args extends {
  readonly [BODY_SURFACES]: infer Entries;
}
  ? Entries
  : never;
type BodyArguments<Args> = Omit<Args, typeof BODY_SURFACES>;
type AddBodyArgument<
  Args,
  Name extends string,
  Value,
  Required extends boolean,
  Aliases extends string = never,
> = AddArgument<BodyArguments<Args>, Name, Value, Required> & {
  readonly [BODY_SURFACES]: BodySurfaceEntries<Args> | ArgumentSurface<Name, Aliases>;
};
type CapturedBodyNames<Args, InheritedGlobals> =
  BodySurfaceEntries<Args> extends infer Entry
    ? Entry extends ArgumentSurface<infer Name, string>
      ? Extract<
          SurfaceIdentifiers<Entry>,
          SurfaceIdentifiers<GlobalEntries<InheritedGlobals>>
        > extends never
        ? never
        : CamelCase<Name>
      : never
    : never;
type UncapturedBodyEntries<Args, InheritedGlobals> =
  BodySurfaceEntries<Args> extends infer Entry
    ? Entry extends ArgumentSurface
      ? Extract<
          SurfaceIdentifiers<Entry>,
          SurfaceIdentifiers<GlobalEntries<InheritedGlobals>>
        > extends never
        ? Entry
        : never
      : never
    : never;
type ReconcileBodyArguments<Args, InheritedGlobals> = Omit<
  Args,
  CapturedBodyNames<Args, InheritedGlobals> | typeof BODY_SURFACES
> & {
  readonly [BODY_SURFACES]: UncapturedBodyEntries<Args, InheritedGlobals>;
};
type TailArgumentKey<Args> = Args extends { readonly [TAIL_ARGUMENT]: infer Key }
  ? Extract<Key, PropertyKey>
  : never;
type NonTailBodyEntries<Args> =
  BodySurfaceEntries<Args> extends infer Entry
    ? Entry extends ArgumentSurface<infer Name, string>
      ? CamelCase<Name> extends TailArgumentKey<Args>
        ? never
        : Entry
      : never
    : never;
type ReplaceTailArgument<Args, Name extends string, Value> = Omit<
  Args,
  TailArgumentKey<Args> | typeof TAIL_ARGUMENT | typeof BODY_SURFACES
> & { [Key in CamelCase<Name>]: Value } & {
  readonly [TAIL_ARGUMENT]: CamelCase<Name>;
  readonly [BODY_SURFACES]: NonTailBodyEntries<Args> | ArgumentSurface<Name, never>;
};
type PublicArguments<Args> = Omit<Args, typeof TAIL_ARGUMENT | typeof BODY_SURFACES>;

type HasDefault<Options> = Options extends { readonly default: unknown } ? true : false;
type IsRequired<Options, Default extends boolean> = Options extends {
  readonly required: infer Required;
}
  ? Required extends true
    ? true
    : Required extends false
      ? false
      : Default
  : Default;
type IsRepeatable<Options> = Options extends { readonly repeatable: RepeatableMode } ? true : false;
type OptionIsRequired<Options> =
  IsRepeatable<Options> extends true
    ? true
    : HasDefault<Options> extends true
      ? true
      : IsRequired<Options, false>;
type PositionalIsRequired<Options> =
  HasDefault<Options> extends true ? true : IsRequired<Options, true>;
type FlagValue<Options> = Options extends { readonly kind: 'count-flag' } ? number : boolean;
type SchemaProducesMap<Schema extends StandardSchemaV1> =
  SchemaOutput<Schema> extends ReadonlyMap<unknown, unknown>
    ? true
    : Schema extends { readonly keyType: unknown; readonly valueType: unknown }
      ? true
      : false;
type OptionValue<Schema extends StandardSchemaV1, Options> =
  IsRepeatable<Options> extends true
    ? SchemaProducesMap<Schema> extends true
      ? SchemaOutput<Schema>
      : SchemaOutput<Schema>[]
    : Options extends { readonly optionalScalar: true }
      ? HasDefault<Options> extends true
        ? SchemaOutput<Schema>
        : SchemaOutput<Schema> | undefined
      : SchemaOutput<Schema>;
type StreamState<Options> = Options extends { readonly required: true } ? 'required' : 'optional';

export interface ToolBodyModel<
  Args = {},
  Success = undefined,
  Errors = never,
  Stdin extends StreamPresence = 'none',
  Stdout extends StreamPresence = 'none',
> {
  readonly args: Args;
  readonly success: Success;
  readonly errors: Errors;
  readonly stdin: Stdin;
  readonly stdout: Stdout;
}

type AnyToolBodyModel = ToolBodyModel<any, any, any, any, any>;

export interface ToolCommandModel<
  Name extends string = string,
  Globals = {},
  Body extends AnyToolBodyModel | undefined = AnyToolBodyModel | undefined,
  Children = {},
> {
  readonly name: Name;
  readonly globals: Globals;
  readonly body: Body;
  readonly children: Children;
}

export interface ToolSubtreeModel<Command = ToolCommandModel> {
  readonly ownership: 'subtree';
  readonly command: Command;
}

type BodyModelOf<Builder> =
  Builder extends BodyBuilder<infer Args, infer Success, infer Errors, infer Stdin, infer Stdout>
    ? ToolBodyModel<Args, Success, Errors, Stdin, Stdout>
    : never;

export type ToolCommandModelOf<Builder> =
  Builder extends CommandBuilder<infer Name, infer Globals, infer Body, infer Children, boolean>
    ? ToolCommandModel<Name, Globals, Body, Children>
    : never;

type BodyArgs<Body> =
  Body extends ToolBodyModel<infer Args, unknown, unknown, any, any> ? PublicArguments<Args> : {};
type BodySuccess<Body> =
  Body extends ToolBodyModel<unknown, infer Success, unknown, any, any> ? Success : never;
type BodyErrors<Body> =
  Body extends ToolBodyModel<unknown, unknown, infer Errors, any, any> ? Errors : never;
type BodyStdin<Body> =
  Body extends ToolBodyModel<unknown, unknown, unknown, infer Stdin, any> ? Stdin : 'none';
type BodyStdout<Body> =
  Body extends ToolBodyModel<unknown, unknown, unknown, any, infer Stdout> ? Stdout : 'none';
type ReconcileBodyModel<Body, InheritedGlobals> =
  Body extends ToolBodyModel<infer Args, infer Success, infer Errors, infer Stdin, infer Stdout>
    ? ToolBodyModel<ReconcileBodyArguments<Args, InheritedGlobals>, Success, Errors, Stdin, Stdout>
    : Body;

type StreamContextField<
  Name extends string,
  Presence extends StreamPresence,
  Stream,
> = Presence extends 'required'
  ? { readonly [Key in Name]: Stream }
  : Presence extends 'optional'
    ? { readonly [Key in Name]?: Stream }
    : {};

export type ToolInvocationContext<
  Stdin extends StreamPresence = StreamPresence,
  Stdout extends StreamPresence = StreamPresence,
> = Simplify<
  { readonly principal: Principal } & StreamContextField<
    'stdin',
    Stdin,
    ReadableStream<Uint8Array>
  > &
    StreamContextField<'stdout', Stdout, WritableStream<Uint8Array>>
>;

export interface ToolOk<Value> {
  readonly tag: 'ok';
  readonly value: Value;
}

export type ToolErr<Name extends string, Payload = never> = [Payload] extends [never]
  ? {
      readonly tag: 'err';
      readonly name: Name;
      readonly hasPayload: false;
      readonly payload?: never;
    }
  : {
      readonly tag: 'err';
      readonly name: Name;
      readonly hasPayload: true;
      readonly payload: Payload;
    };

export type ToolResult<Success, Errors = never> = ToolOk<Success> | Errors;

export function ok<Value>(value: Value): ToolOk<Value> {
  return { tag: 'ok', value };
}

export function err<const Name extends string>(name: Name): ToolErr<Name>;
export function err<const Name extends string, Payload>(
  name: Name,
  payload: Payload,
): ToolErr<Name, Payload>;
export function err(name: string, payload?: unknown): ToolErr<string, unknown> | ToolErr<string> {
  return arguments.length === 1
    ? { tag: 'err', name, hasPayload: false }
    : { tag: 'err', name, hasPayload: true, payload };
}

export type ToolHandler<Args, Success, Errors, Context> = (
  args: Simplify<Args>,
  context: Context,
) => ToolResult<Success, Errors> | Promise<ToolResult<Success, Errors>>;

const COMMAND_IMPLEMENTATION = Symbol('golem.tool.commandImplementation');

export type NestedCommandImplementation<Body, Children> = Children & {
  readonly [COMMAND_IMPLEMENTATION]: { readonly body: Body; readonly receiver: Children };
};

export function command<const Children extends object>(
  children: Children,
): NestedCommandImplementation<undefined, Children>;
export function command<Body, const Children extends object>(
  body: Body,
  children: Children,
): NestedCommandImplementation<Body, Children>;
export function command<Body, Children extends object>(
  bodyOrChildren: Body | Children,
  maybeChildren?: Children,
): NestedCommandImplementation<Body | undefined, Children> {
  const body = maybeChildren === undefined ? undefined : (bodyOrChildren as Body);
  const children = (maybeChildren === undefined ? bodyOrChildren : maybeChildren) as Children;
  const result = Object.create(
    Object.getPrototypeOf(children),
    Object.getOwnPropertyDescriptors(children),
  ) as NestedCommandImplementation<Body | undefined, Children>;
  Object.defineProperty(result, COMMAND_IMPLEMENTATION, {
    value: { body, receiver: children },
    enumerable: false,
  });
  return result;
}

type HandlerFor<Model, Inherited> =
  Model extends ToolCommandModel<string, infer Globals, infer Body, object>
    ? Body extends AnyToolBodyModel
      ? ToolHandler<
          GlobalArguments<MergeGlobalArguments<Inherited, Globals>> & BodyArgs<Body>,
          BodySuccess<Body>,
          BodyErrors<Body>,
          ToolInvocationContext<BodyStdin<Body>, BodyStdout<Body>>
        >
      : never
    : never;

type ChildImplementations<Children, Inherited> = {
  [Name in keyof Children as Children[Name] extends ToolSubtreeModel
    ? never
    : Name]: NodeImplementation<Children[Name], Inherited>;
} & {
  [Name in keyof Children as Children[Name] extends ToolSubtreeModel ? Name : never]?: never;
};

type NodeImplementation<Model, Inherited> =
  Model extends ToolCommandModel<string, infer Globals, infer Body, infer Children>
    ? keyof Children extends never
      ? Body extends AnyToolBodyModel
        ? HandlerFor<Model, Inherited>
        : NestedCommandImplementation<undefined, {}>
      : Body extends AnyToolBodyModel
        ? NestedCommandImplementation<
            HandlerFor<Model, Inherited>,
            ChildImplementations<Children, MergeGlobalArguments<Inherited, Globals>>
          >
        : NestedCommandImplementation<
            undefined,
            ChildImplementations<Children, MergeGlobalArguments<Inherited, Globals>>
          >
    : never;

type RootImplementation<Model> =
  Model extends ToolCommandModel<infer Name, infer Globals, infer Body, infer Children>
    ? Simplify<
        (Body extends AnyToolBodyModel ? { [Key in Name]: HandlerFor<Model, {}> } : {}) &
          ChildImplementations<Children, Globals>
      >
    : never;

export type ToolImplementation<Definition> = RootImplementation<ToolCommandModelOf<Definition>>;

declare const TOOL_CLIENT_ERRORS: unique symbol;

export interface ToolClientMethod<Args, Result, Errors = never> {
  (args: Simplify<Args>): Promise<Result>;
  readonly [TOOL_CLIENT_ERRORS]: Errors;
}

export type ToolClientErrors<Method> = Method extends {
  readonly [TOOL_CLIENT_ERRORS]: infer Errors;
}
  ? Errors
  : never;

type ClientStdin<Body> = StreamContextField<'stdin', BodyStdin<Body>, InputStream>;

type ClientResult<Body> =
  BodyStdout<Body> extends 'required'
    ? [BodySuccess<Body>] extends [undefined]
      ? OutputStream
      : { result: BodySuccess<Body>; stdout: OutputStream }
    : BodyStdout<Body> extends 'optional'
      ? [BodySuccess<Body>] extends [undefined]
        ? OutputStream | undefined
        : { result: BodySuccess<Body>; stdout?: OutputStream }
      : [BodySuccess<Body>] extends [undefined]
        ? void
        : BodySuccess<Body>;

type ClientMethodFor<Model, Inherited> =
  Model extends ToolCommandModel<string, infer Globals, infer Body, object>
    ? Body extends AnyToolBodyModel
      ? ToolClientMethod<
          GlobalArguments<MergeGlobalArguments<Inherited, Globals>> &
            BodyArgs<Body> &
            ClientStdin<Body>,
          ClientResult<Body>,
          BodyErrors<Body>
        >
      : never
    : never;

type ChildClients<Children, Inherited> = {
  [Name in keyof Children]: NodeClient<Children[Name], Inherited>;
};

type NodeClient<Model, Inherited, Reconcile extends boolean = false> =
  Model extends ToolSubtreeModel<infer Command>
    ? NodeClient<Command, Inherited, true>
    : Model extends ToolCommandModel<string, infer Globals, infer Body, infer Children>
      ? NodeClientWithGlobals<
          Model,
          Inherited,
          Reconcile extends true ? ReconcileGlobalArguments<Globals, Inherited> : Globals,
          Reconcile extends true ? ReconcileBodyModel<Body, Inherited> : Body,
          Children
        >
      : never;

type NodeClientWithGlobals<Model, Inherited, Globals, Body, Children> =
  Model extends ToolCommandModel<infer Name, unknown, AnyToolBodyModel | undefined, object>
    ? Body extends AnyToolBodyModel
      ? ClientMethodFor<ToolCommandModel<Name, Globals, Body, Children>, Inherited> &
          ChildClients<Children, MergeGlobalArguments<Inherited, Globals>>
      : ChildClients<Children, MergeGlobalArguments<Inherited, Globals>>
    : never;

type RootClient<Model> =
  Model extends ToolCommandModel<infer Name, infer Globals, infer Body, infer Children>
    ? (Body extends AnyToolBodyModel ? { [Key in Name]: ClientMethodFor<Model, {}> } : {}) &
        ChildClients<Children, Globals>
    : never;

export type ToolClient<Definition> = Simplify<RootClient<ToolCommandModelOf<Definition>>>;

export interface ToolClientInvocationResult {
  readonly result?: WireTypedSchemaValue;
  readonly stdout?: OutputStream;
}

/** Raw WIT invocation seam used by typed tool clients. */
export interface ToolClientTransport {
  invokeAndAwait(
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: InputStream | undefined,
  ): ToolClientInvocationResult | Promise<ToolClientInvocationResult>;
}

export interface ToolClientOptions {
  readonly transport?: ToolClientTransport;
}

export type ToolCallErrorCause<Errors> =
  | { readonly tag: 'rpc'; readonly error: RpcError }
  | { readonly tag: 'tool'; readonly error: Errors };

/** A stable rejected-promise error for remote tool calls. */
export class ToolCallError<Errors = never> extends Error {
  readonly cause: ToolCallErrorCause<Errors>;

  constructor(cause: ToolCallErrorCause<Errors>) {
    super(formatToolCallError(cause));
    this.name = 'ToolCallError';
    this.cause = cause;
  }
}

export interface ImplementedTool<Name extends string = string> {
  readonly name: Name;
}

export type ToolHelpError =
  | {
      readonly tag: 'invalid-command-path';
      readonly commandPath: readonly string[];
    }
  | {
      readonly tag: 'invalid-argument-name';
      readonly commandPath: readonly string[];
      readonly argumentName: string;
    };

export type ToolHelpResult =
  | { readonly tag: 'ok'; readonly value: string }
  | { readonly tag: 'err'; readonly error: ToolHelpError };

export type DocInput = string | Partial<Doc>;
export type RepeatableMode = 'repeated' | 'delimited' | 'either';

interface SurfaceOptions {
  readonly short?: string;
  readonly aliases?: readonly string[];
  readonly doc?: DocInput;
  readonly env?: string;
}

interface ValueOptions extends SurfaceOptions {
  readonly valueName?: string;
  readonly required?: boolean;
}

type RepeatableDefault<Value, MapLike extends boolean> = MapLike extends true ? Value : Value[];

export type OptionOptions<
  Value,
  MapLike extends boolean = Value extends ReadonlyMap<unknown, unknown> ? true : false,
> =
  | (ValueOptions & {
      readonly repeatable?: undefined;
      readonly delim?: never;
      readonly default?: Value;
      readonly optionalScalar?: boolean;
      readonly duplicateKeyPolicy?: never;
    })
  | (ValueOptions & {
      readonly repeatable: 'repeated';
      readonly delim?: never;
      readonly default?: RepeatableDefault<Value, MapLike>;
      readonly optionalScalar?: never;
      readonly duplicateKeyPolicy?: DuplicateKeyPolicy;
    })
  | (ValueOptions & {
      readonly repeatable: 'delimited' | 'either';
      readonly delim: string;
      readonly default?: RepeatableDefault<Value, MapLike>;
      readonly optionalScalar?: never;
      readonly duplicateKeyPolicy?: DuplicateKeyPolicy;
    });

export interface PositionalOptions<Value> {
  readonly doc?: DocInput;
  readonly valueName?: string;
  readonly required?: boolean;
  readonly default?: Value;
  readonly acceptsStdio?: boolean;
}

export interface TailOptions {
  readonly doc?: DocInput;
  readonly valueName?: string;
  readonly min?: number;
  readonly max?: number;
  readonly separator?: string;
  readonly verbatim?: boolean;
  readonly acceptsStdio?: boolean;
}

interface BooleanFlagOptions extends SurfaceOptions {
  readonly kind?: 'flag';
  readonly default?: boolean;
  readonly negatable?: boolean;
  readonly max?: never;
}

interface CountFlagOptions extends SurfaceOptions {
  readonly kind: 'count-flag';
  readonly max?: number;
  readonly default?: never;
  readonly negatable?: never;
}

export type FlagOptions = BooleanFlagOptions | CountFlagOptions;

export type GlobalValueOptions<Value, MapLike extends boolean = false> = OptionOptions<
  Value,
  MapLike
> & {
  readonly kind?: 'option';
};

export type GlobalFlagOptions = Omit<BooleanFlagOptions, 'kind'> & { readonly kind: 'flag' };

export type GlobalCountFlagOptions = CountFlagOptions;

export interface StreamOptions {
  readonly doc?: DocInput;
  readonly mime?: readonly string[];
  readonly required?: boolean;
}

export type FormatterInput =
  | string
  | ({ readonly name: string } & Partial<Omit<Formatter, 'name'>>);

export interface ReturnsOptions {
  readonly doc?: DocInput;
  readonly formatters?: readonly FormatterInput[];
  readonly defaultFormatter?: string;
}

interface ErrorOptionsBase {
  readonly kind: 'usage' | 'runtime';
  readonly exitCode: number;
  readonly doc?: DocInput;
}

export type ErrorOptions<Payload extends StandardSchemaV1 | undefined = undefined> =
  ErrorOptionsBase & { readonly payload?: Payload };

export type ConstraintRef = ExtendedRef;
export type ToolConstraint = ExtendedConstraint;

type ConstraintSide = ConstraintRef | readonly ConstraintRef[];

function refs(side: ConstraintSide): readonly ConstraintRef[] {
  return Array.isArray(side) ? [...side] : [side as ConstraintRef];
}

export const c = {
  present(name: string): ConstraintRef {
    return { tag: 'present', name };
  },
  valueIs(name: string, value: unknown): ConstraintRef {
    return { tag: 'value-is', name, value: { tag: 'deferred', value } };
  },
  requiresAll(values: readonly ConstraintRef[]): ToolConstraint {
    return { tag: 'requires-all', refs: [...values] };
  },
  allOrNone(values: readonly ConstraintRef[]): ToolConstraint {
    return { tag: 'all-or-none', refs: [...values] };
  },
  requiresAny(values: readonly ConstraintRef[]): ToolConstraint {
    return { tag: 'requires-any', refs: [...values] };
  },
  mutexGroups(groups: readonly (readonly ConstraintRef[])[]): ToolConstraint {
    return { tag: 'mutex-groups', groups: groups.map((group) => [...group]) };
  },
  implies(options: {
    readonly lhs: ConstraintSide;
    readonly rhs: ConstraintSide;
    readonly lhsQuant?: Quantifier;
    readonly rhsQuant?: Quantifier;
  }): ToolConstraint {
    return {
      tag: 'implies',
      lhsQuant: options.lhsQuant ?? 'all',
      lhs: refs(options.lhs),
      rhsQuant: options.rhsQuant ?? 'all',
      rhs: refs(options.rhs),
    };
  },
  forbids(options: {
    readonly lhs: ConstraintSide;
    readonly rhs: ConstraintSide;
    readonly lhsQuant?: Quantifier;
  }): ToolConstraint {
    return {
      tag: 'forbids',
      lhsQuant: options.lhsQuant ?? 'all',
      lhs: refs(options.lhs),
      rhs: refs(options.rhs),
    };
  },
} as const;

export class BodyBuilder<
  Args = {},
  Success = undefined,
  Errors = never,
  Stdin extends StreamPresence = 'none',
  Stdout extends StreamPresence = 'none',
> {
  private constructor(private readonly value: ExtendedCommandBody) {}

  static start(): BodyBuilder {
    return new BodyBuilder({
      positionals: emptyPositionals(),
      options: [],
      flags: [],
      constraints: [],
      errors: [],
    });
  }

  positional<
    const Name extends string,
    Schema extends StandardSchemaV1,
    const Options extends PositionalOptions<SchemaOutput<Schema>> = {},
  >(
    name: Name,
    schema: Schema,
    options?: Options,
  ): BodyBuilder<
    AddBodyArgument<Args, Name, SchemaOutput<Schema>, PositionalIsRequired<Options>>,
    Success,
    Errors,
    Stdin,
    Stdout
  > {
    const codec = compileSchema(schema);
    const defaultValue = hasOwn(options, 'default')
      ? codecValue(codec, options?.default)
      : undefined;
    return this.next({
      ...this.value,
      positionals: {
        ...this.value.positionals,
        fixed: [
          ...this.value.positionals.fixed,
          {
            name,
            doc: normalizeDoc(options?.doc),
            valueName: options?.valueName,
            codec,
            default: defaultValue,
            required: options?.required ?? true,
            acceptsStdio: options?.acceptsStdio ?? false,
          },
        ],
      },
    });
  }

  tail<const Name extends string, Schema extends StandardSchemaV1>(
    name: Name,
    schema: Schema,
    options: TailOptions = {},
  ): BodyBuilder<
    ReplaceTailArgument<Args, Name, SchemaOutput<Schema>[]>,
    Success,
    Errors,
    Stdin,
    Stdout
  > {
    return this.next({
      ...this.value,
      positionals: {
        ...this.value.positionals,
        tail: {
          name,
          doc: normalizeDoc(options.doc),
          valueName: options.valueName,
          itemCodec: compileSchema(schema),
          min: options.min ?? 0,
          max: options.max,
          separator: options.separator,
          verbatim: options.verbatim ?? false,
          acceptsStdio: options.acceptsStdio ?? false,
        },
      },
    });
  }

  option<
    const Name extends string,
    Schema extends StandardSchemaV1,
    const Options extends OptionOptions<SchemaOutput<Schema>, SchemaProducesMap<Schema>> = {},
  >(
    name: Name,
    schema: Schema,
    options?: Options,
  ): BodyBuilder<
    AddBodyArgument<
      Args,
      Name,
      OptionValue<Schema, Options>,
      OptionIsRequired<Options>,
      OptionAliases<Options>
    >,
    Success,
    Errors,
    Stdin,
    Stdout
  > {
    return this.next({
      ...this.value,
      options: [...this.value.options, buildOption(name, schema, options)],
    });
  }

  flag<const Name extends string, const Options extends FlagOptions = {}>(
    name: Name,
    options?: Options,
  ): BodyBuilder<
    AddBodyArgument<Args, Name, FlagValue<Options>, true, OptionAliases<Options>>,
    Success,
    Errors,
    Stdin,
    Stdout
  > {
    return this.next({
      ...this.value,
      flags: [...this.value.flags, buildFlag(name, options)],
    });
  }

  constraint(constraint: ToolConstraint): BodyBuilder<Args, Success, Errors, Stdin, Stdout> {
    return this.next({
      ...this.value,
      constraints: [...this.value.constraints, constraint],
    });
  }

  stdin<const Options extends StreamOptions>(
    options: Options,
  ): BodyBuilder<Args, Success, Errors, StreamState<Options>, Stdout> {
    return this.next({ ...this.value, stdin: buildStream(options) });
  }

  stdout<const Options extends StreamOptions>(
    options: Options,
  ): BodyBuilder<Args, Success, Errors, Stdin, StreamState<Options>> {
    return this.next({ ...this.value, stdout: buildStream(options) });
  }

  returns<Schema extends StandardSchemaV1>(
    schema: Schema,
    options: ReturnsOptions = {},
  ): BodyBuilder<Args, SuccessOutput<Schema>, Errors, Stdin, Stdout> {
    const codec = compileSchema(schema);
    let result: ExtendedResultSpec | undefined;
    if (codec.isUnit) {
      if (options.formatters !== undefined || options.defaultFormatter !== undefined) {
        toolBuildError('invalid-metadata-value', 'unit tool results cannot declare formatters');
      }
    } else {
      const formatters = (options.formatters ?? ['default']).map(normalizeFormatter);
      result = {
        codec,
        doc: normalizeDoc(options.doc),
        formatters,
        defaultFormatter: options.defaultFormatter ?? formatters[0]?.name ?? 'default',
      };
    }
    return this.next({ ...this.value, result });
  }

  error<const Name extends string>(
    name: Name,
    options: ErrorOptions,
  ): BodyBuilder<Args, Success, Errors | ToolErr<Name>, Stdin, Stdout>;
  error<const Name extends string, Payload extends StandardSchemaV1>(
    name: Name,
    options: ErrorOptions<Payload> & { readonly payload: Payload },
  ): BodyBuilder<Args, Success, Errors | ToolErr<Name, SchemaOutput<Payload>>, Stdin, Stdout>;
  error(
    name: string,
    options: ErrorOptions<StandardSchemaV1 | undefined>,
  ): BodyBuilder<Args, Success, Errors | ToolErr<string, unknown>, Stdin, Stdout> {
    const errorCase: ExtendedErrorCase = {
      name,
      doc: normalizeDoc(options.doc),
      kind: options.kind === 'usage' ? 'usage-error' : 'runtime-error',
      exitCode: options.exitCode,
      payloadCodec: options.payload ? compileSchema(options.payload) : undefined,
    };
    return this.next({ ...this.value, errors: [...this.value.errors, errorCase] });
  }

  private next<
    NextArgs,
    NextSuccess,
    NextErrors,
    NextStdin extends StreamPresence,
    NextStdout extends StreamPresence,
  >(
    value: ExtendedCommandBody,
  ): BodyBuilder<NextArgs, NextSuccess, NextErrors, NextStdin, NextStdout> {
    return new BodyBuilder(value);
  }

  build(): ExtendedCommandBody {
    return this.value;
  }
}

const BUILD_TOOL = Symbol('golem.tool.build');
type AnyToolDefinition = CommandBuilder<any, any, any, any, true>;

export class CommandBuilder<
  Name extends string,
  Globals = {},
  Body extends AnyToolBodyModel | undefined = undefined,
  Children = {},
  Root extends boolean = false,
> {
  private implemented = false;
  private compiled?: ExtendedToolType;

  private constructor(
    readonly name: Name,
    private readonly node: ExtendedCommandNode,
    private readonly toolVersion: string,
    private readonly commandAnnotations?: CommandAnnotations,
    private readonly subtreeForwards: readonly SubtreeForward[] = [],
  ) {}

  static root<const Name extends string>(
    name: Name,
  ): CommandBuilder<Name, {}, undefined, {}, true> {
    return new CommandBuilder(name, emptyCommand(name), '0.0.0');
  }

  private static child<const Name extends string>(name: Name): CommandBuilder<Name> {
    return new CommandBuilder(name, emptyCommand(name), '0.0.0');
  }

  version(
    this: CommandBuilder<Name, Globals, Body, Children, true>,
    version: string,
  ): CommandBuilder<Name, Globals, Body, Children, true> {
    return new CommandBuilder(
      this.name,
      this.node,
      version,
      this.commandAnnotations,
      this.subtreeForwards,
    );
  }

  doc(doc: DocInput): CommandBuilder<Name, Globals, Body, Children, Root> {
    return this.next({ ...this.node, doc: normalizeDoc(doc) });
  }

  aliases(...aliases: readonly string[]): CommandBuilder<Name, Globals, Body, Children, Root> {
    return this.next({ ...this.node, aliases: [...this.node.aliases, ...aliases] });
  }

  annotations(
    annotations: Partial<CommandAnnotations>,
  ): CommandBuilder<Name, Globals, Body, Children, Root> {
    return new CommandBuilder(
      this.name,
      this.node,
      this.toolVersion,
      normalizeAnnotations(annotations),
      this.subtreeForwards,
    );
  }

  global<
    const ArgumentName extends string,
    Schema extends StandardSchemaV1,
    const Options extends GlobalValueOptions<SchemaOutput<Schema>, SchemaProducesMap<Schema>> = {},
  >(
    name: ArgumentName,
    schema: Schema,
    options?: Options,
  ): CommandBuilder<
    Name,
    AddGlobalArgument<
      Globals,
      ArgumentName,
      OptionValue<Schema, Options>,
      OptionIsRequired<Options>,
      Options
    >,
    Body,
    Children,
    Root
  >;
  global<
    const ArgumentName extends string,
    Schema extends StandardSchemaV1<unknown, boolean>,
    const Options extends GlobalFlagOptions,
  >(
    name: ArgumentName,
    schema: Schema,
    options: Options,
  ): CommandBuilder<
    Name,
    AddGlobalArgument<Globals, ArgumentName, boolean, true, Options>,
    Body,
    Children,
    Root
  >;
  global<const ArgumentName extends string, const Options extends GlobalCountFlagOptions>(
    name: ArgumentName,
    options: Options,
  ): CommandBuilder<
    Name,
    AddGlobalArgument<Globals, ArgumentName, number, true, Options>,
    Body,
    Children,
    Root
  >;
  global(
    name: string,
    schemaOrOptions: StandardSchemaV1 | GlobalCountFlagOptions,
    options?: GlobalValueOptions<unknown> | GlobalFlagOptions,
  ): CommandBuilder<Name, object, Body, Children, Root> {
    let globals: ExtendedGlobals;
    if (isStandardSchema(schemaOrOptions)) {
      if (options?.kind === 'flag') {
        globals = {
          ...this.node.globals,
          flags: [...this.node.globals.flags, buildFlag(name, options)],
        };
      } else {
        globals = {
          ...this.node.globals,
          options: [
            ...this.node.globals.options,
            buildOption(name, schemaOrOptions, options as GlobalValueOptions<unknown>),
          ],
        };
      }
    } else {
      globals = {
        ...this.node.globals,
        flags: [...this.node.globals.flags, buildFlag(name, schemaOrOptions)],
      };
    }
    return this.next({ ...this.node, globals });
  }

  body<Built extends BodyBuilder<any, any, any, any, any>>(
    build: (body: BodyBuilder) => Built,
  ): CommandBuilder<Name, Globals, BodyModelOf<Built>, Children, Root> {
    const body = build(BodyBuilder.start());
    if (!(body instanceof BodyBuilder)) {
      toolBuildError('invalid-metadata-value', 'tool body callback must return its body builder');
    }
    return this.next({ ...this.node, body: body.build() });
  }

  command<
    const ChildName extends string,
    ChildGlobals,
    ChildBody extends AnyToolBodyModel | undefined,
    ChildChildren,
  >(
    name: ChildName,
    subtree: ToolDefinition<ChildName, ChildGlobals, ChildBody, ChildChildren>,
  ): CommandBuilder<
    Name,
    Globals,
    Body,
    Children & {
      [Key in ChildName]: ToolSubtreeModel<
        ToolCommandModel<ChildName, ChildGlobals, ChildBody, ChildChildren>
      >;
    },
    Root
  >;
  command<const ChildName extends string, Built extends CommandBuilder<any, any, any, any, false>>(
    name: ChildName,
    build: (command: CommandBuilder<ChildName>) => Built,
  ): CommandBuilder<
    Name,
    Globals,
    Body,
    Children & { [Key in ChildName]: ToolCommandModelOf<Built> },
    Root
  >;
  command(
    name: string,
    buildOrSubtree:
      | AnyToolDefinition
      | ((command: CommandBuilder<string>) => CommandBuilder<any, any, any, any, false>),
  ): CommandBuilder<Name, Globals, Body, Children & Record<string, unknown>, Root> {
    if (buildOrSubtree instanceof CommandBuilder) {
      const childTool = new ExtendedToolType(
        buildOrSubtree.toolVersion,
        buildOrSubtree.finalizeNode(),
      );
      const graft = graftSubtree(childTool, { expectedName: name });
      return this.next(
        {
          ...this.node,
          subcommands: [...this.node.subcommands, graft],
        },
        [...this.subtreeForwards, { pathPrefix: [graft.name], childToolName: childTool.toolName }],
      );
    }

    const child = buildOrSubtree(CommandBuilder.child(name));
    if (!(child instanceof CommandBuilder) || child.name !== name) {
      toolBuildError(
        'invalid-metadata-value',
        `tool command callback for "${name}" must return that command's builder`,
      );
    }
    return this.next(
      {
        ...this.node,
        subcommands: [...this.node.subcommands, child.finalizeNode()],
      },
      [
        ...this.subtreeForwards,
        ...child.subtreeForwards.map((forward) => ({
          ...forward,
          pathPrefix: [child.name, ...forward.pathPrefix],
        })),
      ],
    );
  }

  implement(
    this: CommandBuilder<Name, Globals, Body, Children, true>,
    implementation: ToolImplementation<CommandBuilder<Name, Globals, Body, Children, true>>,
  ): ImplementedTool<Name> {
    if (this.implemented) {
      ToolRegistry.recordRegistrationError(
        this.name,
        'Implementation failed: implement() was called more than once for this definition',
      );
      return { name: this.name };
    }
    this.implemented = true;
    try {
      ToolRegistry.registerImplementation(this.name, () => {
        const tool = this[BUILD_TOOL]();
        const runtime = bindToolImplementation(
          tool,
          implementation as object,
          this.subtreeForwards,
        );
        return { tool, runtime };
      });
    } catch (error) {
      ToolRegistry.recordRegistrationError(
        this.name,
        `Implementation failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    return { name: this.name };
  }

  private next<NextGlobals, NextBody extends AnyToolBodyModel | undefined, NextChildren>(
    node: ExtendedCommandNode,
    subtreeForwards: readonly SubtreeForward[] = this.subtreeForwards,
  ): CommandBuilder<Name, NextGlobals, NextBody, NextChildren, Root> {
    return new CommandBuilder(
      this.name,
      node,
      this.toolVersion,
      this.commandAnnotations,
      subtreeForwards,
    );
  }

  private finalizeNode(): ExtendedCommandNode {
    if (this.commandAnnotations && !this.node.body) {
      toolBuildError(
        'invalid-metadata-value',
        `command "${this.name}" declares annotations but has no body`,
      );
    }
    return this.node.body
      ? { ...this.node, body: { ...this.node.body, annotations: this.commandAnnotations } }
      : this.node;
  }

  [BUILD_TOOL](this: CommandBuilder<Name, Globals, Body, Children, true>): ExtendedToolType {
    if (this.compiled) return this.compiled;
    const tool = normalizeExtendedTool(new ExtendedToolType(this.toolVersion, this.finalizeNode()));
    validateTypeScriptProjection(tool);
    this.compiled = tool;
    return this.compiled;
  }
}

export type ToolDefinition<
  Name extends string = string,
  Globals = {},
  Body extends AnyToolBodyModel | undefined = undefined,
  Children = {},
> = CommandBuilder<Name, Globals, Body, Children, true>;

export function toolDefinition<const Name extends string>(name: Name): ToolDefinition<Name> {
  return CommandBuilder.root(name);
}

/** Finalize a fluent definition into the shared extended model used by tool runtime layers. */
export function getExtendedToolDefinition<Definition extends AnyToolDefinition>(
  definition: Definition,
): ExtendedToolType {
  return definition[BUILD_TOOL]();
}

/** Assemble a typed runtime client directly from a local tool definition. */
export function client<Definition extends AnyToolDefinition>(
  definition: Definition,
  options: ToolClientOptions = {},
): ToolClient<Definition> {
  const tool = getExtendedToolDefinition(definition);
  const transport = options.transport ?? new HostToolClientTransport(tool.toolName);
  const result: Record<string, unknown> = {};

  if (tool.root.body) {
    defineClientMember(
      result,
      tool.root.name,
      createToolClientMethod(tool, tool.root, [], transport),
    );
  }
  tool.root.subcommands.forEach((child) => {
    defineClientMember(
      result,
      child.name,
      assembleToolClientNode(tool, child, [child.name], transport),
    );
  });

  return result as ToolClient<Definition>;
}

class HostToolClientTransport implements ToolClientTransport {
  private rpc?: ToolRpc;

  constructor(private readonly toolName: string) {}

  invokeAndAwait(
    commandPath: readonly string[],
    input: WireTypedSchemaValue,
    stdin: InputStream | undefined,
  ): ToolClientInvocationResult {
    this.rpc ??= new ToolRpc(this.toolName);
    return this.rpc.invokeAndAwait([...commandPath], input, stdin);
  }
}

function assembleToolClientNode(
  tool: ExtendedToolType,
  node: ExtendedCommandNode,
  commandPath: readonly string[],
  transport: ToolClientTransport,
): unknown {
  const result: Record<string, unknown> | ((args: Record<string, unknown>) => Promise<unknown>) =
    node.body ? createToolClientMethod(tool, node, commandPath, transport) : {};

  node.subcommands.forEach((child) => {
    defineClientMember(
      result,
      child.name,
      assembleToolClientNode(tool, child, [...commandPath, child.name], transport),
    );
  });
  return result;
}

function defineClientMember(target: object, name: string, value: unknown): void {
  Object.defineProperty(target, name, {
    value,
    enumerable: true,
    configurable: true,
    writable: false,
  });
}

function createToolClientMethod(
  tool: ExtendedToolType,
  node: ExtendedCommandNode,
  commandPath: readonly string[],
  transport: ToolClientTransport,
): (args: Record<string, unknown>) => Promise<unknown> {
  const body = node.body;
  if (!body) throw new Error(`tool command "${node.name}" has no callable body`);
  const inputModel = tool.canonicalInputModel(node);
  const callName = [tool.toolName, ...commandPath].join(' ');

  return async (args: Record<string, unknown>): Promise<unknown> => {
    try {
      if (!isImplementationObject(args)) {
        throw new Error('tool client arguments must be an object');
      }
      const canonicalInput = Object.fromEntries(
        inputModel.fields.map((field) => {
          const projectedName = camelCase(field.name);
          return [field.name, hasOwn(args, projectedName) ? args[projectedName] : undefined];
        }),
      );
      const input = typedSchemaValueToWit(inputModel.encodeTyped(canonicalInput));
      const stdin = body.stdin ? (args.stdin as InputStream | undefined) : undefined;
      if (stdin !== undefined && !isWitResource(stdin)) {
        throw new Error('stdin must be a WIT resource');
      }
      if (body.stdin?.required && stdin === undefined) {
        throw new Error('required stdin stream is missing');
      }
      const invocation = await transport.invokeAndAwait(commandPath, input, stdin);
      return decodeToolClientResult(body, invocation, callName);
    } catch (error) {
      if (error instanceof ToolCallError) throw error;
      if (isRpcError(error)) throw mapToolRpcError(body, error, callName);
      throw protocolToolCallError(`${callName}: ${errorMessage(error)}`);
    }
  };
}

function decodeToolClientResult(
  body: ExtendedCommandBody,
  invocation: ToolClientInvocationResult,
  callName: string,
): unknown {
  const hasResult = invocation.result !== undefined;
  const hasStdout = invocation.stdout !== undefined;

  try {
    if (hasStdout && !isWitResource(invocation.stdout)) {
      throw protocolToolCallError(`${callName}: stdout must be a WIT resource`);
    }
    if (!body.result && hasResult) {
      throw protocolToolCallError(`${callName}: unit command returned an unexpected result`);
    }
    if (body.result && !hasResult) {
      throw protocolToolCallError(`${callName}: structured command result is missing`);
    }
    if (!body.stdout && hasStdout) {
      throw protocolToolCallError(`${callName}: command returned undeclared stdout`);
    }
    if (body.stdout?.required && !hasStdout) {
      throw protocolToolCallError(`${callName}: required stdout stream is missing`);
    }

    const decodedResult = body.result
      ? decodeWireValue(body.result.codec, invocation.result!, `${callName} result`)
      : undefined;
    if (!body.stdout) return decodedResult;
    if (!body.result) return invocation.stdout;
    return hasStdout
      ? { result: decodedResult, stdout: invocation.stdout }
      : { result: decodedResult };
  } catch (error) {
    disposeWitResource(invocation.stdout);
    throw error;
  }
}

function isWitResource(value: unknown): boolean {
  return value !== null && (typeof value === 'object' || typeof value === 'function');
}

function mapToolRpcError(
  body: ExtendedCommandBody,
  error: RpcError,
  callName: string,
): ToolCallError<unknown> {
  if (error.tag !== 'remote-tool-error' || error.val.tag !== 'custom-error') {
    return new ToolCallError({ tag: 'rpc', error });
  }

  try {
    const declaredError = decodeDeclaredToolError(body, error.val.val, callName);
    return new ToolCallError({ tag: 'tool', error: declaredError });
  } catch (decodeError) {
    if (decodeError instanceof ToolCallError) return decodeError;
    return protocolToolCallError(`${callName}: ${errorMessage(decodeError)}`);
  }
}

function decodeDeclaredToolError(
  body: ExtendedCommandBody,
  wirePayload: WireTypedSchemaValue,
  callName: string,
): ToolErr<string, unknown> | ToolErr<string> {
  const payload = typedSchemaValueFromWit(wirePayload);
  const unitGraph = { defs: new Map(), root: t.tuple([]) };
  const errorCase = body.errors.find((errorCase) =>
    deepEqual(payload.graph, errorCase.payloadCodec?.graph ?? unitGraph),
  );
  if (!errorCase) {
    throw new Error('remote custom error does not match any declared error schema');
  }

  if (!errorCase.payloadCodec) {
    if (payload.value.tag !== 'tuple' || payload.value.elements.length !== 0) {
      throw new Error(`remote custom error "${errorCase.name}" has a non-unit payload`);
    }
    return err(errorCase.name);
  }
  const decoded = decodeTypedValue(
    errorCase.payloadCodec,
    payload,
    `${callName} custom error "${errorCase.name}"`,
  );
  return err(errorCase.name, decoded);
}

function decodeWireValue(
  codec: FluentCodec,
  wire: WireTypedSchemaValue,
  position: string,
): unknown {
  return decodeTypedValue(codec, typedSchemaValueFromWit(wire), position);
}

function decodeTypedValue(codec: FluentCodec, typed: TypedSchemaValue, position: string): unknown {
  if (!deepEqual(typed.graph, codec.graph)) {
    throw new Error(`${position} schema does not match the local definition`);
  }
  if (!schemaValueConforms(codec.graph, codec.graph.root, typed.value)) {
    throw new Error(`${position} does not conform to the local definition`);
  }
  return codec.fromValue(typed.value);
}

function protocolToolCallError(message: string): ToolCallError<never> {
  return new ToolCallError({ tag: 'rpc', error: protocolRpcError(message) });
}

function protocolRpcError(message: string): RpcError {
  return { tag: 'protocol-error', val: message };
}

function isRpcError(value: unknown): value is RpcError {
  if (!isImplementationObject(value) || typeof value.tag !== 'string' || !hasOwn(value, 'val')) {
    return false;
  }
  switch (value.tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
      return typeof value.val === 'string';
    case 'remote-tool-error':
      return isToolError(value.val);
    default:
      return false;
  }
}

function isToolError(value: unknown): value is ToolError {
  if (!isImplementationObject(value) || typeof value.tag !== 'string' || !hasOwn(value, 'val')) {
    return false;
  }
  switch (value.tag) {
    case 'invalid-tool-name':
    case 'invalid-input':
    case 'constraint-violation':
    case 'invalid-result':
      return typeof value.val === 'string';
    case 'invalid-command-path':
      return Array.isArray(value.val) && value.val.every((segment) => typeof segment === 'string');
    case 'custom-error':
      return (
        isImplementationObject(value.val) &&
        hasOwn(value.val, 'graph') &&
        hasOwn(value.val, 'value')
      );
    default:
      return false;
  }
}

function formatToolCallError(cause: ToolCallErrorCause<unknown>): string {
  if (cause.tag === 'tool') {
    const name = isImplementationObject(cause.error) ? cause.error.name : undefined;
    return typeof name === 'string'
      ? `Remote tool returned declared error "${name}"`
      : 'Remote tool returned a declared error';
  }
  return cause.error.tag === 'remote-tool-error'
    ? `Remote tool call failed: ${cause.error.val.tag}`
    : `Remote tool call failed: ${cause.error.tag}: ${cause.error.val}`;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** Render descriptor-based help for the root or a nested command. */
export function renderHelp(
  definition: AnyToolDefinition,
  commandPath: readonly string[] = [],
): ToolHelpResult {
  const tool = getExtendedToolDefinition(definition);
  const projection = tool.projectHelp(commandPath);
  if (!projection) {
    return {
      tag: 'err',
      error: { tag: 'invalid-command-path', commandPath: [...commandPath] },
    };
  }
  return { tag: 'ok', value: renderToolHelp(tool, projection) };
}

/** Render descriptor-based help for one argument at the selected command. */
export function renderArgumentHelp(
  definition: AnyToolDefinition,
  commandPath: readonly string[],
  argumentName: string,
): ToolHelpResult {
  const tool = getExtendedToolDefinition(definition);
  const projection = tool.projectHelp(commandPath);
  if (!projection) {
    return {
      tag: 'err',
      error: { tag: 'invalid-command-path', commandPath: [...commandPath] },
    };
  }
  const argument = projection.arguments.find(
    (entry) => entry.name === argumentName || entry.aliases.includes(argumentName),
  );
  if (!argument) {
    return {
      tag: 'err',
      error: {
        tag: 'invalid-argument-name',
        commandPath: [...projection.commandPath],
        argumentName,
      },
    };
  }
  return { tag: 'ok', value: renderToolArgumentHelp(argument) };
}

function renderToolHelp(tool: ExtendedToolType, projection: ToolHelpProjection): string {
  const { command, arguments: arguments_, subcommands } = projection;
  const lines = [
    `Usage: ${[tool.toolName, ...projection.commandPath].join(' ')}`,
    '',
    command.doc.summary,
    command.doc.description,
  ];
  appendAliases(lines, command.aliases);
  appendExamples(lines, command.doc, '');
  appendArgumentSection(
    lines,
    'Globals',
    arguments_.filter((entry) => entry.kind === 'global-option' || entry.kind === 'global-flag'),
  );
  appendArgumentSection(
    lines,
    'Positionals',
    arguments_.filter((entry) => entry.kind === 'positional'),
  );
  appendArgumentSection(
    lines,
    'Tail',
    arguments_.filter((entry) => entry.kind === 'tail'),
  );
  appendArgumentSection(
    lines,
    'Options',
    arguments_.filter((entry) => entry.kind === 'option'),
  );
  appendArgumentSection(
    lines,
    'Flags',
    arguments_.filter((entry) => entry.kind === 'flag'),
  );
  if (subcommands.length > 0) {
    lines.push('', 'Subcommands:');
    subcommands.forEach((entry) => {
      const aliases = entry.aliases.length > 0 ? ` (aliases: ${entry.aliases.join(', ')})` : '';
      lines.push(`  ${entry.name}${aliases}\t${entry.doc.summary}`);
    });
  }
  const body = command.body;
  if (body) {
    if (body.stdin) appendStreamHelp(lines, 'Stdin', body.stdin);
    if (body.stdout) appendStreamHelp(lines, 'Stdout', body.stdout);
    if (body.result) appendResultHelp(lines, body.result);
    if (body.errors.length > 0) appendErrorHelp(lines, body.errors);
    if (body.annotations) appendAnnotationHelp(lines, body.annotations);
  }
  return `${lines.join('\n')}\n`;
}

function renderToolArgumentHelp(argument: ToolHelpArgument): string {
  const global = argument.kind === 'global-option' || argument.kind === 'global-flag';
  const type =
    argument.kind === 'global-option' || argument.kind === 'option'
      ? 'option'
      : argument.kind === 'global-flag' || argument.kind === 'flag'
        ? 'flag'
        : argument.kind === 'tail'
          ? 'tail positional'
          : 'positional';
  const required = type === 'positional' && argument.required ? ', required' : '';
  const globalSuffix = global ? ', global' : '';
  const lines = [
    `${argumentDisplayName(argument)} (${type}${required}${globalSuffix})`,
    argument.doc.summary,
    argument.doc.description,
  ];
  const aliases = [
    ...argument.aliases.map((alias) => `--${alias}`),
    ...(argument.short ? [`-${argument.short}`] : []),
  ];
  appendAliases(lines, aliases);
  if (argument.valueName) lines.push(`Value: ${argument.valueName}`);
  if (argument.required !== undefined) lines.push(`Required: ${String(argument.required)}`);
  if (argument.default) lines.push(`Default: ${formatDefaultValue(argument.default)}`);
  if (argument.flagDefault !== undefined) {
    lines.push(`Default: ${String(argument.flagDefault)}`);
  }
  if (argument.envVar) lines.push(`Environment: ${argument.envVar}`);
  if (argument.min !== undefined) lines.push(`Minimum occurrences: ${argument.min}`);
  if (argument.max !== undefined) lines.push(`Maximum occurrences: ${argument.max}`);
  if (argument.separator !== undefined) lines.push(`Separator: ${argument.separator}`);
  if (argument.verbatim) lines.push('Verbatim: true');
  if (argument.negatable) lines.push('Negatable: true');
  if (argument.countMax !== undefined) lines.push(`Maximum count: ${argument.countMax}`);
  if (argument.acceptsStdio) lines.push('Accepts standard input: true');
  appendExamples(lines, argument.doc, '');
  return `${lines.join('\n')}\n`;
}

function appendArgumentSection(
  lines: string[],
  title: string,
  arguments_: readonly ToolHelpArgument[],
): void {
  if (arguments_.length === 0) return;
  lines.push('', `${title}:`);
  arguments_.forEach((argument) => {
    const details = argumentSummaryDetails(argument);
    lines.push(
      `  ${argumentDisplayName(argument)}${details.length > 0 ? ` [${details.join('; ')}]` : ''}\t${argument.doc.summary}`,
    );
  });
}

function argumentDisplayName(argument: ToolHelpArgument): string {
  if (argument.kind === 'positional') {
    return `${argument.name}${argument.valueName ? ` <${argument.valueName}>` : ''}`;
  }
  if (argument.kind === 'tail') {
    return `${argument.name}...${argument.valueName ? ` <${argument.valueName}>...` : ''}`;
  }
  const surfaces = [
    `--${argument.name}`,
    ...argument.aliases.map((alias) => `--${alias}`),
    ...(argument.short ? [`-${argument.short}`] : []),
  ];
  return `${surfaces.join(', ')}${argument.valueName ? ` <${argument.valueName}>` : ''}`;
}

function argumentSummaryDetails(argument: ToolHelpArgument): string[] {
  const details: string[] = [];
  if (argument.required !== undefined) details.push(argument.required ? 'required' : 'optional');
  if (argument.default) details.push(`default: ${formatDefaultValue(argument.default)}`);
  if (argument.flagDefault !== undefined) details.push(`default: ${String(argument.flagDefault)}`);
  if (argument.envVar) details.push(`env: ${argument.envVar}`);
  if (argument.acceptsStdio) details.push('accepts stdio');
  if (argument.min !== undefined) details.push(`min: ${argument.min}`);
  if (argument.max !== undefined) details.push(`max: ${argument.max}`);
  if (argument.separator !== undefined) details.push(`separator: ${argument.separator}`);
  if (argument.verbatim) details.push('verbatim');
  if (argument.negatable) details.push('negatable');
  if (argument.countMax !== undefined) details.push(`max count: ${argument.countMax}`);
  return details;
}

function appendAliases(lines: string[], aliases: readonly string[]): void {
  if (aliases.length > 0) lines.push(`Aliases: ${aliases.join(', ')}`);
}

function appendExamples(lines: string[], doc: Doc, indent: string): void {
  if (doc.examples.length === 0) return;
  lines.push(`${indent}Examples:`);
  doc.examples.forEach((example) => {
    lines.push(`${indent}  ${example.title}:`);
    example.body.split('\n').forEach((line) => lines.push(`${indent}    ${line}`));
  });
}

function appendStreamHelp(lines: string[], title: string, stream: StreamSpec): void {
  const details = [stream.required ? 'required' : 'optional'];
  if (stream.mime.length > 0) details.push(`MIME: ${stream.mime.join(', ')}`);
  lines.push('', `${title}:`, `  ${details.join('; ')}\t${stream.doc.summary}`);
  appendIndentedDoc(lines, stream.doc);
}

function appendResultHelp(lines: string[], result: ExtendedResultSpec): void {
  lines.push('', 'Result:', `  ${result.doc.summary}`);
  appendIndentedDoc(lines, result.doc);
  lines.push(
    `  Formatters: ${result.formatters
      .map((formatter) =>
        formatter.name === result.defaultFormatter ? `${formatter.name} (default)` : formatter.name,
      )
      .join(', ')}`,
  );
  result.formatters.forEach((formatter) => {
    if (formatter.doc.summary || formatter.doc.description || formatter.doc.examples.length > 0) {
      lines.push(`    ${formatter.name}\t${formatter.doc.summary}`);
      if (formatter.doc.description) lines.push(`      ${formatter.doc.description}`);
      appendExamples(lines, formatter.doc, '      ');
    }
  });
}

function appendErrorHelp(lines: string[], errors: readonly ExtendedErrorCase[]): void {
  lines.push('', 'Errors:');
  errors.forEach((error) => {
    const details = [error.kind, `exit: ${error.exitCode}`];
    if (error.payloadCodec) details.push('payload');
    lines.push(`  ${error.name} [${details.join('; ')}]\t${error.doc.summary}`);
    appendIndentedDoc(lines, error.doc);
  });
}

function appendAnnotationHelp(lines: string[], annotations: CommandAnnotations): void {
  lines.push(
    '',
    'Annotations:',
    `  read-only: ${String(annotations.readOnly)}`,
    `  destructive: ${String(annotations.destructive)}`,
    `  idempotent: ${String(annotations.idempotent)}`,
    `  open-world: ${String(annotations.openWorld)}`,
  );
}

function appendIndentedDoc(lines: string[], doc: Doc): void {
  if (doc.description) lines.push(`    ${doc.description}`);
  appendExamples(lines, doc, '    ');
}

function formatDefaultValue(value: NonNullable<ToolHelpArgument['default']>): string {
  const parsed = parseSourceValue(value.codec, value.value);
  return formatHelpValue(parsed.tag === 'valid' ? parsed.value : value.value);
}

function formatHelpValue(
  value: unknown,
  ancestors: Set<object> = new Set(),
  nested = false,
): string {
  if (typeof value === 'string') return nested ? JSON.stringify(value) : value;
  if (typeof value === 'number') return Object.is(value, -0) ? '-0' : String(value);
  if (typeof value === 'bigint') return value.toString();
  if (typeof value === 'boolean' || value === null || value === undefined) return String(value);
  if (typeof value !== 'object') return String(value);
  if (ancestors.has(value)) return '[Circular]';
  if (value instanceof Uint8Array) return `[${Array.from(value).join(', ')}]`;
  if (value instanceof Date) return value.toISOString();

  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      return `[${value.map((entry) => formatHelpValue(entry, ancestors, true)).join(', ')}]`;
    }
    if (value instanceof Map) {
      return `{${Array.from(
        value,
        ([key, entry]) =>
          `${formatHelpValue(key, ancestors, true)}: ${formatHelpValue(entry, ancestors, true)}`,
      ).join(', ')}}`;
    }
    return `{${Object.entries(value)
      .map(([key, entry]) => `${JSON.stringify(key)}: ${formatHelpValue(entry, ancestors, true)}`)
      .join(', ')}}`;
  } finally {
    ancestors.delete(value);
  }
}

function bindToolImplementation(
  tool: ExtendedToolType,
  implementation: object,
  subtreeForwards: readonly SubtreeForward[],
): ExtendedToolRuntime {
  if (!isImplementationObject(implementation)) {
    throw new Error(`tool "${tool.toolName}" implementation must be an object`);
  }

  const bindings: ExtendedToolRuntime['bindings'][number][] = [];

  const visit = (
    node: ExtendedCommandNode,
    commandPath: readonly string[],
    nodeImplementation: unknown,
    nodeReceiver: object | undefined,
    root: boolean,
  ): void => {
    const commandName = [tool.toolName, ...commandPath].join(' ');
    let childrenImplementation: Record<PropertyKey, unknown> | undefined;
    let handler: unknown;
    let receiver = nodeReceiver;

    if (root) {
      childrenImplementation = implementation;
      handler = node.body ? getImplementationProperty(implementation, node.name).value : undefined;
      receiver = implementation;
    } else if (node.body && node.subcommands.length === 0) {
      handler = nodeImplementation;
    } else {
      if (!isNestedCommandImplementation(nodeImplementation)) {
        throw new Error(
          `tool command "${commandName}" implementation must be created with command(...)`,
        );
      }
      childrenImplementation = nodeImplementation;
      handler = nodeImplementation[COMMAND_IMPLEMENTATION].body;
      receiver = nodeImplementation[COMMAND_IMPLEMENTATION].receiver;
    }

    if (node.body) {
      if (typeof handler !== 'function') {
        throw new Error(`tool command "${commandName}" implementation must be a function`);
      }
      bindings.push({
        commandPath: [...commandPath],
        handler: handler as ExtendedToolRuntime['bindings'][number]['handler'],
        receiver,
      });
    } else if (handler !== undefined) {
      throw new Error(`tool dispatcher "${commandName}" cannot have a body implementation`);
    }

    for (const child of node.subcommands) {
      const childPath = [...commandPath, child.name];
      if (subtreeForwards.some((forward) => pathsEqual(forward.pathPrefix, childPath))) {
        continue;
      }
      const childImplementation = childrenImplementation
        ? getImplementationProperty(childrenImplementation, child.name)
        : { found: false as const, value: undefined };
      if (!childImplementation.found) {
        throw new Error(`missing implementation for tool command "${commandName} ${child.name}"`);
      }
      visit(child, childPath, childImplementation.value, childImplementation.receiver, false);
    }
  };

  visit(tool.root, [], implementation, implementation, true);
  return {
    bindings,
    subtreeForwards: subtreeForwards.map((forward) => ({
      ...forward,
      pathPrefix: [...forward.pathPrefix],
    })),
  };
}

function pathsEqual(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((segment, index) => segment === right[index]);
}

function isImplementationObject(value: unknown): value is Record<PropertyKey, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function getImplementationProperty(
  implementation: Record<PropertyKey, unknown>,
  name: PropertyKey,
): { readonly found: boolean; readonly value: unknown; readonly receiver?: object } {
  const receiver = isNestedCommandImplementation(implementation)
    ? implementation[COMMAND_IMPLEMENTATION].receiver
    : implementation;
  let current: object | null = receiver;
  while (current !== null && current !== Object.prototype) {
    if (hasOwn(current, name)) {
      return {
        found: true,
        value: Reflect.get(receiver, name, receiver),
        receiver,
      };
    }
    current = Object.getPrototypeOf(current) as object | null;
  }
  return { found: false, value: undefined };
}

function isNestedCommandImplementation(value: unknown): value is Record<PropertyKey, unknown> & {
  readonly [COMMAND_IMPLEMENTATION]: { readonly body: unknown; readonly receiver: object };
} {
  if (!isImplementationObject(value) || !(COMMAND_IMPLEMENTATION in value)) return false;
  const marker = value[COMMAND_IMPLEMENTATION];
  return (
    isImplementationObject(marker) &&
    hasOwn(marker, 'body') &&
    hasOwn(marker, 'receiver') &&
    (typeof marker.receiver === 'object' || typeof marker.receiver === 'function') &&
    marker.receiver !== null
  );
}

function emptyCommand(name: string): ExtendedCommandNode {
  return {
    name,
    aliases: [],
    doc: emptyDoc(),
    globals: emptyGlobals(),
    subcommands: [],
  };
}

function normalizeDoc(input?: DocInput): Doc {
  if (typeof input === 'string') return { summary: input, description: '', examples: [] };
  return {
    summary: input?.summary ?? '',
    description: input?.description ?? '',
    examples: input?.examples ? input.examples.map((example) => ({ ...example })) : [],
  };
}

function normalizeAnnotations(input: Partial<CommandAnnotations>): CommandAnnotations {
  return {
    readOnly: input.readOnly ?? false,
    destructive: input.destructive ?? true,
    idempotent: input.idempotent ?? false,
    openWorld: input.openWorld ?? true,
  };
}

function normalizeFormatter(input: FormatterInput): Formatter {
  return typeof input === 'string'
    ? { name: input, doc: emptyDoc() }
    : { name: input.name, doc: normalizeDoc(input.doc) };
}

function buildStream(options: StreamOptions): StreamSpec {
  return {
    doc: normalizeDoc(options.doc),
    mime: options.mime ? [...options.mime] : [],
    required: options.required ?? false,
  };
}

function buildOption<Schema extends StandardSchemaV1>(
  name: string,
  schema: Schema,
  options: SurfaceOptions & {
    readonly valueName?: string;
    readonly required?: boolean;
    readonly default?: unknown;
    readonly repeatable?: RepeatableMode;
    readonly delim?: string;
    readonly optionalScalar?: boolean;
    readonly duplicateKeyPolicy?: DuplicateKeyPolicy;
  } = {},
): ExtendedOptionSpec {
  const codec = compileSchema(schema);
  const shape = buildOptionShape(codec, options);
  const collectedCodec = shape.tag === 'repeatable-list' ? listCodec(codec) : codec;
  return {
    long: name,
    short: options.short,
    aliases: options.aliases ? [...options.aliases] : [],
    doc: normalizeDoc(options.doc),
    valueName: options.valueName,
    shape,
    default: hasOwn(options, 'default') ? codecValue(collectedCodec, options.default) : undefined,
    required: options.required ?? false,
    envVar: options.env,
  };
}

function buildOptionShape(
  codec: FluentCodec,
  options: {
    readonly repeatable?: RepeatableMode;
    readonly delim?: string;
    readonly optionalScalar?: boolean;
    readonly duplicateKeyPolicy?: DuplicateKeyPolicy;
  },
): ExtendedOptionShape {
  if (options.repeatable) {
    const repetition = buildRepetition({
      repeatable: options.repeatable,
      delim: options.delim,
    });
    if (codec.mapValue) {
      return {
        tag: 'repeatable-map',
        repetition,
        mapCodec: codec,
        valueCodec: codec.mapValue,
        duplicateKeyPolicy: options.duplicateKeyPolicy ?? 'reject',
      };
    }
    return { tag: 'repeatable-list', repetition, itemCodec: codec };
  }
  return options.optionalScalar ? { tag: 'optional-scalar', codec } : { tag: 'scalar', codec };
}

function buildRepetition(options: {
  readonly repeatable: RepeatableMode;
  readonly delim?: string;
}): Repetition {
  if (options.repeatable === 'repeated') return { tag: 'repeated' };
  if (options.delim === undefined) {
    toolBuildError(
      'invalid-metadata-value',
      `${options.repeatable} repeatable options require a delimiter`,
    );
  }
  return { tag: options.repeatable, val: options.delim };
}

function buildFlag(name: string, options: FlagOptions | GlobalCountFlagOptions = {}) {
  return {
    long: name,
    short: options.short,
    aliases: options.aliases ? [...options.aliases] : [],
    doc: normalizeDoc(options.doc),
    shape:
      options.kind === 'count-flag'
        ? ({ tag: 'count-flag', val: options.max } as const)
        : ({
            tag: 'bool-flag',
            val: {
              default_: options.default ?? false,
              negatable: options.negatable ?? false,
            },
          } as const),
    envVar: options.env,
  };
}

function isStandardSchema(value: unknown): value is StandardSchemaV1 {
  return (
    (typeof value === 'object' || typeof value === 'function') &&
    value !== null &&
    '~standard' in value
  );
}

function hasOwn(value: object | undefined, key: PropertyKey): boolean {
  return value !== undefined && Object.prototype.hasOwnProperty.call(value, key);
}

function camelCase(name: string): string {
  return name.replace(/-([a-z0-9])/g, (_, char: string) => char.toUpperCase());
}

function validateTypeScriptProjection(tool: ExtendedToolType): void {
  const visit = (commandNode: ExtendedCommandNode): void => {
    if (commandNode.body) {
      const projected = new Map<string, string>();
      tool.canonicalInputFields(commandNode).forEach((field) => {
        const key = camelCase(field.name);
        if (commandNode.body?.stdin && key === 'stdin') {
          toolBuildError(
            'duplicate-name',
            `tool argument "${field.name}" conflicts with the TypeScript stdin stream field`,
          );
        }
        const previous = projected.get(key);
        if (previous !== undefined) {
          toolBuildError(
            'duplicate-name',
            `tool arguments "${previous}" and "${field.name}" both project to TypeScript key "${key}"`,
          );
        }
        projected.set(key, field.name);
      });
    }
    commandNode.subcommands.forEach(visit);
  };
  if (tool.root.body && tool.root.subcommands.some((child) => child.name === tool.root.name)) {
    toolBuildError(
      'duplicate-name',
      `root body and subcommand both use implementation key "${tool.root.name}"`,
    );
  }
  visit(tool.root);
}
