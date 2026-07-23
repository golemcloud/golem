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

// Structural well-formedness validation for the recursive schema model. This
// mirrors golem-schema's validation/well_formedness.rs. Checks that Rust gets
// from its field types (u32 lengths, s64 quantity mantissas, and s32 scales)
// are explicit here because the TypeScript model uses number and bigint.

import type {
  DiscriminatorRule,
  NumericBound,
  NumericRestrictions,
  QuantityValue,
  SchemaGraph,
  SchemaType,
  TypeId,
  UnionBranch,
} from './model';

export type SchemaValidationErrorCode =
  | 'dangling-ref'
  | 'recursive-alias'
  | 'inline-cycle'
  | 'empty-variant'
  | 'empty-enum'
  | 'empty-union'
  | 'empty-flags'
  | 'duplicate-field-name'
  | 'duplicate-variant-case'
  | 'duplicate-enum-case'
  | 'duplicate-flag-name'
  | 'duplicate-union-tag'
  | 'map-key-not-primitive'
  | 'invalid-fixed-list-length'
  | 'quantity-min-greater-than-max'
  | 'quantity-min-unit-mismatch'
  | 'quantity-max-unit-mismatch'
  | 'quantity-comparison-overflow'
  | 'invalid-quantity-bound'
  | 'union-string-rule-on-non-string-body'
  | 'union-field-rule-on-non-record-body'
  | 'union-field-equals-literal-on-non-string-field'
  | 'union-field-rule-missing-field'
  | 'union-ambiguous-discriminators'
  | 'union-unsatisfiable-field-absent'
  | 'invalid-union-regex'
  | 'invalid-text-regex'
  | 'invalid-text-length-range'
  | 'invalid-binary-byte-range'
  | 'invalid-numeric-restriction'
  | 'nullable-nesting';

export interface SchemaValidationError {
  readonly code: SchemaValidationErrorCode;
  readonly message: string;
  readonly refId?: TypeId;
}

type Errors = SchemaValidationError[];
type AliasResolution =
  | { readonly tag: 'concrete'; readonly type: SchemaType }
  | { readonly tag: 'dangling'; readonly id: TypeId }
  | { readonly tag: 'recursive'; readonly id: TypeId };
type BodyShape =
  | { readonly tag: 'string' }
  | { readonly tag: 'record'; readonly fields: SchemaType['body'] & { tag: 'record' } }
  | { readonly tag: 'other' }
  | { readonly tag: 'unresolved' };

const U32_MAX = 2 ** 32 - 1;
const S32_MIN = -(2 ** 31);
const S32_MAX = 2 ** 31 - 1;
const S64_MIN = -(2n ** 63n);
const S64_MAX = 2n ** 63n - 1n;
const U64_MAX = 2n ** 64n - 1n;
const I128_MIN = -(2n ** 127n);
const I128_MAX = 2n ** 127n - 1n;
const FLOAT_BITS = new DataView(new ArrayBuffer(8));

/**
 * Validate every definition body followed by the graph root, in deterministic order.
 *
 * Regex restrictions are compiled with the platform's ECMAScript `RegExp` implementation.
 * Rust validates the same schema fields with the `regex` crate, so engine-specific syntax can
 * be accepted by one SDK and rejected by the other. Cross-SDK schemas should use their common
 * regular-expression subset.
 */
export function validateSchemaGraph(graph: SchemaGraph): SchemaValidationError[] {
  const errors: Errors = [];
  for (const definition of graph.defs.values()) {
    checkType(graph, definition.body, errors, new Set());
  }
  checkType(graph, graph.root, errors, new Set());
  return errors;
}

/** Validate one root against an existing graph's definitions. */
export function validateSchemaRoot(graph: SchemaGraph, root: SchemaType): SchemaValidationError[] {
  const errors: Errors = [];
  checkType(graph, root, errors, new Set());
  return errors;
}

function issue(
  errors: Errors,
  code: SchemaValidationErrorCode,
  message: string,
  refId?: TypeId,
): void {
  errors.push({ code, message, ...(refId === undefined ? {} : { refId }) });
}

