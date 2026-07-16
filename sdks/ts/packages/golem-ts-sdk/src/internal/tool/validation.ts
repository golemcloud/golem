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

import type { FluentCodec } from '../../fluent/schema/codec';
import {
  assertSchemaValueRepresentable,
  deepEqual,
  floatFromBits,
  isQuantityValueRepresentable,
  mergeGraphDefs,
  quantityLessOrEqual,
  type NumericBound,
  type NumericRestrictions,
  type QuantityValue,
  type SchemaGraph,
  type SchemaType,
  type SchemaValidationError,
  type SchemaValue,
  type UnionBranch,
  validateSchemaGraph,
} from '../schema-model';
import { ToolBuildError, type ToolBuildErrorCode, toolBuildError } from './errors';
import {
  type CodecValue,
  type ExtendedCommandBody,
  type ExtendedCommandNode,
  type ExtendedConstraint,
  type ExtendedGlobals,
  type ExtendedOptionSpec,
  type ExtendedRef,
  type ExtendedToolType,
  type Quantifier,
  type Repetition,
  type ValueIsMode,
  optionCollectedCodec,
  optionValueCodec,
  resolveCodecRoot,
  valueIsCodecs,
} from './model';

interface ValueScopeEntry {
  readonly codec?: FluentCodec;
  readonly mode?: ValueIsMode;
}

/** Validate all producer-owned invariants of a normalized extended tool tree. */
export function validateExtendedTool(tool: ExtendedToolType): void {
  validateTreeShape(tool.root);
  const codecs: FluentCodec[] = [];
  visitCommand(tool.root, [], codecs);
  try {
    mergeGraphDefs(codecs.map((codec) => codec.graph));
  } catch (error) {
    toolBuildError('schema-conflict', `tool schema merge failed: ${errorMessage(error)}`);
  }
}

function validateTreeShape(root: ExtendedCommandNode): void {
  const visited = new Map<ExtendedCommandNode, number>();
  const onStack = new Set<ExtendedCommandNode>();
  let nextIndex = 0;
  const visit = (node: ExtendedCommandNode): void => {
    const index = visited.get(node);
    if (onStack.has(node)) {
      toolBuildError('command-tree-cycle', `the command tree contains a cycle at node ${index}`);
    }
    if (index !== undefined) {
      toolBuildError('duplicate-command-parent', `command node ${index} has more than one parent`);
    }
    visited.set(node, nextIndex++);
    onStack.add(node);
    node.subcommands.forEach(visit);
    onStack.delete(node);
  };
  visit(root);
}

function visitCommand(
  node: ExtendedCommandNode,
  ancestorGlobals: readonly ExtendedGlobals[],
  codecs: FluentCodec[],
): void {
  checkIdentifier('command name', node.name);
  node.aliases.forEach((alias) => checkIdentifier('command alias', alias));
  checkSiblingNames(node);
  validateGlobals(node.globals, ancestorGlobals, codecs);

  const inScope = [...ancestorGlobals, node.globals];
  if (node.body) validateBody(node.body, inScope, codecs);
  node.subcommands.forEach((child) => visitCommand(child, inScope, codecs));
}

function validateGlobals(
  globals: ExtendedGlobals,
  ancestors: readonly ExtendedGlobals[],
  codecs: FluentCodec[],
): void {
  const names = new Set<string>();
  const shorts = new Set<string>();
  ancestors.forEach((entry) => seedGlobals(entry, names, shorts));
  globals.options.forEach((option) => {
    validateOption(option, codecs);
    insertSurface(option.long, option.aliases, option.short, names, shorts);
  });
  globals.flags.forEach((flag) => {
    validateFlag(flag);
    insertSurface(flag.long, flag.aliases, flag.short, names, shorts);
  });
}

