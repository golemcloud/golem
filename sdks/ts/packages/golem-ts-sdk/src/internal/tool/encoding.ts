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
  CommandBody,
  CommandNode,
  Constraint,
  Doc,
  ErrorCase,
  FlagSpec,
  Globals,
  OptionShape,
  OptionSpec,
  Positional,
  Ref,
  ResultSpec,
  TailPositional,
  Tool,
} from 'golem:tool/common@0.1.0';
import type { FluentCodec } from '../../fluent/schema/codec';
import {
  cloneSchemaValue,
  GraphEncoder,
  mergeGraphDefs,
  type SchemaGraph,
  type SchemaValue,
  schemaValueToWit,
  t,
} from '../schema-model';
import { normalizeExtendedTool } from './composition';
import { ToolBuildError, toolBuildError } from './errors';
import { optionCollectedCodec } from './model';
import type {
  CodecValue,
  ExtendedCommandBody,
  ExtendedCommandNode,
  ExtendedConstraint,
  ExtendedErrorCase,
  ExtendedGlobals,
  ExtendedOptionShape,
  ExtendedOptionSpec,
  ExtendedPositional,
  ExtendedRef,
  ExtendedResultSpec,
  ExtendedTailPositional,
  ExtendedToolType,
} from './model';
import { parseSourceValue, schemaValueConforms } from './validation';

interface EncodingContext {
  readonly encoder: GraphEncoder;
  readonly graphByCodec: ReadonlyMap<FluentCodec, SchemaGraph>;
}

/** Encode an extended tool tree into the flat `golem:tool/common@0.1.0` carrier. */
export function encodeTool(source: ExtendedToolType): Tool {
  try {
    const tool = normalizeExtendedTool(source);
    const nodes = flattenCommands(tool.root);
    const codecs = collectSchemaCodecs(nodes);

    const context = createEncodingContext(codecs);
    const indexByNode = new Map(nodes.map((node, index) => [node, index]));
    const encodedNodes = nodes.map((node) => encodeCommand(node, indexByNode, context));
    return {
      version: tool.version,
      commands: { nodes: encodedNodes },
      schema: context.encoder.finish(),
    };
  } catch (error) {
    if (error instanceof ToolBuildError) throw error;
    toolBuildError('encode-error', `tool metadata encode error: ${errorMessage(error)}`);
  }
}

function flattenCommands(root: ExtendedCommandNode): ExtendedCommandNode[] {
  const nodes: ExtendedCommandNode[] = [];
  const visit = (node: ExtendedCommandNode): void => {
    nodes.push(node);
    node.subcommands.forEach(visit);
  };
  visit(root);
  return nodes;
}

function createEncodingContext(codecs: readonly FluentCodec[]): EncodingContext {
  const graphByCodec = new Map(codecs.map((codec) => [codec, codec.graph]));
  const encoder = new GraphEncoder(mergeGraphDefs(graphByCodec.values()));
  return { encoder, graphByCodec };
}

function collectSchemaCodecs(nodes: readonly ExtendedCommandNode[]): FluentCodec[] {
  const codecs: FluentCodec[] = [];
  const collectCodec = (codec: FluentCodec): void => {
    codecs.push(codec);
  };
  const collectValue = (value: CodecValue | undefined): void => {
    if (value) collectCodec(value.codec);
  };
  const collectOption = (option: ExtendedOptionSpec): void => {
    switch (option.shape.tag) {
      case 'scalar':
      case 'optional-scalar':
        collectCodec(option.shape.codec);
        break;
      case 'repeatable-list':
        collectCodec(option.shape.itemCodec);
        break;
      case 'repeatable-map':
        collectCodec(option.shape.mapCodec);
        collectCodec(option.shape.valueCodec);
        break;
    }
    collectValue(option.default);
  };
  const collectRef = (ref: ExtendedRef): void => {
    if (ref.tag === 'value-is' && ref.value.tag === 'resolved') {
      collectCodec(ref.value.codec);
    }
  };
  const collectConstraint = (constraint: ExtendedConstraint): void => {
    switch (constraint.tag) {
      case 'requires-all':
      case 'all-or-none':
      case 'requires-any':
        constraint.refs.forEach(collectRef);
        break;
      case 'mutex-groups':
        constraint.groups.forEach((group) => group.forEach(collectRef));
        break;
      case 'implies':
      case 'forbids':
        constraint.lhs.forEach(collectRef);
        constraint.rhs.forEach(collectRef);
        break;
    }
  };

  nodes.forEach((node) => {
    node.globals.options.forEach(collectOption);
    const body = node.body;
    if (!body) return;
    body.positionals.fixed.forEach((positional) => {
      collectCodec(positional.codec);
      collectValue(positional.default);
    });
    if (body.positionals.tail) collectCodec(body.positionals.tail.itemCodec);
    body.options.forEach(collectOption);
    body.constraints.forEach(collectConstraint);
    if (body.result) collectCodec(body.result.codec);
    body.errors.forEach((errorCase) => {
      if (errorCase.payloadCodec) collectCodec(errorCase.payloadCodec);
    });
  });
  return codecs;
}