function checkType(
  graph: SchemaGraph,
  type: SchemaType,
  errors: Errors,
  onPath: Set<SchemaType>,
): void {
  if (onPath.has(type)) {
    issue(errors, 'inline-cycle', 'schema contains an inline object-reference cycle');
    return;
  }
  onPath.add(type);
  try {
    const body = type.body;
    switch (body.tag) {
      case 'ref': {
        const resolved = resolveAlias(graph, type);
        if (resolved.tag === 'dangling') {
          issue(errors, 'dangling-ref', `dangling type reference \`${resolved.id}\``, resolved.id);
        } else if (resolved.tag === 'recursive') {
          issue(
            errors,
            'recursive-alias',
            `type reference \`${resolved.id}\` forms a reference cycle with no concrete type`,
            resolved.id,
          );
        }
        break;
      }
      case 'record':
        checkDuplicates(
          body.fields.map((field) => field.name),
          (name) => issue(errors, 'duplicate-field-name', `duplicate field \`${name}\``),
        );
        body.fields.forEach((field) => checkType(graph, field.body, errors, onPath));
        break;
      case 'variant':
        if (body.cases.length === 0) issue(errors, 'empty-variant', 'variant has no cases');
        checkDuplicates(
          body.cases.map((entry) => entry.name),
          (name) => issue(errors, 'duplicate-variant-case', `duplicate variant case \`${name}\``),
        );
        body.cases.forEach((entry) => {
          if (entry.payload) checkType(graph, entry.payload, errors, onPath);
        });
        break;
      case 'enum':
        if (body.cases.length === 0) issue(errors, 'empty-enum', 'enum has no cases');
        checkDuplicates(body.cases, (name) =>
          issue(errors, 'duplicate-enum-case', `duplicate enum case \`${name}\``),
        );
        break;
      case 'flags':
        if (body.names.length === 0) issue(errors, 'empty-flags', 'flags has no entries');
        checkDuplicates(body.names, (name) =>
          issue(errors, 'duplicate-flag-name', `duplicate flag \`${name}\``),
        );
        break;
      case 'tuple':
        body.elements.forEach((element) => checkType(graph, element, errors, onPath));
        break;
      case 'list':
        checkType(graph, body.element, errors, onPath);
        break;
      case 'fixed-list':
        if (!isU32(body.length) || body.length === 0) {
          issue(errors, 'invalid-fixed-list-length', 'fixed-list length must be a positive u32');
        }
        checkType(graph, body.element, errors, onPath);
        break;
      case 'map':
        if (classifyMapKey(graph, body.key) === 'non-primitive') {
          issue(errors, 'map-key-not-primitive', 'map key must be a primitive type');
        }
        checkType(graph, body.key, errors, onPath);
        checkType(graph, body.value, errors, onPath);
        break;
      case 'option':
        if (isNullable(graph, body.element, new Set())) {
          issue(
            errors,
            'nullable-nesting',
            `option<${describeNullable(body.element)}> is invalid because the inner type is also nullable`,
          );
        }
        checkType(graph, body.element, errors, onPath);
        break;
      case 'result':
        if (body.ok) checkType(graph, body.ok, errors, onPath);
        if (body.err) checkType(graph, body.err, errors, onPath);
        break;
      case 'text':
        checkTextRestrictions(body.restrictions, errors);
        break;
      case 'binary':
        checkBinaryRestrictions(body.restrictions, errors);
        break;
      case 'quantity':
        checkQuantity(body.spec, errors);
        break;
      case 'union':
        checkUnion(graph, body.branches, errors, onPath);
        break;
      case 'secret':
        checkType(graph, body.inner, errors, onPath);
        break;
      case 'future':
      case 'stream':
        if (body.element) checkType(graph, body.element, errors, onPath);
        break;
      case 's8':
      case 's16':
      case 's32':
      case 's64':
      case 'u8':
      case 'u16':
      case 'u32':
      case 'u64':
      case 'f32':
      case 'f64':
        checkNumericRestrictions(body.tag, body.restrictions, errors);
        break;
      case 'bool':
      case 'char':
      case 'string':
      case 'path':
      case 'url':
      case 'datetime':
      case 'duration':
      case 'quota-token':
        break;
    }
  } finally {
    onPath.delete(type);
  }
}