function validateBody(
  body: ExtendedCommandBody,
  inScopeGlobals: readonly ExtendedGlobals[],
  codecs: FluentCodec[],
): void {
  const names = new Set<string>();
  const shorts = new Set<string>();
  const valueScope = new Map<string, ValueScopeEntry>();
  inScopeGlobals.forEach((globals) => {
    seedGlobals(globals, names, shorts);
    globals.options.forEach((option) => registerOption(valueScope, option));
    globals.flags.forEach((flag) => registerFlag(valueScope, flag));
  });

  body.options.forEach((option) => {
    validateOption(option, codecs);
    insertSurface(option.long, option.aliases, option.short, names, shorts);
    registerOption(valueScope, option);
  });
  body.flags.forEach((flag) => {
    validateFlag(flag);
    insertSurface(flag.long, flag.aliases, flag.short, names, shorts);
    registerFlag(valueScope, flag);
  });

  let sawOptional = false;
  body.positionals.fixed.forEach((positional) => {
    checkIdentifier('positional name', positional.name);
    insertName(names, positional.name);
    validateCodec(positional.codec, `positional ${positional.name}`);
    rejectVariantInput(positional.codec, positional.name);
    codecs.push(positional.codec);
    if (positional.default) {
      validateCodecValue(positional.default, positional.codec, 'default-type-mismatch');
      codecs.push(positional.default.codec);
    }
    valueScope.set(positional.name, {
      codec: positional.codec,
      mode: 'whole-or-one-peel',
    });
    if (positional.required && sawOptional) {
      toolBuildError(
        'required-positional-after-optional',
        `required positional "${positional.name}" cannot appear after an optional positional`,
      );
    }
    if (!positional.required) sawOptional = true;
  });

  const tail = body.positionals.tail;
  if (tail) {
    checkIdentifier('positional name', tail.name);
    insertName(names, tail.name);
    validateCodec(tail.itemCodec, `tail ${tail.name}`);
    rejectVariantInput(tail.itemCodec, tail.name);
    codecs.push(tail.itemCodec);
    valueScope.set(tail.name, { codec: tail.itemCodec, mode: 'exact' });
    if (tail.verbatim && tail.separator === undefined) {
      toolBuildError(
        'verbatim-without-separator',
        `verbatim tail positional has no separator: ${tail.name}`,
      );
    }
    checkIntegerRange(
      'tail minimum occurrence count',
      tail.min,
      0,
      0xffff_ffff,
      'invalid-tail-occurrence-bounds',
    );
    if (tail.max !== undefined) {
      checkIntegerRange(
        'tail maximum occurrence count',
        tail.max,
        0,
        0xffff_ffff,
        'invalid-tail-occurrence-bounds',
      );
    }
    if (tail.max !== undefined && tail.min > tail.max) {
      toolBuildError(
        'invalid-tail-occurrence-bounds',
        `tail positional "${tail.name}" has an impossible occurrence range: min ${tail.min} is greater than max ${tail.max}`,
      );
    }
  }

  body.constraints.forEach((constraint) => validateConstraint(constraint, valueScope, codecs));

  if (body.result) {
    validateCodec(body.result.codec, 'result');
    codecs.push(body.result.codec);
    const formatterNames = new Set<string>();
    body.result.formatters.forEach((formatter) => {
      checkIdentifier('formatter name', formatter.name);
      insertName(formatterNames, formatter.name);
    });
    if (!formatterNames.has(body.result.defaultFormatter)) {
      toolBuildError(
        'unresolved-default-formatter',
        `default-formatter is not declared: ${body.result.defaultFormatter}`,
      );
    }
  }

  const errorNames = new Set<string>();
  body.errors.forEach((errorCase) => {
    checkIdentifier('error-case name', errorCase.name);
    insertName(errorNames, errorCase.name);
    checkOneOf('error kind', errorCase.kind, ['usage-error', 'runtime-error']);
    checkIntegerRange('error exit code', errorCase.exitCode, 0, 0xff);
    if (errorCase.payloadCodec) {
      validateCodec(errorCase.payloadCodec, `error ${errorCase.name}`);
      codecs.push(errorCase.payloadCodec);
    }
  });
}