function encodeCommand(
  node: ExtendedCommandNode,
  indexByNode: ReadonlyMap<ExtendedCommandNode, number>,
  context: EncodingContext,
): CommandNode {
  return {
    name: node.name,
    aliases: [...node.aliases],
    doc: cloneDoc(node.doc),
    globals: encodeGlobals(node.globals, context),
    subcommands: node.subcommands.map((child) => {
      const index = indexByNode.get(child);
      if (index === undefined) {
        toolBuildError('encode-error', `subcommand "${child.name}" was not flattened`);
      }
      return index;
    }),
    body: node.body ? encodeBody(node.body, context) : undefined,
  };
}

function encodeGlobals(globals: ExtendedGlobals, context: EncodingContext): Globals {
  return {
    options: globals.options.map((option) => encodeOption(option, context)),
    flags: globals.flags.map(encodeFlag),
  };
}

function encodeBody(body: ExtendedCommandBody, context: EncodingContext): CommandBody {
  return {
    positionals: {
      fixed: body.positionals.fixed.map((positional) => encodePositional(positional, context)),
      tail: body.positionals.tail ? encodeTail(body.positionals.tail, context) : undefined,
    },
    options: body.options.map((option) => encodeOption(option, context)),
    flags: body.flags.map(encodeFlag),
    constraints: body.constraints.map(encodeConstraint),
    stdin: body.stdin
      ? { ...body.stdin, doc: cloneDoc(body.stdin.doc), mime: [...body.stdin.mime] }
      : undefined,
    stdout: body.stdout
      ? { ...body.stdout, doc: cloneDoc(body.stdout.doc), mime: [...body.stdout.mime] }
      : undefined,
    result: body.result ? encodeResult(body.result, context) : undefined,
    errors: body.errors.map((errorCase) => encodeError(errorCase, context)),
    annotations: body.annotations ? { ...body.annotations } : undefined,
  };
}

function encodePositional(positional: ExtendedPositional, context: EncodingContext): Positional {
  const graph = codecGraph(context, positional.codec);
  return {
    name: positional.name,
    doc: cloneDoc(positional.doc),
    valueName: positional.valueName,
    type: context.encoder.encodeType(graph.root),
    default_: positional.default
      ? encodeCodecValue(positional.default, positional.codec, graph)
      : undefined,
    required: positional.required,
    acceptsStdio: positional.acceptsStdio,
  };
}

function encodeTail(tail: ExtendedTailPositional, context: EncodingContext): TailPositional {
  return {
    name: tail.name,
    doc: cloneDoc(tail.doc),
    valueName: tail.valueName,
    itemType: context.encoder.encodeType(codecGraph(context, tail.itemCodec).root),
    min: tail.min,
    max: tail.max,
    separator: tail.separator,
    verbatim: tail.verbatim,
    acceptsStdio: tail.acceptsStdio,
  };
}

function encodeOption(option: ExtendedOptionSpec, context: EncodingContext): OptionSpec {
  const collectedGraph = optionCollectedGraph(context, option.shape);
  const collectedCodec = optionCollectedCodec(option.shape);
  return {
    long: option.long,
    short: option.short,
    aliases: [...option.aliases],
    doc: cloneDoc(option.doc),
    valueName: option.valueName,
    shape: encodeOptionShape(option.shape, context),
    default_: option.default
      ? encodeCodecValue(option.default, collectedCodec, collectedGraph)
      : undefined,
    required: option.required,
    envVar: option.envVar,
  };
}

function encodeOptionShape(shape: ExtendedOptionShape, context: EncodingContext): OptionShape {
  switch (shape.tag) {
    case 'scalar':
      return {
        tag: 'scalar',
        val: context.encoder.encodeType(codecGraph(context, shape.codec).root),
      };
    case 'optional-scalar':
      return {
        tag: 'optional-scalar',
        val: context.encoder.encodeType(codecGraph(context, shape.codec).root),
      };
    case 'repeatable-list':
      return {
        tag: 'repeatable-list',
        val: {
          repetition: { ...shape.repetition },
          itemType: context.encoder.encodeType(codecGraph(context, shape.itemCodec).root),
        },
      };
    case 'repeatable-map':
      return {
        tag: 'repeatable-map',
        val: {
          repetition: { ...shape.repetition },
          mapType: context.encoder.encodeType(codecGraph(context, shape.mapCodec).root),
          duplicateKeyPolicy: shape.duplicateKeyPolicy,
        },
      };
  }
}