function checkDuplicates(values: readonly string[], onDuplicate: (value: string) => void): void {
  const seen = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) onDuplicate(value);
    else seen.add(value);
  }
}

function resolveAlias(graph: SchemaGraph, type: SchemaType): AliasResolution {
  let current = type;
  const visited = new Set<TypeId>();
  while (current.body.tag === 'ref') {
    const id = current.body.id;
    if (visited.has(id)) return { tag: 'recursive', id };
    visited.add(id);
    const definition = graph.defs.get(id);
    if (!definition) return { tag: 'dangling', id };
    current = definition.body;
  }
  return { tag: 'concrete', type: current };
}

function classifyMapKey(
  graph: SchemaGraph,
  type: SchemaType,
): 'primitive' | 'non-primitive' | 'unresolved' {
  const resolved = resolveAlias(graph, type);
  if (resolved.tag !== 'concrete') return 'unresolved';
  return isPrimitive(resolved.type) ? 'primitive' : 'non-primitive';
}

function isPrimitive(type: SchemaType): boolean {
  return [
    'bool',
    's8',
    's16',
    's32',
    's64',
    'u8',
    'u16',
    'u32',
    'u64',
    'f32',
    'f64',
    'char',
    'string',
  ].includes(type.body.tag);
}

function isNullable(graph: SchemaGraph, type: SchemaType, visited: Set<TypeId>): boolean {
  switch (type.body.tag) {
    case 'option':
      return true;
    case 'union':
      return type.body.branches.some((branch) => isNullable(graph, branch.body, visited));
    case 'ref': {
      const id = type.body.id;
      if (visited.has(id)) return false;
      visited.add(id);
      const definition = graph.defs.get(id);
      const result = definition ? isNullable(graph, definition.body, visited) : false;
      visited.delete(id);
      return result;
    }
    default:
      return false;
  }
}

function describeNullable(type: SchemaType): string {
  switch (type.body.tag) {
    case 'option':
      return 'option<_>';
    case 'union':
      return 'union';
    case 'ref':
      return `ref \`${type.body.id}\``;
    default:
      return 'nullable';
  }
}

type NumericTag = 's8' | 's16' | 's32' | 's64' | 'u8' | 'u16' | 'u32' | 'u64' | 'f32' | 'f64';

function checkNumericRestrictions(
  tag: NumericTag,
  restrictions: NumericRestrictions | undefined,
  errors: Errors,
): void {
  if (!restrictions) return;
  const { min, max, unit } = restrictions;
  const reason =
    min === undefined && max === undefined && !unit
      ? 'numeric restriction set is empty'
      : (checkNumericBound(tag, min) ??
        checkNumericBound(tag, max) ??
        (min && max && compareNumericBounds(min, max) === 1
          ? 'numeric min bound is greater than max bound'
          : undefined));
  if (reason) issue(errors, 'invalid-numeric-restriction', reason);
}

function checkNumericBound(tag: NumericTag, bound: NumericBound | undefined): string | undefined {
  if (!bound) return undefined;
  const family = tag.startsWith('s') ? 'signed' : tag.startsWith('u') ? 'unsigned' : 'float-bits';
  if (bound.tag !== family) return 'numeric bound family does not match the numeric type';
  if (typeof bound.val !== 'bigint') return 'numeric bound is not representable on the WIT wire';
  if (bound.tag === 'float-bits') {
    const value = floatFromBits(bound.val);
    if (value === undefined) return 'numeric float bits do not fit u64';
    if (!Number.isFinite(value)) return 'numeric float bound must be finite';
    if (tag === 'f32' && Math.fround(value) !== value) {
      return 'f32 numeric bound does not round-trip through f32';
    }
    return undefined;
  }
  const range = numericRange(tag);
  return !range || bound.val < range[0] || bound.val > range[1]
    ? 'numeric bound does not fit the numeric type range'
    : undefined;
}