function validateOption(option: ExtendedOptionSpec, codecs: FluentCodec[]): void {
  checkIdentifier('option long name', option.long);
  option.aliases.forEach((alias) => checkIdentifier('option alias', alias));
  switch (option.shape.tag) {
    case 'scalar':
    case 'optional-scalar':
      break;
    case 'repeatable-list':
      validateRepetition(option.shape.repetition);
      break;
    case 'repeatable-map':
      validateRepetition(option.shape.repetition);
      checkOneOf('duplicate-key policy', option.shape.duplicateKeyPolicy, ['reject', 'last-wins']);
      break;
    default:
      invalidMetadata('option shape', (option.shape as { readonly tag?: unknown }).tag);
  }
  const authored =
    option.shape.tag === 'repeatable-map' ? option.shape.mapCodec : optionValueCodec(option.shape)!;
  validateCodec(authored, `option --${option.long}`);
  codecs.push(authored);
  if (option.shape.tag === 'repeatable-map') {
    let root: SchemaType;
    try {
      root = resolveCodecRoot(option.shape.mapCodec);
    } catch (error) {
      toolBuildError('ill-formed-schema', errorMessage(error));
    }
    if (root.body.tag !== 'map') {
      toolBuildError(
        'repeatable-map-type-not-map',
        `repeatable-map option does not collect into a map: ${option.long}`,
      );
    }
    if (!deepEqual(root.body.value, option.shape.valueCodec.graph.root)) {
      toolBuildError(
        'repeatable-map-type-not-map',
        `repeatable-map value codec does not match the map value type: ${option.long}`,
      );
    }
    validateCodec(option.shape.valueCodec, `option --${option.long} map value`);
    codecs.push(option.shape.valueCodec);
  }
  const inputCodec = optionValueCodec(option.shape);
  if (inputCodec) rejectVariantInput(inputCodec, option.long);
  if (option.default) {
    const collected = optionCollectedCodec(option.shape);
    validateCodecValue(option.default, collected, 'default-type-mismatch');
    codecs.push(option.default.codec);
  }
}

function validateFlag(flag: ExtendedGlobals['flags'][number]): void {
  checkIdentifier('flag long name', flag.long);
  flag.aliases.forEach((alias) => checkIdentifier('flag alias', alias));
  switch (flag.shape.tag) {
    case 'bool-flag':
      if (
        typeof flag.shape.val.default_ !== 'boolean' ||
        typeof flag.shape.val.negatable !== 'boolean'
      ) {
        invalidMetadata('boolean flag shape', flag.shape.val);
      }
      break;
    case 'count-flag':
      if (flag.shape.val !== undefined) {
        checkIntegerRange('count-flag maximum', flag.shape.val, 0, 0xffff_ffff);
      }
      break;
    default:
      invalidMetadata('flag shape', (flag.shape as { readonly tag?: unknown }).tag);
  }
}

function validateConstraint(
  constraint: ExtendedConstraint,
  scope: ReadonlyMap<string, ValueScopeEntry>,
  codecs: FluentCodec[],
): void {
  switch (constraint.tag) {
    case 'requires-all':
    case 'all-or-none':
    case 'requires-any':
      constraint.refs.forEach((ref) => validateRef(ref, scope, codecs));
      break;
    case 'mutex-groups':
      constraint.groups.flat().forEach((ref) => validateRef(ref, scope, codecs));
      break;
    case 'implies':
      checkQuantifier('left-hand quantifier', constraint.lhsQuant);
      checkQuantifier('right-hand quantifier', constraint.rhsQuant);
      [...constraint.lhs, ...constraint.rhs].forEach((ref) => validateRef(ref, scope, codecs));
      break;
    case 'forbids':
      checkQuantifier('left-hand quantifier', constraint.lhsQuant);
      [...constraint.lhs, ...constraint.rhs].forEach((ref) => validateRef(ref, scope, codecs));
      break;
    default:
      invalidMetadata('constraint', (constraint as { readonly tag?: unknown }).tag);
  }
}

function validateRef(
  ref: ExtendedRef,
  scope: ReadonlyMap<string, ValueScopeEntry>,
  codecs: FluentCodec[],
): void {
  const entry = scope.get(ref.name);
  if (!entry) {
    toolBuildError(
      'unresolved-constraint-ref',
      `constraint references an unknown argument: ${ref.name}`,
    );
  }
  if (ref.tag === 'present') return;
  if (!entry.codec || !entry.mode) {
    toolBuildError(
      'value-is-type-mismatch',
      `value-is literal does not match the argument type: ${ref.name}`,
    );
  }
  const resolved = ref.value;
  if (resolved.tag === 'deferred') {
    toolBuildError(
      'unresolved-value-is-literal',
      `value-is literal for argument "${ref.name}" was not resolved during normalization`,
    );
  }
  let candidates: FluentCodec[];
  try {
    candidates = valueIsCodecs(entry.codec, entry.mode);
  } catch {
    candidates = [entry.codec];
  }
  const candidate = candidates.find((codec) => deepEqual(resolved.codec.graph, codec.graph));
  if (!candidate) {
    toolBuildError(
      'value-is-type-mismatch',
      `value-is literal does not match the argument type: ${ref.name}`,
    );
  }
  let encoded: SchemaValue;
  try {
    encoded = resolved.codec.toValue(resolved.value);
  } catch {
    toolBuildError(
      'value-is-type-mismatch',
      `value-is literal does not match the argument type: ${ref.name}`,
    );
  }
  if (
    !deepEqual(encoded, resolved.schemaValue) ||
    !sourceValueConforms(candidate, resolved.value) ||
    !schemaValueConforms(candidate.graph, candidate.graph.root, resolved.schemaValue)
  ) {
    toolBuildError(
      'value-is-type-mismatch',
      `value-is literal does not match the argument type: ${ref.name}`,
    );
  }
  codecs.push(resolved.codec);
}

