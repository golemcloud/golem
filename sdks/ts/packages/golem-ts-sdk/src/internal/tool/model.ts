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
  ErrorKind,
  FlagSpec,
  Formatter,
  Quantifier,
  Repetition,
  StreamSpec,
} from 'golem:tool/common@0.1.0';
import {
  deepEqual,
  field,
  mergeGraphDefs,
  type SchemaGraph,
  type SchemaValue,
  t,
  type TypedSchemaValue,
  v,
  validateSchemaGraph,
} from '../schema-model';
import type { FluentCodec } from '../../fluent/schema/codec';
import { toolBuildError } from './errors';

export type {
  CommandAnnotations,
  Doc,
  DuplicateKeyPolicy,
  ErrorKind,
  FlagSpec,
  Formatter,
  Quantifier,
  Repetition,
  StreamSpec,
};

/** A metadata-time value kept with the codec that gives it its wire meaning. */
export interface CodecValue<T = unknown> {
  readonly codec: FluentCodec;
  readonly value: T;
}

export function codecValue<T>(codec: FluentCodec, value: T): CodecValue<T> {
  return { codec, value };
}

export interface ExtendedToolRuntime {
  readonly bindings: readonly CommandBinding[];
  readonly subtreeForwards: readonly SubtreeForward[];
}

export interface CommandBinding {
  readonly commandPath: readonly string[];
  readonly handler: (input: Record<string, unknown>, context: unknown) => unknown;
}

export interface SubtreeForward {
  readonly pathPrefix: readonly string[];
  readonly childToolName: string;
}

export interface ExtendedCommandNode {
  readonly name: string;
  readonly aliases: readonly string[];
  readonly doc: Doc;
  readonly globals: ExtendedGlobals;
  readonly subcommands: readonly ExtendedCommandNode[];
  readonly body?: ExtendedCommandBody;
}

export interface ExtendedGlobals {
  readonly options: readonly ExtendedOptionSpec[];
  readonly flags: readonly FlagSpec[];
}

export interface ExtendedCommandBody {
  readonly positionals: ExtendedPositionals;
  readonly options: readonly ExtendedOptionSpec[];
  readonly flags: readonly FlagSpec[];
  readonly constraints: readonly ExtendedConstraint[];
  readonly stdin?: StreamSpec;
  readonly stdout?: StreamSpec;
  readonly result?: ExtendedResultSpec;
  readonly errors: readonly ExtendedErrorCase[];
  readonly annotations?: CommandAnnotations;
}

export interface ExtendedPositionals {
  readonly fixed: readonly ExtendedPositional[];
  readonly tail?: ExtendedTailPositional;
}

export interface ExtendedPositional {
  readonly name: string;
  readonly doc: Doc;
  readonly valueName?: string;
  readonly codec: FluentCodec;
  readonly default?: CodecValue;
  readonly required: boolean;
  readonly acceptsStdio: boolean;
}

export interface ExtendedTailPositional {
  readonly name: string;
  readonly doc: Doc;
  readonly valueName?: string;
  readonly itemCodec: FluentCodec;
  readonly min: number;
  readonly max?: number;
  readonly separator?: string;
  readonly verbatim: boolean;
  readonly acceptsStdio: boolean;
}

export interface ExtendedOptionSpec {
  readonly long: string;
  readonly short?: string;
  readonly aliases: readonly string[];
  readonly doc: Doc;
  readonly valueName?: string;
  readonly shape: ExtendedOptionShape;
  readonly default?: CodecValue;
  readonly required: boolean;
  readonly envVar?: string;
}

export type ExtendedOptionShape =
  | { readonly tag: 'scalar'; readonly codec: FluentCodec }
  | { readonly tag: 'optional-scalar'; readonly codec: FluentCodec }
  | {
      readonly tag: 'repeatable-list';
      readonly repetition: Repetition;
      readonly itemCodec: FluentCodec;
    }
  | {
      readonly tag: 'repeatable-map';
      readonly repetition: Repetition;
      readonly mapCodec: FluentCodec;
      readonly valueCodec: FluentCodec;
      readonly duplicateKeyPolicy: DuplicateKeyPolicy;
    };

export interface ExtendedResultSpec {
  readonly codec: FluentCodec;
  readonly doc: Doc;
  readonly formatters: readonly Formatter[];
  readonly defaultFormatter: string;
}

export interface ExtendedErrorCase {
  readonly name: string;
  readonly doc: Doc;
  readonly kind: ErrorKind;
  readonly exitCode: number;
  readonly payloadCodec?: FluentCodec;
}