function numericRange(tag: NumericTag): readonly [bigint, bigint] | undefined {
  switch (tag) {
    case 's8':
      return [-128n, 127n];
    case 's16':
      return [-32768n, 32767n];
    case 's32':
      return [-(2n ** 31n), 2n ** 31n - 1n];
    case 's64':
      return [S64_MIN, S64_MAX];
    case 'u8':
      return [0n, 255n];
    case 'u16':
      return [0n, 65535n];
    case 'u32':
      return [0n, 2n ** 32n - 1n];
    case 'u64':
      return [0n, U64_MAX];
    case 'f32':
    case 'f64':
      return undefined;
  }
}

function compareNumericBounds(left: NumericBound, right: NumericBound): -1 | 0 | 1 | undefined {
  if (left.tag !== right.tag) return undefined;
  if (left.tag !== 'float-bits') {
    return left.val < right.val ? -1 : left.val > right.val ? 1 : 0;
  }
  const a = floatFromBits(left.val);
  const b = floatFromBits(right.val);
  if (a === undefined || b === undefined || !Number.isFinite(a) || !Number.isFinite(b)) {
    return undefined;
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

export function floatFromBits(bits: bigint): number | undefined {
  if (typeof bits !== 'bigint' || bits < 0n || bits > U64_MAX) return undefined;
  FLOAT_BITS.setBigUint64(0, bits);
  return FLOAT_BITS.getFloat64(0);
}

function checkTextRestrictions(
  restrictions: Extract<SchemaType['body'], { tag: 'text' }>['restrictions'],
  errors: Errors,
): void {
  if (!validU32Range(restrictions.minLength, restrictions.maxLength)) {
    issue(errors, 'invalid-text-length-range', 'text length bounds must be ordered u32 values');
  }
  if (restrictions.regex !== undefined) {
    try {
      new RegExp(restrictions.regex);
    } catch (error) {
      issue(
        errors,
        'invalid-text-regex',
        `text regex \`${restrictions.regex}\` failed to compile: ${errorMessage(error)}`,
      );
    }
  }
}

function checkBinaryRestrictions(
  restrictions: Extract<SchemaType['body'], { tag: 'binary' }>['restrictions'],
  errors: Errors,
): void {
  if (!validU32Range(restrictions.minBytes, restrictions.maxBytes)) {
    issue(errors, 'invalid-binary-byte-range', 'binary byte bounds must be ordered u32 values');
  }
}

function validU32Range(min: number | undefined, max: number | undefined): boolean {
  return (
    (min === undefined || isU32(min)) &&
    (max === undefined || isU32(max)) &&
    (min === undefined || max === undefined || min <= max)
  );
}

function isU32(value: number): boolean {
  return Number.isInteger(value) && value >= 0 && value <= U32_MAX;
}

export function isQuantityValueRepresentable(value: unknown): value is QuantityValue {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as QuantityValue).mantissa === 'bigint' &&
    (value as QuantityValue).mantissa >= S64_MIN &&
    (value as QuantityValue).mantissa <= S64_MAX &&
    Number.isInteger((value as QuantityValue).scale) &&
    (value as QuantityValue).scale >= S32_MIN &&
    (value as QuantityValue).scale <= S32_MAX &&
    typeof (value as QuantityValue).unit === 'string'
  );
}

export function quantityLessOrEqual(
  left: QuantityValue,
  right: QuantityValue,
): boolean | undefined {
  if (!isQuantityValueRepresentable(left) || !isQuantityValueRepresentable(right)) return undefined;
  const commonScale = Math.max(left.scale, right.scale);
  const leftShift = commonScale - left.scale;
  const rightShift = commonScale - right.scale;
  if (leftShift > 38 || rightShift > 38) return undefined;
  const leftValue = left.mantissa * 10n ** BigInt(leftShift);
  const rightValue = right.mantissa * 10n ** BigInt(rightShift);
  if (
    leftValue < I128_MIN ||
    leftValue > I128_MAX ||
    rightValue < I128_MIN ||
    rightValue > I128_MAX
  ) {
    return undefined;
  }
  return leftValue <= rightValue;
}