function validateCodecValue(
  value: CodecValue,
  expected: FluentCodec,
  code: 'default-type-mismatch' | 'value-is-type-mismatch',
): void {
  if (!deepEqual(value.codec.graph, expected.graph)) {
    toolBuildError(
      code,
      `${code === 'default-type-mismatch' ? 'default value' : 'value-is literal'} codec does not match its declared schema`,
    );
  }
  if (!sourceValueConforms(expected, value.value)) {
    toolBuildError(
      code,
      `${code === 'default-type-mismatch' ? 'default value' : 'value-is literal'} does not satisfy its source schema`,
    );
  }
  let encoded: SchemaValue;
  try {
    encoded = value.codec.toValue(value.value);
  } catch (error) {
    toolBuildError(code, errorMessage(error));
  }
  if (!schemaValueConforms(expected.graph, expected.graph.root, encoded)) {
    toolBuildError(
      code,
      `${code === 'default-type-mismatch' ? 'default value' : 'value-is literal'} does not match its declared schema`,
    );
  }
}

function registerOption(scope: Map<string, ValueScopeEntry>, option: ExtendedOptionSpec): void {
  const entry = {
    codec: optionValueCodec(option.shape),
    mode:
      option.shape.tag === 'scalar' || option.shape.tag === 'optional-scalar'
        ? ('whole-or-one-peel' as const)
        : ('exact' as const),
  };
  scope.set(option.long, entry);
  option.aliases.forEach((alias) => scope.set(alias, entry));
}

function registerFlag(
  scope: Map<string, ValueScopeEntry>,
  flag: ExtendedGlobals['flags'][number],
): void {
  const entry = {};
  scope.set(flag.long, entry);
  flag.aliases.forEach((alias) => scope.set(alias, entry));
}

function seedGlobals(globals: ExtendedGlobals, names: Set<string>, shorts: Set<string>): void {
  globals.options.forEach((option) =>
    seedSurface(option.long, option.aliases, option.short, names, shorts),
  );
  globals.flags.forEach((flag) => seedSurface(flag.long, flag.aliases, flag.short, names, shorts));
}

function seedSurface(
  name: string,
  aliases: readonly string[],
  short: string | undefined,
  names: Set<string>,
  shorts: Set<string>,
): void {
  names.add(name);
  aliases.forEach((alias) => names.add(alias));
  if (short !== undefined) shorts.add(short);
}

function insertSurface(
  name: string,
  aliases: readonly string[],
  short: string | undefined,
  names: Set<string>,
  shorts: Set<string>,
): void {
  insertName(names, name);
  aliases.forEach((alias) => insertName(names, alias));
  if (short !== undefined) {
    checkUnicodeScalar('short form', short, 'invalid-identifier');
    if (shorts.has(short)) toolBuildError('duplicate-short', `duplicate short form: '${short}'`);
    shorts.add(short);
  }
}

function insertName(names: Set<string>, name: string): void {
  if (names.has(name)) toolBuildError('duplicate-name', `duplicate tool metadata name: ${name}`);
  names.add(name);
}

function checkSiblingNames(node: ExtendedCommandNode): void {
  const names = new Set<string>();
  node.subcommands.forEach((child) => {
    insertName(names, child.name);
    child.aliases.forEach((alias) => insertName(names, alias));
  });
}

const IDENTIFIER = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;

function checkIdentifier(kind: string, value: string): void {
  if (!IDENTIFIER.test(value)) {
    toolBuildError('invalid-identifier', `invalid ${kind}: "${value}"`);
  }
}