export type ExtendedValueIsLiteral =
  | { readonly tag: 'deferred'; readonly value: unknown }
  | {
      readonly tag: 'resolved';
      readonly codec: FluentCodec;
      readonly value: unknown;
      readonly schemaValue: SchemaValue;
    };

export type ExtendedRef =
  | { readonly tag: 'present'; readonly name: string }
  | { readonly tag: 'value-is'; readonly name: string; readonly value: ExtendedValueIsLiteral };

export type ExtendedConstraint =
  | { readonly tag: 'requires-all'; readonly refs: readonly ExtendedRef[] }
  | { readonly tag: 'all-or-none'; readonly refs: readonly ExtendedRef[] }
  | { readonly tag: 'requires-any'; readonly refs: readonly ExtendedRef[] }
  | { readonly tag: 'mutex-groups'; readonly groups: readonly (readonly ExtendedRef[])[] }
  | {
      readonly tag: 'implies';
      readonly lhsQuant: Quantifier;
      readonly lhs: readonly ExtendedRef[];
      readonly rhsQuant: Quantifier;
      readonly rhs: readonly ExtendedRef[];
    }
  | {
      readonly tag: 'forbids';
      readonly lhsQuant: Quantifier;
      readonly lhs: readonly ExtendedRef[];
      readonly rhs: readonly ExtendedRef[];
    };

export type EffectiveCommandField =
  | { readonly tag: 'option'; readonly option: ExtendedOptionSpec }
  | { readonly tag: 'flag'; readonly flag: FlagSpec };

export interface CanonicalInputField {
  readonly name: string;
  readonly aliases: readonly string[];
  readonly codec: FluentCodec;
}

export interface CanonicalInputValue {
  readonly name: string;
  readonly aliases: readonly string[];
  readonly codec: FluentCodec;
  readonly value: unknown;
  readonly schemaValue: SchemaValue;
}

export type ValueIsMode = 'exact' | 'whole-or-one-peel';

/** Ordered canonical record codec shared by invocation and typed clients. */
export class CanonicalInputModel {
  readonly codec: FluentCodec;

  constructor(readonly fields: readonly CanonicalInputField[]) {
    fields.forEach((entry) =>
      assertCanonicalGraph(entry.codec.graph, `canonical input field "${entry.name}"`),
    );
    let defs: SchemaGraph['defs'];
    try {
      defs = mergeGraphDefs(fields.map((entry) => entry.codec.graph));
    } catch (error) {
      toolBuildError(
        'schema-conflict',
        `canonical input schema merge failed: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
    const graph = {
      defs,
      root: t.record(fields.map((entry) => field(entry.name, entry.codec.graph.root))),
    };
    assertCanonicalGraph(graph, 'canonical input record');
    this.codec = {
      graph,
      toValue: (input) => {
        const record = input as Record<string, unknown>;
        return v.record(
          fields.map((entry) => {
            if (!Object.prototype.hasOwnProperty.call(record, entry.name)) {
              throw new Error(`missing canonical tool input field \`${entry.name}\``);
            }
            return entry.codec.toValue(record[entry.name]);
          }),
        );
      },
      fromValue: (input) => {
        if (input.tag !== 'record') {
          throw new Error('tool input must be a positional record');
        }
        if (input.fields.length !== fields.length) {
          throw new Error(
            `tool input record has ${input.fields.length} fields, expected ${fields.length} canonical fields`,
          );
        }
        return Object.fromEntries(
          fields.map((entry, index) => [entry.name, entry.codec.fromValue(input.fields[index])]),
        );
      },
    };
  }

  encode(input: Record<string, unknown>): SchemaValue {
    return this.codec.toValue(input);
  }

  encodeTyped(input: Record<string, unknown>): TypedSchemaValue {
    return { graph: this.codec.graph, value: this.encode(input) };
  }

  decode(input: SchemaValue): Record<string, unknown> {
    return this.codec.fromValue(input) as Record<string, unknown>;
  }

  decodeValues(input: SchemaValue): CanonicalInputValue[] {
    if (input.tag !== 'record') throw new Error('tool input must be a positional record');
    if (input.fields.length !== this.fields.length) {
      throw new Error(
        `tool input record has ${input.fields.length} fields, expected ${this.fields.length} canonical fields`,
      );
    }
    return this.fields.map((entry, index) => ({
      ...entry,
      value: entry.codec.fromValue(input.fields[index]),
      schemaValue: input.fields[index],
    }));
  }

  /**
   * Rebuild this model's positional record from fields decoded by another
   * canonical model. Forwarding matches canonical names and aliases, but only
   * reuses a value when the complete per-field schema graph is identical.
   */
  forwardValues(input: readonly CanonicalInputValue[]): TypedSchemaValue {
    const values = this.fields.map((field) => {
      const value = input.find((candidate) => canonicalSurfacesOverlap(field, candidate));
      if (!value) {
        throw new Error(`missing canonical tool input field \`${field.name}\``);
      }
      if (!deepEqual(value.codec.graph, field.codec.graph)) {
        throw new Error(
          `canonical tool input field \`${value.name}\` has incompatible schema for forwarded field \`${field.name}\``,
        );
      }
      return value.schemaValue;
    });
    return { graph: this.codec.graph, value: v.record(values) };
  }
}