function checkQuantity(
  spec: Extract<SchemaType['body'], { tag: 'quantity' }>['spec'],
  errors: Errors,
): void {
  const minValid = spec.min === undefined || isQuantityValueRepresentable(spec.min);
  const maxValid = spec.max === undefined || isQuantityValueRepresentable(spec.max);
  if (!minValid || !maxValid) {
    issue(
      errors,
      'invalid-quantity-bound',
      'quantity bounds require an s64 mantissa and s32 scale',
    );
  }
  if (spec.min && minValid && spec.min.unit !== spec.baseUnit) {
    issue(
      errors,
      'quantity-min-unit-mismatch',
      `quantity min unit mismatch: base \`${spec.baseUnit}\`, min \`${spec.min.unit}\``,
    );
  }
  if (spec.max && maxValid && spec.max.unit !== spec.baseUnit) {
    issue(
      errors,
      'quantity-max-unit-mismatch',
      `quantity max unit mismatch: base \`${spec.baseUnit}\`, max \`${spec.max.unit}\``,
    );
  }
  if (
    spec.min &&
    spec.max &&
    minValid &&
    maxValid &&
    spec.min.unit === spec.baseUnit &&
    spec.max.unit === spec.baseUnit
  ) {
    const comparison = quantityLessOrEqual(spec.min, spec.max);
    if (comparison === false) {
      issue(errors, 'quantity-min-greater-than-max', 'quantity min is greater than max');
    } else if (comparison === undefined) {
      issue(
        errors,
        'quantity-comparison-overflow',
        `quantity range comparison overflowed in base unit \`${spec.baseUnit}\``,
      );
    }
  }
}

function checkUnion(
  graph: SchemaGraph,
  branches: readonly UnionBranch[],
  errors: Errors,
  onPath: Set<SchemaType>,
): void {
  if (branches.length === 0) issue(errors, 'empty-union', 'union has no branches');
  checkDuplicates(
    branches.map((branch) => branch.tag),
    (tag) => issue(errors, 'duplicate-union-tag', `duplicate union branch tag \`${tag}\``),
  );
  for (const branch of branches) {
    checkUnionBranch(graph, branch, errors);
    checkType(graph, branch.body, errors, onPath);
  }
  for (let left = 0; left < branches.length; left += 1) {
    for (let right = left + 1; right < branches.length; right += 1) {
      const reason = discriminatorsOverlap(
        branches[left].discriminator,
        branches[right].discriminator,
      );
      if (reason) {
        issue(
          errors,
          'union-ambiguous-discriminators',
          `union branches \`${branches[left].tag}\` and \`${branches[right].tag}\` have overlapping discriminators (${reason})`,
        );
      }
    }
  }
}

function checkUnionBranch(graph: SchemaGraph, branch: UnionBranch, errors: Errors): void {
  const shape = resolvedShape(graph, branch.body);
  const shapeKnown = shape.tag !== 'unresolved';
  const rule = branch.discriminator;
  switch (rule.tag) {
    case 'prefix':
    case 'suffix':
    case 'contains':
      if (shapeKnown && shape.tag !== 'string') {
        issue(
          errors,
          'union-string-rule-on-non-string-body',
          `union branch \`${branch.tag}\` uses a string-pattern rule but body is not string-shaped`,
        );
      }
      break;
    case 'regex':
      if (shapeKnown && shape.tag !== 'string') {
        issue(
          errors,
          'union-string-rule-on-non-string-body',
          `union branch \`${branch.tag}\` uses a string-pattern rule but body is not string-shaped`,
        );
      }
      if (rule.val.length === 0) {
        issue(
          errors,
          'invalid-union-regex',
          `union branch \`${branch.tag}\` regex must be non-empty`,
        );
      } else {
        try {
          new RegExp(rule.val);
        } catch (error) {
          issue(
            errors,
            'invalid-union-regex',
            `union branch \`${branch.tag}\` regex \`${rule.val}\` failed to compile: ${errorMessage(error)}`,
          );
        }
      }
      break;
    case 'field-equals':
      if (shape.tag === 'record') {
        const field = shape.fields.fields.find((entry) => entry.name === rule.val.fieldName);
        if (!field) {
          issue(
            errors,
            'union-field-rule-missing-field',
            `union branch \`${branch.tag}\` references missing field \`${rule.val.fieldName}\``,
          );
        } else if (rule.val.literal !== undefined) {
          const fieldShape = resolvedShape(graph, field.body);
          if (fieldShape.tag !== 'string' && fieldShape.tag !== 'unresolved') {
            issue(
              errors,
              'union-field-equals-literal-on-non-string-field',
              `union branch \`${branch.tag}\` compares a literal against non-string field \`${rule.val.fieldName}\``,
            );
          }
        }
      } else if (shapeKnown) {
        issue(
          errors,
          'union-field-rule-on-non-record-body',
          `union branch \`${branch.tag}\` uses a field rule but body is not record-shaped`,
        );
      }
      break;
    case 'field-absent':
      if (shape.tag === 'record') {
        if (shape.fields.fields.some((entry) => entry.name === rule.val)) {
          issue(
            errors,
            'union-unsatisfiable-field-absent',
            `union branch \`${branch.tag}\` requires declared field \`${rule.val}\` to be absent`,
          );
        }
      } else if (shapeKnown) {
        issue(
          errors,
          'union-field-rule-on-non-record-body',
          `union branch \`${branch.tag}\` uses a field rule but body is not record-shaped`,
        );
      }
      break;
  }
}