function validateRepetition(repetition: Repetition): void {
  switch (repetition.tag) {
    case 'repeated':
      break;
    case 'delimited':
    case 'either':
      checkUnicodeScalar('repetition delimiter', repetition.val);
      break;
    default:
      invalidMetadata('repetition', (repetition as { readonly tag?: unknown }).tag);
  }
}

function checkQuantifier(kind: string, value: Quantifier): void {
  checkOneOf(kind, value, ['all', 'any']);
}

function checkUnicodeScalar(
  kind: string,
  value: string,
  code: ToolBuildErrorCode = 'invalid-metadata-value',
): void {
  if (!isUnicodeScalar(value)) {
    toolBuildError(code, `${kind} must be one Unicode scalar value: ${JSON.stringify(value)}`);
  }
}

function checkIntegerRange(
  kind: string,
  value: number,
  min: number,
  max: number,
  code: ToolBuildErrorCode = 'invalid-metadata-value',
): void {
  if (!Number.isInteger(value) || value < min || value > max) {
    toolBuildError(code, `${kind} must be an integer in [${min}, ${max}]: ${value}`);
  }
}

function checkOneOf<T extends string>(kind: string, value: unknown, allowed: readonly T[]): void {
  if (!allowed.includes(value as T)) invalidMetadata(kind, value);
}

function invalidMetadata(kind: string, value: unknown): never {
  toolBuildError('invalid-metadata-value', `invalid ${kind}: ${String(value)}`);
}

function validateCodec(codec: FluentCodec, position: string): void {
  const error = validateSchemaGraph(codec.graph)[0];
  if (!error) return;
  toolBuildError(schemaErrorCode(error), `schema at ${position}: ${error.message}`);
}

function schemaErrorCode(
  error: SchemaValidationError,
): 'unresolved-type-ref' | 'ill-formed-schema' {
  return error.code === 'dangling-ref' ? 'unresolved-type-ref' : 'ill-formed-schema';
}

function rejectVariantInput(codec: FluentCodec, name: string): void {
  if (graphReachesVariant(codec.graph, codec.graph.root, new Set())) {
    toolBuildError(
      'variant-in-input-position',
      `a variant or union type is reachable from input position: ${name}`,
    );
  }
}

function graphReachesVariant(
  graph: SchemaGraph,
  type: SchemaType,
  visitedRefs: Set<string>,
): boolean {
  const body = type.body;
  switch (body.tag) {
    case 'ref': {
      if (visitedRefs.has(body.id)) return false;
      visitedRefs.add(body.id);
      const definition = graph.defs.get(body.id);
      return definition ? graphReachesVariant(graph, definition.body, visitedRefs) : false;
    }
    case 'variant':
      return true;
    case 'union':
      return body.branches.some((entry) => graphReachesVariant(graph, entry.body, visitedRefs));
    case 'record':
      return body.fields.some((entry) => graphReachesVariant(graph, entry.body, visitedRefs));
    case 'tuple':
      return body.elements.some((entry) => graphReachesVariant(graph, entry, visitedRefs));
    case 'list':
    case 'fixed-list':
    case 'option':
      return graphReachesVariant(graph, body.element, visitedRefs);
    case 'map':
      return (
        graphReachesVariant(graph, body.key, visitedRefs) ||
        graphReachesVariant(graph, body.value, visitedRefs)
      );
    case 'result':
      return (
        (body.ok ? graphReachesVariant(graph, body.ok, visitedRefs) : false) ||
        (body.err ? graphReachesVariant(graph, body.err, visitedRefs) : false)
      );
    case 'secret':
      return false;
    case 'future':
    case 'stream':
      return body.element ? graphReachesVariant(graph, body.element, visitedRefs) : false;
    default:
      return false;
  }
}

export function schemaValueConforms(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
): boolean {
  try {
    assertSchemaValueRepresentable(value);
  } catch {
    return false;
  }
  return schemaValueMatches(graph, type, value);
}