function assertCanonicalGraph(graph: SchemaGraph, position: string): void {
  const error = validateSchemaGraph(graph)[0];
  if (!error) return;
  toolBuildError(
    error.code === 'dangling-ref' ? 'unresolved-type-ref' : 'ill-formed-schema',
    `${position}: ${error.message}`,
  );
}

function canonicalSurfacesOverlap(
  left: Pick<CanonicalInputField, 'name' | 'aliases'>,
  right: Pick<CanonicalInputField, 'name' | 'aliases'>,
): boolean {
  return (
    left.name === right.name ||
    left.aliases.includes(right.name) ||
    right.aliases.includes(left.name) ||
    left.aliases.some((alias) => right.aliases.includes(alias))
  );
}

export type ToolHelpArgumentKind =
  | 'global-option'
  | 'global-flag'
  | 'positional'
  | 'tail'
  | 'option'
  | 'flag';

export interface ToolHelpArgument {
  readonly kind: ToolHelpArgumentKind;
  readonly name: string;
  readonly aliases: readonly string[];
  readonly doc: Doc;
  readonly required?: boolean;
  readonly valueName?: string;
  readonly default?: CodecValue;
  readonly envVar?: string;
}

export interface ToolHelpProjection {
  readonly command: ExtendedCommandNode;
  readonly commandPath: readonly string[];
  readonly arguments: readonly ToolHelpArgument[];
  readonly subcommands: readonly ExtendedCommandNode[];
}

export class ExtendedToolType {
  constructor(
    readonly version: string,
    readonly root: ExtendedCommandNode,
  ) {}

  get toolName(): string {
    return this.root.name;
  }

  /** Resolve a root-excluded command path by canonical name or alias. */
  commandByPath(
    commandPath: readonly string[],
    requireBody = true,
  ): ExtendedCommandNode | undefined {
    let current = this.root;
    for (const segment of commandPath) {
      const next = current.subcommands.find(
        (child) => child.name === segment || child.aliases.includes(segment),
      );
      if (!next) return undefined;
      current = next;
    }
    return requireBody && !current.body ? undefined : current;
  }

  /** Return the root-to-node object path, guarding against malformed cycles. */
  pathTo(target: ExtendedCommandNode): readonly ExtendedCommandNode[] | undefined {
    let targetPath: ExtendedCommandNode[] | undefined;
    const path: ExtendedCommandNode[] = [];
    const visited = new Set<ExtendedCommandNode>();
    const onStack = new Set<ExtendedCommandNode>();
    const visit = (node: ExtendedCommandNode): void => {
      if (onStack.has(node)) {
        toolBuildError('command-tree-cycle', `the command tree contains a cycle at ${node.name}`);
      }
      if (visited.has(node)) {
        toolBuildError(
          'duplicate-command-parent',
          `command "${node.name}" has more than one parent`,
        );
      }
      visited.add(node);
      onStack.add(node);
      path.push(node);
      if (node === target) targetPath = [...path];
      node.subcommands.forEach(visit);
      path.pop();
      onStack.delete(node);
    };
    visit(this.root);
    return targetPath;
  }

  commandPath(target: ExtendedCommandNode): readonly string[] | undefined {
    return this.pathTo(target)
      ?.slice(1)
      .map((node) => node.name);
  }

  effectiveGlobals(target: ExtendedCommandNode): EffectiveCommandField[] {
    return (this.pathTo(target) ?? []).flatMap((node) => [
      ...node.globals.options.map((option) => ({ tag: 'option' as const, option })),
      ...node.globals.flags.map((flag) => ({ tag: 'flag' as const, flag })),
    ]);
  }