function encodeResult(result: ExtendedResultSpec, context: EncodingContext): ResultSpec {
  return {
    type: context.encoder.encodeType(codecGraph(context, result.codec).root),
    doc: cloneDoc(result.doc),
    formatters: result.formatters.map((formatter) => ({
      name: formatter.name,
      doc: cloneDoc(formatter.doc),
    })),
    defaultFormatter: result.defaultFormatter,
  };
}

function encodeError(errorCase: ExtendedErrorCase, context: EncodingContext): ErrorCase {
  return {
    name: errorCase.name,
    doc: cloneDoc(errorCase.doc),
    kind: errorCase.kind,
    exitCode: errorCase.exitCode,
    payload: errorCase.payloadCodec
      ? context.encoder.encodeType(codecGraph(context, errorCase.payloadCodec).root)
      : undefined,
  };
}

function encodeConstraint(constraint: ExtendedConstraint): Constraint {
  switch (constraint.tag) {
    case 'requires-all':
    case 'all-or-none':
    case 'requires-any':
      return { tag: constraint.tag, val: constraint.refs.map(encodeRef) };
    case 'mutex-groups':
      return {
        tag: 'mutex-groups',
        val: constraint.groups.map((refs) => ({ refs: refs.map(encodeRef) })),
      };
    case 'implies':
      return {
        tag: 'implies',
        val: {
          lhsQuant: constraint.lhsQuant,
          lhs: constraint.lhs.map(encodeRef),
          rhsQuant: constraint.rhsQuant,
          rhs: constraint.rhs.map(encodeRef),
        },
      };
    case 'forbids':
      return {
        tag: 'forbids',
        val: {
          lhsQuant: constraint.lhsQuant,
          lhs: constraint.lhs.map(encodeRef),
          rhs: constraint.rhs.map(encodeRef),
        },
      };
  }
}

function encodeRef(ref: ExtendedRef): Ref {
  if (ref.tag === 'present') return { tag: 'present', val: ref.name };
  if (ref.value.tag === 'deferred') {
    toolBuildError(
      'unresolved-value-is-literal',
      `value-is literal for argument "${ref.name}" was not resolved during normalization`,
    );
  }
  return {
    tag: 'value-is',
    val: {
      name: ref.name,
      value: schemaValueToWit(cloneSchemaValue(ref.value.schemaValue)),
    },
  };
}

function codecGraph(context: EncodingContext, codec: FluentCodec): SchemaGraph {
  const graph = context.graphByCodec.get(codec);
  if (!graph) toolBuildError('encode-error', 'tool codec graph was not collected');
  return graph;
}

function optionCollectedGraph(context: EncodingContext, shape: ExtendedOptionShape): SchemaGraph {
  switch (shape.tag) {
    case 'scalar':
    case 'optional-scalar':
      return codecGraph(context, shape.codec);
    case 'repeatable-list': {
      const item = codecGraph(context, shape.itemCodec);
      return { defs: item.defs, root: t.list(item.root) };
    }
    case 'repeatable-map':
      return codecGraph(context, shape.mapCodec);
  }
}

function encodeCodecValue(
  value: CodecValue,
  expectedCodec: FluentCodec,
  expectedGraph: SchemaGraph,
) {
  const sourceValue = parseSourceValue(expectedCodec, value.value);
  if (sourceValue.tag === 'invalid') {
    toolBuildError('default-type-mismatch', 'default value does not satisfy its source schema');
  }
  let encoded: SchemaValue;
  try {
    encoded = cloneSchemaValue(expectedCodec.toValue(sourceValue.value));
  } catch (error) {
    toolBuildError('default-type-mismatch', errorMessage(error));
  }
  if (!schemaValueConforms(expectedGraph, expectedGraph.root, encoded)) {
    toolBuildError('default-type-mismatch', 'default value does not match its declared schema');
  }
  return schemaValueToWit(encoded);
}

function encodeFlag(flag: FlagSpec): FlagSpec {
  return {
    ...flag,
    aliases: [...flag.aliases],
    doc: cloneDoc(flag.doc),
    shape:
      flag.shape.tag === 'bool-flag'
        ? { tag: 'bool-flag', val: { ...flag.shape.val } }
        : { tag: 'count-flag', val: flag.shape.val },
  };
}

function cloneDoc(doc: Doc): Doc {
  return {
    summary: doc.summary,
    description: doc.description,
    examples: doc.examples.map((example) => ({ ...example })),
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