function schemaValueMatches(graph: SchemaGraph, type: SchemaType, value: SchemaValue): boolean {
  const body = resolveType(graph, type, new Set());
  if (!body) return false;
  const resolvedBody = body.body;
  switch (resolvedBody.tag) {
    case 'bool':
      return value.tag === 'bool' && typeof value.value === 'boolean';
    case 's8':
      return (
        value.tag === 's8' &&
        integerInRange(value.value, -128n, 127n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 's16':
      return (
        value.tag === 's16' &&
        integerInRange(value.value, -32768n, 32767n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 's32':
      return (
        value.tag === 's32' &&
        integerInRange(value.value, -(2n ** 31n), 2n ** 31n - 1n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 's64':
      return (
        value.tag === 's64' &&
        typeof value.value === 'bigint' &&
        value.value >= -(2n ** 63n) &&
        value.value <= 2n ** 63n - 1n &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      );
    case 'u8':
      return (
        value.tag === 'u8' &&
        integerInRange(value.value, 0n, 255n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 'u16':
      return (
        value.tag === 'u16' &&
        integerInRange(value.value, 0n, 65535n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 'u32':
      return (
        value.tag === 'u32' &&
        integerInRange(value.value, 0n, 2n ** 32n - 1n) &&
        numericRestrictionsMatch(resolvedBody.restrictions, BigInt(value.value))
      );
    case 'u64':
      return (
        value.tag === 'u64' &&
        typeof value.value === 'bigint' &&
        value.value >= 0n &&
        value.value <= 2n ** 64n - 1n &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      );
    case 'f32':
      return (
        value.tag === 'f32' &&
        typeof value.value === 'number' &&
        numericRestrictionsMatch(resolvedBody.restrictions, Math.fround(value.value))
      );
    case 'f64':
      return (
        value.tag === 'f64' &&
        typeof value.value === 'number' &&
        numericRestrictionsMatch(resolvedBody.restrictions, value.value)
      );
    case 'char':
      return value.tag === 'char' && isUnicodeScalar(value.value);
    case 'string':
      return value.tag === 'string' && typeof value.value === 'string';
    case 'path':
      return (
        value.tag === 'path' &&
        typeof value.value === 'string' &&
        pathMatches(resolvedBody.spec.allowedExtensions, value.value)
      );
    case 'url':
      return (
        value.tag === 'url' &&
        typeof value.value === 'string' &&
        urlMatches(resolvedBody.restrictions, value.value)
      );
    case 'datetime':
      return (
        value.tag === 'datetime' &&
        typeof value.value.seconds === 'bigint' &&
        value.value.seconds >= -(2n ** 63n) &&
        value.value.seconds <= 2n ** 63n - 1n &&
        Number.isInteger(value.value.nanoseconds) &&
        value.value.nanoseconds >= 0 &&
        value.value.nanoseconds < 1_000_000_000
      );
    case 'duration':
      return (
        value.tag === 'duration' &&
        typeof value.nanoseconds === 'bigint' &&
        value.nanoseconds >= -(2n ** 63n) &&
        value.nanoseconds <= 2n ** 63n - 1n
      );
    case 'secret':
    case 'quota-token':
      return value.tag === resolvedBody.tag;
    case 'text': {
      if (value.tag !== 'text') return false;
      if (
        typeof value.text !== 'string' ||
        (value.language !== undefined && typeof value.language !== 'string')
      ) {
        return false;
      }
      const length = [...value.text].length;
      return (
        (value.language === undefined ||
          resolvedBody.restrictions.languages === undefined ||
          resolvedBody.restrictions.languages.includes(value.language)) &&
        (resolvedBody.restrictions.minLength === undefined ||
          length >= resolvedBody.restrictions.minLength) &&
        (resolvedBody.restrictions.maxLength === undefined ||
          length <= resolvedBody.restrictions.maxLength) &&
        (resolvedBody.restrictions.regex === undefined ||
          regexMatches(resolvedBody.restrictions.regex, value.text))
      );
    }
    case 'binary':
      return (
        value.tag === 'binary' &&
        value.bytes instanceof Uint8Array &&
        (value.mimeType === undefined || typeof value.mimeType === 'string') &&
        (value.mimeType === undefined ||
          resolvedBody.restrictions.mimeTypes === undefined ||
          resolvedBody.restrictions.mimeTypes.includes(value.mimeType)) &&
        (resolvedBody.restrictions.minBytes === undefined ||
          value.bytes.byteLength >= resolvedBody.restrictions.minBytes) &&
        (resolvedBody.restrictions.maxBytes === undefined ||
          value.bytes.byteLength <= resolvedBody.restrictions.maxBytes)
      );
    case 'quantity':
      return value.tag === 'quantity' && quantityMatches(resolvedBody.spec, value.value);
    case 'record':
      return (
        value.tag === 'record' &&
        value.fields.length === resolvedBody.fields.length &&
        resolvedBody.fields.every((entry, index) =>
          schemaValueMatches(graph, entry.body, value.fields[index]),
        )
      );
    case 'variant': {
      if (value.tag !== 'variant') return false;
      const selected = resolvedBody.cases[value.caseIndex];
      return (
        !!selected &&
        (selected.payload === undefined
          ? value.payload === undefined
          : value.payload !== undefined &&
            schemaValueMatches(graph, selected.payload, value.payload))
      );
    }
    case 'enum':
      return (
        value.tag === 'enum' && value.caseIndex >= 0 && value.caseIndex < resolvedBody.cases.length
      );
    case 'flags':
      return (
        value.tag === 'flags' &&
        Array.isArray(value.flags) &&
        value.flags.length === resolvedBody.names.length &&
        value.flags.every((flag) => typeof flag === 'boolean')
      );
    case 'tuple':
      return (
        value.tag === 'tuple' &&
        value.elements.length === resolvedBody.elements.length &&
        resolvedBody.elements.every((entry, index) =>
          schemaValueMatches(graph, entry, value.elements[index]),
        )
      );
    case 'list':
      return (
        value.tag === 'list' &&
        value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry))
      );
    case 'fixed-list':
      return (
        value.tag === 'fixed-list' &&
        value.elements.length === resolvedBody.length &&
        value.elements.every((entry) => schemaValueMatches(graph, resolvedBody.element, entry))
      );
    case 'map':
      return (
        value.tag === 'map' &&
        value.entries.every(
          (entry) =>
            schemaValueMatches(graph, resolvedBody.key, entry.key) &&
            schemaValueMatches(graph, resolvedBody.value, entry.value),
        )
      );
    case 'option':
      return (
        value.tag === 'option' &&
        (value.value === undefined || schemaValueMatches(graph, resolvedBody.element, value.value))
      );
    case 'result':
      return (
        value.tag === 'result' &&
        (value.result.tag === 'ok'
          ? resolvedBody.ok === undefined
            ? value.result.value === undefined
            : value.result.value !== undefined &&
              schemaValueMatches(graph, resolvedBody.ok, value.result.value)
          : resolvedBody.err === undefined
            ? value.result.value === undefined
            : value.result.value !== undefined &&
              schemaValueMatches(graph, resolvedBody.err, value.result.value))
      );
    case 'union': {
      if (value.tag !== 'union') return false;
      const branch = resolvedBody.branches.find((entry) => entry.tag === value.unionTag);
      return (
        !!branch &&
        schemaValueMatches(graph, branch.body, value.body) &&
        discriminatorMatches(graph, branch, value.body)
      );
    }
    case 'future':
    case 'stream':
    case 'ref':
      return false;
  }
}

function discriminatorMatches(
  graph: SchemaGraph,
  branch: UnionBranch,
  value: SchemaValue,
): boolean {
  const discriminator = branch.discriminator;
  switch (discriminator.tag) {
    case 'prefix':
      return stringView(value)?.startsWith(discriminator.val) ?? false;
    case 'suffix':
      return stringView(value)?.endsWith(discriminator.val) ?? false;
    case 'contains':
      return stringView(value)?.includes(discriminator.val) ?? false;
    case 'regex': {
      const text = stringView(value);
      return text !== undefined && regexMatches(discriminator.val, text);
    }
    case 'field-equals': {
      const record = recordView(graph, branch.body, value);
      if (!record) return false;
      const index = record.names.indexOf(discriminator.val.fieldName);
      if (index < 0) return false;
      return (
        discriminator.val.literal === undefined ||
        stringView(record.values[index]) === discriminator.val.literal
      );
    }
    case 'field-absent': {
      const record = recordView(graph, branch.body, value);
      return !!record && !record.names.includes(discriminator.val);
    }
  }
}

function stringView(value: SchemaValue): string | undefined {
  switch (value.tag) {
    case 'string':
    case 'path':
    case 'url':
      return value.value;
    case 'text':
      return value.text;
    default:
      return undefined;
  }
}

function recordView(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
): { readonly names: string[]; readonly values: SchemaValue[] } | undefined {
  const resolved = resolveType(graph, type, new Set());
  return resolved?.body.tag === 'record' &&
    value.tag === 'record' &&
    value.fields.length === resolved.body.fields.length
    ? { names: resolved.body.fields.map((field) => field.name), values: value.fields }
    : undefined;
}

function integerInRange(value: number, min: bigint, max: bigint): boolean {
  return Number.isInteger(value) && BigInt(value) >= min && BigInt(value) <= max;
}

function numericRestrictionsMatch(
  restrictions: NumericRestrictions | undefined,
  value: number | bigint,
): boolean {
  if (!restrictions) return true;
  const compare = (bound: NumericBound): number | undefined => {
    if (typeof value === 'number') {
      if (bound.tag !== 'float-bits') return undefined;
      const decoded = floatFromBits(bound.val);
      if (decoded === undefined || !Number.isFinite(decoded) || Number.isNaN(value)) {
        return undefined;
      }
      return value < decoded ? -1 : value > decoded ? 1 : 0;
    }
    if (bound.tag === 'float-bits') return undefined;
    return value < bound.val ? -1 : value > bound.val ? 1 : 0;
  };
  const min = restrictions.min ? compare(restrictions.min) : 0;
  const max = restrictions.max ? compare(restrictions.max) : 0;
  return min !== undefined && max !== undefined && min >= 0 && max <= 0;
}

function isUnicodeScalar(value: string): boolean {
  const points = [...value];
  if (points.length !== 1) return false;
  const codePoint = points[0].codePointAt(0);
  return codePoint !== undefined && (codePoint < 0xd800 || codePoint > 0xdfff);
}

function regexMatches(pattern: string, value: string): boolean {
  try {
    return new RegExp(pattern).test(value);
  } catch {
    return false;
  }
}

function pathMatches(allowedExtensions: string[] | undefined, value: string): boolean {
  if (value.length === 0) return false;
  if (!allowedExtensions) return true;
  const name = value.split('/').at(-1);
  const dot = name?.lastIndexOf('.') ?? -1;
  if (dot < 0 || dot + 1 >= (name?.length ?? 0)) return true;
  return allowedExtensions.includes(name!.slice(dot + 1));
}

function urlMatches(
  restrictions: Extract<SchemaType['body'], { tag: 'url' }>['restrictions'],
  value: string,
): boolean {
  try {
    const url = new URL(value);
    return (
      (restrictions.allowedSchemes === undefined ||
        restrictions.allowedSchemes.some(
          (scheme) => scheme.toLowerCase() === url.protocol.slice(0, -1).toLowerCase(),
        )) &&
      (restrictions.allowedHosts === undefined ||
        restrictions.allowedHosts.some((host) => host.toLowerCase() === url.hostname.toLowerCase()))
    );
  } catch {
    return false;
  }
}

function quantityMatches(
  spec: Extract<SchemaType['body'], { tag: 'quantity' }>['spec'],
  value: QuantityValue,
): boolean {
  if (!isQuantityValueRepresentable(value)) return false;
  const unitAllowed =
    spec.allowedSuffixes.length === 0
      ? value.unit === spec.baseUnit
      : spec.allowedSuffixes.includes(value.unit);
  return (
    unitAllowed &&
    (spec.min === undefined || quantityLessOrEqual(spec.min, value) === true) &&
    (spec.max === undefined || quantityLessOrEqual(value, spec.max) === true)
  );
}

export type SourceValueResult =
  | { readonly tag: 'valid'; readonly value: unknown }
  | { readonly tag: 'invalid' };

/** Apply a codec's synchronous Standard Schema validation, preserving transformed output. */
export function parseSourceValue(codec: FluentCodec, value: unknown): SourceValueResult {
  if (!codec.sourceSchema) return { tag: 'valid', value };
  try {
    const result = codec.sourceSchema['~standard'].validate(value);
    return result instanceof Promise || result.issues !== undefined
      ? { tag: 'invalid' }
      : { tag: 'valid', value: result.value };
  } catch {
    return { tag: 'invalid' };
  }
}

export function sourceValueConforms(codec: FluentCodec, value: unknown): boolean {
  return parseSourceValue(codec, value).tag === 'valid';
}

function resolveType(
  graph: SchemaGraph,
  type: SchemaType,
  visited: Set<string>,
): SchemaType | undefined {
  if (type.body.tag !== 'ref') return type;
  if (visited.has(type.body.id)) return undefined;
  visited.add(type.body.id);
  const definition = graph.defs.get(type.body.id);
  return definition ? resolveType(graph, definition.body, visited) : undefined;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export { ToolBuildError };