  canonicalInputFields(target: ExtendedCommandNode): CanonicalInputField[] {
    const body = target.body;
    const localNames = new Set<string>();
    if (body) {
      body.positionals.fixed.forEach((positional) => localNames.add(positional.name));
      if (body.positionals.tail) localNames.add(body.positionals.tail.name);
      body.options.forEach((option) => {
        localNames.add(option.long);
        option.aliases.forEach((alias) => localNames.add(alias));
      });
      body.flags.forEach((flag) => {
        localNames.add(flag.long);
        flag.aliases.forEach((alias) => localNames.add(alias));
      });
    }
    const globals = this.effectiveGlobals(target)
      .filter((entry) => {
        const names =
          entry.tag === 'option'
            ? [entry.option.long, ...entry.option.aliases]
            : [entry.flag.long, ...entry.flag.aliases];
        return names.every((name) => !localNames.has(name));
      })
      .map(
        (entry): CanonicalInputField =>
          entry.tag === 'option'
            ? {
                name: entry.option.long,
                aliases: entry.option.aliases,
                codec: canonicalOptionCodec(entry.option),
              }
            : {
                name: entry.flag.long,
                aliases: entry.flag.aliases,
                codec: flagCodec(entry.flag),
              },
      );
    if (!body) return globals;
    return [
      ...globals,
      ...body.positionals.fixed.map((positional) => ({
        name: positional.name,
        aliases: [],
        codec: canonicalPositionalCodec(positional),
      })),
      ...(body.positionals.tail
        ? [
            {
              name: body.positionals.tail.name,
              aliases: [],
              codec: listCodec(body.positionals.tail.itemCodec),
            },
          ]
        : []),
      ...body.options.map((option) => ({
        name: option.long,
        aliases: option.aliases,
        codec: canonicalOptionCodec(option),
      })),
      ...body.flags.map((flag) => ({
        name: flag.long,
        aliases: flag.aliases,
        codec: flagCodec(flag),
      })),
    ];
  }

  canonicalInputModel(target: ExtendedCommandNode): CanonicalInputModel {
    if (!this.pathTo(target)) throw new Error(`command not found: ${target.name}`);
    return new CanonicalInputModel(this.canonicalInputFields(target));
  }

  projectHelp(commandPath: readonly string[]): ToolHelpProjection | undefined {
    const command = this.commandByPath(commandPath, false);
    if (!command) return undefined;
    const arguments_: ToolHelpArgument[] = this.effectiveGlobals(command).map((entry) =>
      entry.tag === 'option'
        ? {
            kind: 'global-option',
            name: entry.option.long,
            aliases: entry.option.aliases,
            doc: entry.option.doc,
            required: entry.option.required,
            valueName: entry.option.valueName,
            default: entry.option.default,
            envVar: entry.option.envVar,
          }
        : {
            kind: 'global-flag',
            name: entry.flag.long,
            aliases: entry.flag.aliases,
            doc: entry.flag.doc,
            envVar: entry.flag.envVar,
          },
    );
    const body = command.body;
    if (body) {
      arguments_.push(
        ...body.positionals.fixed.map((entry) => ({
          kind: 'positional' as const,
          name: entry.name,
          aliases: [],
          doc: entry.doc,
          required: entry.required,
          valueName: entry.valueName,
          default: entry.default,
        })),
      );
      if (body.positionals.tail) {
        arguments_.push({
          kind: 'tail',
          name: body.positionals.tail.name,
          aliases: [],
          doc: body.positionals.tail.doc,
          required: body.positionals.tail.min > 0,
          valueName: body.positionals.tail.valueName,
        });
      }
      arguments_.push(
        ...body.options.map((entry) => ({
          kind: 'option' as const,
          name: entry.long,
          aliases: entry.aliases,
          doc: entry.doc,
          required: entry.required,
          valueName: entry.valueName,
          default: entry.default,
          envVar: entry.envVar,
        })),
        ...body.flags.map((entry) => ({
          kind: 'flag' as const,
          name: entry.long,
          aliases: entry.aliases,
          doc: entry.doc,
          envVar: entry.envVar,
        })),
      );
    }
    return {
      command,
      commandPath: [...commandPath],
      arguments: arguments_,
      subcommands: command.subcommands,
    };
  }
}