function resolvedShape(graph: SchemaGraph, type: SchemaType): BodyShape {
  const resolved = resolveAlias(graph, type);
  if (resolved.tag !== 'concrete') return { tag: 'unresolved' };
  switch (resolved.type.body.tag) {
    case 'string':
    case 'text':
    case 'url':
    case 'path':
      return { tag: 'string' };
    case 'record':
      return { tag: 'record', fields: resolved.type.body };
    default:
      return { tag: 'other' };
  }
}

function discriminatorsOverlap(
  left: DiscriminatorRule,
  right: DiscriminatorRule,
): string | undefined {
  if (left.tag === 'prefix' && right.tag === 'prefix') {
    if (!left.val && !right.val) return 'both prefixes are empty';
    if (!left.val) return `empty prefix overlaps any other prefix \`${right.val}\``;
    if (!right.val) return `empty prefix overlaps any other prefix \`${left.val}\``;
    return left.val.startsWith(right.val) || right.val.startsWith(left.val)
      ? `prefix \`${left.val}\` and prefix \`${right.val}\` overlap`
      : undefined;
  }
  if (left.tag === 'suffix' && right.tag === 'suffix') {
    if (!left.val && !right.val) return 'both suffixes are empty';
    if (!left.val) return `empty suffix overlaps any other suffix \`${right.val}\``;
    if (!right.val) return `empty suffix overlaps any other suffix \`${left.val}\``;
    return left.val.endsWith(right.val) || right.val.endsWith(left.val)
      ? `suffix \`${left.val}\` and suffix \`${right.val}\` overlap`
      : undefined;
  }
  if (left.tag === 'contains' && right.tag === 'contains') {
    return !left.val || !right.val ? 'empty contains substring matches every string' : undefined;
  }
  if ((left.tag === 'prefix' && !left.val) || (right.tag === 'prefix' && !right.val)) {
    return 'empty prefix matches every string';
  }
  if ((left.tag === 'suffix' && !left.val) || (right.tag === 'suffix' && !right.val)) {
    return 'empty suffix matches every string';
  }
  if ((left.tag === 'contains' && !left.val) || (right.tag === 'contains' && !right.val)) {
    return 'empty contains substring matches every string';
  }
  if (left.tag === 'regex' && right.tag === 'regex') {
    return left.val === right.val ? `both branches share regex \`${left.val}\`` : undefined;
  }
  if (left.tag === 'field-equals' && right.tag === 'field-equals') {
    if (left.val.fieldName !== right.val.fieldName) return undefined;
    if (left.val.literal === undefined || right.val.literal === undefined) {
      return `field-equals on \`${left.val.fieldName}\` without literal overlaps another field-equals on the same field`;
    }
    return left.val.literal === right.val.literal
      ? `two field-equals on \`${left.val.fieldName}\` share literal \`${left.val.literal}\``
      : undefined;
  }
  if (left.tag === 'field-absent' && right.tag === 'field-absent') {
    return left.val === right.val
      ? `two field-absent rules share field \`${left.val}\``
      : undefined;
  }
  return undefined;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