export function optionCollectedCodec(shape: ExtendedOptionShape): FluentCodec {
  switch (shape.tag) {
    case 'scalar':
    case 'optional-scalar':
      return shape.codec;
    case 'repeatable-list':
      return listCodec(shape.itemCodec);
    case 'repeatable-map':
      return shape.mapCodec;
  }
}

function canonicalOptionCodec(option: ExtendedOptionSpec): FluentCodec {
  const collected = optionCollectedCodec(option.shape);
  return !option.required && option.default === undefined && !isRepeatable(option.shape)
    ? optionalCanonicalFieldCodec(collected)
    : collected;
}

function canonicalPositionalCodec(positional: ExtendedPositional): FluentCodec {
  return !positional.required && positional.default === undefined
    ? optionalCanonicalFieldCodec(positional.codec)
    : positional.codec;
}

function isRepeatable(shape: ExtendedOptionShape): boolean {
  return shape.tag === 'repeatable-list' || shape.tag === 'repeatable-map';
}

/**
 * Optional tool arguments keep their declared inner graph but carry an option
 * value at invocation time. This matches the canonical Rust tool contract and
 * preserves graph equality when forwarding inherited arguments.
 */
export function optionalCanonicalFieldCodec(inner: FluentCodec): FluentCodec {
  return {
    graph: inner.graph,
    toValue: (input) => v.option(input === undefined ? undefined : inner.toValue(input)),
    fromValue: (input) => {
      if (input.tag !== 'option') throw new Error('expected an optional tool input value');
      return input.value === undefined ? undefined : inner.fromValue(input.value);
    },
  };
}

export function optionValueCodec(shape: ExtendedOptionShape): FluentCodec | undefined {
  switch (shape.tag) {
    case 'scalar':
    case 'optional-scalar':
      return shape.codec;
    case 'repeatable-list':
      return shape.itemCodec;
    case 'repeatable-map':
      return shape.valueCodec;
  }
}

/** Codecs accepted by `value-is`, in whole-value then one-level-peel order. */
export function valueIsCodecs(codec: FluentCodec, mode: ValueIsMode): FluentCodec[] {
  if (mode === 'exact') return [codec];
  let unwrapped = codec;
  while (unwrapped.optionInner) unwrapped = unwrapped.optionInner;
  const root = resolveCodecRoot(unwrapped);
  const peeled =
    root.body.tag === 'list' || root.body.tag === 'fixed-list'
      ? unwrapped.listItem
      : root.body.tag === 'map'
        ? unwrapped.mapValue
        : undefined;
  if (!peeled) return [codec];
  return [
    codec,
    {
      ...peeled,
      graph: {
        defs: mergeGraphDefs([codec.graph, peeled.graph]),
        root: peeled.graph.root,
      },
    },
  ];
}

export function resolveCodecRoot(codec: FluentCodec) {
  let current = codec.graph.root;
  const seen = new Set<string>();
  while (current.body.tag === 'ref') {
    if (seen.has(current.body.id)) throw new Error(`cyclic schema ref: ${current.body.id}`);
    seen.add(current.body.id);
    const definition = codec.graph.defs.get(current.body.id);
    if (!definition) throw new Error(`unresolved schema ref: ${current.body.id}`);
    current = definition.body;
  }
  return current;
}

export function listCodec(itemCodec: FluentCodec): FluentCodec {
  return {
    graph: { defs: itemCodec.graph.defs, root: t.list(itemCodec.graph.root) },
    listItem: itemCodec,
    toValue: (input) => v.list((input as unknown[]).map((item) => itemCodec.toValue(item))),
    fromValue: (input) => {
      if (input.tag !== 'list') throw new Error('expected a list schema value');
      return input.elements.map((item) => itemCodec.fromValue(item));
    },
  };
}

export function flagCodec(flag: FlagSpec): FluentCodec {
  return flag.shape.tag === 'bool-flag'
    ? {
        graph: { defs: new Map(), root: t.bool() },
        toValue: (input) => v.bool(input as boolean),
        fromValue: (input) => (input as Extract<SchemaValue, { tag: 'bool' }>).value,
      }
    : {
        graph: { defs: new Map(), root: t.u32() },
        toValue: (input) => v.u32(input as number),
        fromValue: (input) => (input as Extract<SchemaValue, { tag: 'u32' }>).value,
      };
}

export function emptyDoc(summary = '', description = ''): Doc {
  return { summary, description, examples: [] };
}

export function emptyGlobals(): ExtendedGlobals {
  return { options: [], flags: [] };
}

export function emptyPositionals(): ExtendedPositionals {
  return { fixed: [] };
}
