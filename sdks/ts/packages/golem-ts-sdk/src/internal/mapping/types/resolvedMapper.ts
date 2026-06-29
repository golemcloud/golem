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

// `ResolvedType`-native mapper: reflected TypeScript types (`Type.Type`) ->
// `ResolvedGraph`. This is the new front-end that lets the SDK think directly in
// the new schema model, replacing the legacy `Type.Type -> AnalysedType`
// handlers (which could not represent recursion).
//
// Recursion support is the headline difference from the legacy mapper. The
// reflection emits a recursive back-edge as a finite `{ kind: 'others', name,
// owner, recursive: true }` node (it stops re-expanding once a type is on the
// current path). Named composites (`record` / `variant` / `enum`) reserve their
// stable `type-id` *before* their body is walked, so a back-edge encountered
// during the walk resolves to a `ResolvedType` `ref` instead of being rejected.
// The composite bodies are collected in a `ResolvedGraph.defs` registry; the
// graph stays acyclic (cycles are expressed via `ref` ids).
//
// All other behaviour matches the legacy mapper exactly (numeric parity, tagged
// unions stay `variant`, inbuilt vs custom `result`, typed arrays, option
// none-representation, question-mark optionality, the principal / quota-token
// well-known shapes, and every rejection message), so this can be swapped in for
// `typeMapper` at the registration / runtime boundary without behaviour drift.

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../../newTypes/either';
import { TypeScope } from './scope';
import { isNumberString, trimQuotes } from './stringFormat';
import {
  TaggedTypeMetadata,
  tryTaggedUnion,
  tryUnionOfOnlyLiteral,
  UserDefinedResultType,
} from './taggedUnion';
import { generateVariantTermName } from './name';
import { typeIdForName } from './schemaType';
import {
  AbsentRepr,
  r,
  ResolvedField,
  ResolvedGraph,
  ResolvedType,
  resolvedField,
  ResolvedVariantCase,
  TypeId,
  TypedArrayKind,
} from './resolvedType';

type TsType = CoreType.Type;
type EitherR = Either.Either<ResolvedType, string>;

// ============================================================
// Mapper state (def registry + recursion tracking)
// ============================================================

interface MapperState {
  // Committed nominal definitions, keyed by stable `type-id`.
  defs: Map<TypeId, ResolvedType>;
  // Ids whose body is currently being built (so recursive back-edges and
  // mutual recursion close to a `ref` instead of recursing forever).
  inProgress: Set<TypeId>;
  // Structural signature of each committed def, used to reject two different
  // types that collide on the same owner + name.
  hashes: Map<TypeId, string>;
}

function stableHash(rt: ResolvedType): string {
  // `ResolvedType` is acyclic (recursion is expressed via `ref` ids), so a
  // structural JSON signature is safe even for recursive graphs.
  return JSON.stringify(rt);
}

/**
 * Register a nominal composite under `id` (or inline it when `id` is
 * `undefined`, i.e. anonymous or a built-in generic container name). The body is
 * built under recursion protection: while it is being built, references back to
 * the same id (direct or mutual) resolve to a `ref`. Re-encountering an already
 * committed id rebuilds the body and rejects a structural conflict; otherwise it
 * deduplicates to the existing def. Always returns a `ref(id)` for use at the
 * call site (or the inlined body for anonymous types).
 */
function registerComposite(
  id: TypeId | undefined,
  build: () => EitherR,
  state: MapperState,
): EitherR {
  if (id === undefined) return build();
  if (state.inProgress.has(id)) return Either.right(r.ref(id));

  const alreadyCommitted = state.defs.has(id);

  state.inProgress.add(id);
  const built = build();
  state.inProgress.delete(id);
  if (Either.isLeft(built)) return built;

  if (alreadyCommitted) {
    if (stableHash(built.val) !== state.hashes.get(id)) {
      return Either.left(
        `Conflicting definitions for type '${id}': the same owner and name map to two different structures`,
      );
    }
    return Either.right(r.ref(id));
  }

  state.defs.set(id, built.val);
  state.hashes.set(id, stableHash(built.val));
  return Either.right(r.ref(id));
}

// ============================================================
// Entry point
// ============================================================

/** Map a reflected TypeScript type into a self-contained `ResolvedGraph`. */
export function mapTsTypeToResolvedGraph(
  type: TsType,
  scope: TypeScope | undefined,
): Either.Either<ResolvedGraph, string> {
  const state: MapperState = { defs: new Map(), inProgress: new Set(), hashes: new Map() };
  return Either.map(mapType(type, scope, state), (root) => ({ defs: state.defs, root }));
}

// ============================================================
// Core recursive walk
// ============================================================

function mapType(type: TsType, scope: TypeScope | undefined, state: MapperState): EitherR {
  const inner = mapTypeInner(type, scope, state);

  // Question-mark optionality (`x?: T`) wraps the mapped type in an `option`,
  // unless it already resolved to one (an explicit `T | undefined | null`).
  if (scope && TypeScope.isQuestionMarkOptional(scope)) {
    return Either.map(inner, (rt) => (rt.body.tag === 'option' ? rt : r.option(rt, 'undefined')));
  }
  return inner;
}

function mapTypeInner(type: TsType, scope: TypeScope | undefined, state: MapperState): EitherR {
  const boxed = rejectBoxedTypes(type);
  if (Either.isLeft(boxed)) return boxed;

  switch (type.kind) {
    case 'boolean':
      return Either.right(r.bool());
    case 'number':
      return Either.right(r.f64());
    case 'string':
      return Either.right(r.string());
    case 'bigint':
      return Either.right(r.u64());

    case 'null':
      return unsupported('null', scope);
    case 'undefined':
      return unsupported('undefined', scope);
    case 'void':
      return unsupported('void', scope);

    case 'literal':
      return mapLiteral(type, state);
    case 'array':
      return mapArray(type, state);
    case 'tuple':
      return mapTuple(type, state);
    case 'map':
      return mapMap(type, state);
    case 'promise':
      // Reuse the same scope when unwrapping a promise.
      return mapType(type.element, scope, state);
    case 'object':
      return mapRecord(type, 'object', state);
    case 'interface':
      return mapRecord(type, 'interface', state);
    case 'union':
      return mapUnion(type, scope, state);

    case 'principal':
      return mapPrincipal(state);
    case 'secret':
      return mapSecret(type, scope, state);
    case 'quota-token':
      return mapQuotaToken();
    case 'path':
      return Either.right(r.path({ direction: 'in-out', kind: 'any' }));
    case 'url':
      return Either.right(r.url({}));
    case 'datetime':
      return Either.right(r.datetime());
    case 'duration':
      return Either.right(r.duration());
    case 'quantity':
      if (!type.spec)
        return Either.left(
          'Quantity<T> type parameter must have a literal baseUnit and a tuple of string-literal allowedSuffixes',
        );
      return Either.right(r.quantity(type.spec));

    case 'others':
      return mapOthers(type, state);

    case 'class':
      return unsupportedWithHint('class', 'Use object instead.', scope);
    case 'config':
      return unsupportedWithHint('Config', 'Use an inline object type instead.', scope);
    case 'alias':
      return Either.left(
        `Type aliases are not supported. Found alias: ${type.name ?? '<anonymous>'}`,
      );
    case 'unresolved-type':
      return Either.left(`Failed to resolve type for \`${type.text}\`: ${type.error}`);
  }
}

function rejectBoxedTypes(type: TsType): Either.Either<never, string> {
  switch (type.name) {
    case 'String':
      return Either.left('Unsupported type `String`, use `string` instead');
    case 'Boolean':
      return Either.left('Unsupported type `Boolean`, use `boolean` instead');
    case 'BigInt':
      return Either.left('Unsupported type `BigInt`, use `bigint` instead');
    case 'Number':
      return Either.left('Unsupported type `Number`, use `number` instead');
    case 'Symbol':
      return Either.left('Unsupported type `Symbol`, use `string` if possible');
    case 'Date':
      return Either.left('Unsupported type `Date`. Use a `string` if possible');
    case 'RegExp':
      return Either.left('Unsupported type `RegExp`. Use a `string` if possible');
  }
  return Either.right(undefined as never);
}

function unsupported(kind: string, scope: TypeScope | undefined): EitherR {
  const scopeName = scope?.name;
  const parameterInScope = scope ? TypeScope.paramName(scope) : undefined;
  return Either.left(
    `Unsupported type \`${kind}\`` +
      (scopeName ? ` in ${scopeName}` : '') +
      (parameterInScope ? ` for parameter \`${parameterInScope}\`` : ''),
  );
}

function unsupportedWithHint(kind: string, hint: string, scope: TypeScope | undefined): EitherR {
  const scopeName = scope?.name;
  const parameterInScope = scope ? TypeScope.paramName(scope) : undefined;
  return Either.left(
    `Unsupported type \`${kind}\`${scopeName ? ` in ${scopeName}` : ''}` +
      (parameterInScope ? ` for parameter \`${parameterInScope}\`` : '') +
      `. Hint: ${hint}`,
  );
}

// ============================================================
// Records (object / interface)
// ============================================================

function mapRecord(
  type: Extract<TsType, { kind: 'object' | 'interface' }>,
  kind: 'object' | 'interface',
  state: MapperState,
): EitherR {
  const id = typeIdForName(type.name, type.owner);
  return registerComposite(id, () => buildRecordBody(type, kind, state), state);
}

function buildRecordBody(
  type: Extract<TsType, { kind: 'object' | 'interface' }>,
  kind: 'object' | 'interface',
  state: MapperState,
): EitherR {
  const entityName = type.name ?? type.kind;

  const fieldResults: Either.Either<ResolvedField, string>[] = type.properties.map((prop) => {
    const internalType = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());
    const node = prop.getDeclarations()[0];
    const hasQuestion =
      (Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) &&
      node.hasQuestionToken();
    const fieldScope =
      kind === 'object'
        ? TypeScope.object(entityName, prop.getName(), hasQuestion)
        : TypeScope.interface(entityName, prop.getName(), hasQuestion);
    return Either.map(mapType(internalType, fieldScope, state), (rt) =>
      resolvedField(prop.getName(), rt),
    );
  });

  const all = Either.all(fieldResults);
  if (Either.isLeft(all)) return all;

  if (all.val.length === 0) {
    return Either.left(
      `Type ${type.name} is an object but has no properties. Object types must define at least one property.`,
    );
  }

  return Either.right(r.record(all.val, type.name, type.owner));
}

// ============================================================
// Arrays / tuples / maps / literals
// ============================================================

const TYPED_ARRAY_ELEMENTS: Record<string, { kind: TypedArrayKind; element: () => ResolvedType }> =
  {
    Float64Array: { kind: 'f64', element: r.f64 },
    Float32Array: { kind: 'f32', element: r.f32 },
    Int8Array: { kind: 'i8', element: r.s8 },
    Uint8Array: { kind: 'u8', element: r.u8 },
    Int16Array: { kind: 'i16', element: r.s16 },
    Uint16Array: { kind: 'u16', element: r.u16 },
    Int32Array: { kind: 'i32', element: r.s32 },
    Uint32Array: { kind: 'u32', element: r.u32 },
    BigInt64Array: { kind: 'big-i64', element: r.s64 },
    BigUint64Array: { kind: 'big-u64', element: r.u64 },
  };

function mapArray(type: Extract<TsType, { kind: 'array' }>, state: MapperState): EitherR {
  if (type.name) {
    const typed = TYPED_ARRAY_ELEMENTS[type.name];
    if (typed) return Either.right(r.list(typed.element(), typed.kind));
  }
  return Either.map(mapType(type.element, undefined, state), (inner) => r.list(inner));
}

function mapTuple(type: Extract<TsType, { kind: 'tuple' }>, state: MapperState): EitherR {
  if (!type.elements.length) {
    return Either.left('Empty tuple types are not supported');
  }
  return Either.map(Either.all(type.elements.map((el) => mapType(el, undefined, state))), (items) =>
    r.tuple(items),
  );
}

function mapMap(type: Extract<TsType, { kind: 'map' }>, state: MapperState): EitherR {
  return Either.zipWith(
    mapType(type.key, undefined, state),
    mapType(type.value, undefined, state),
    (key, value) => r.map(key, value),
  );
}

function mapLiteral(type: Extract<TsType, { kind: 'literal' }>, state: MapperState): EitherR {
  const literalName = type.literalValue;
  if (!literalName) {
    return Either.left(
      `internal error: failed to retrieve the literal value from type of kind ${type.kind}`,
    );
  }
  if (literalName === 'true' || literalName === 'false') {
    return Either.right(r.bool());
  }
  if (isNumberString(literalName)) {
    return Either.left('Literals of number type are not supported');
  }
  const id = typeIdForName(type.name, type.owner);
  return registerComposite(
    id,
    () => Either.right(r.enum([trimQuotes(literalName)], type.name, type.owner)),
    state,
  );
}

// ============================================================
// `others` (well-known rejections + recursive back-edges)
// ============================================================

function mapOthers(type: Extract<TsType, { kind: 'others' }>, state: MapperState): EitherR {
  const name = type.name;

  if (!name) {
    return Either.left('Unsupported type (anonymous) found.');
  }

  for (const rule of REJECT_RULES) {
    if (rule.test(name)) {
      return Either.left(rule.message(name));
    }
  }

  if (type.recursive) {
    const id = typeIdForName(name, type.owner);
    if (id !== undefined && (state.inProgress.has(id) || state.defs.has(id))) {
      return Either.right(r.ref(id));
    }
    return Either.left(
      `\`${name}\` is recursive, but its definition could not be resolved.\n` +
        `Recursive references are only supported through named record or variant types.`,
    );
  }

  return Either.left(`Unsupported type \`${name}\``);
}

type RejectRule = {
  test: (name: string) => boolean;
  message: (name: string) => string;
};

const REJECT_RULES: RejectRule[] = [
  {
    test: (name) => name === 'any',
    message: () => 'Unsupported type `any`. Use a specific type instead',
  },
  {
    test: (name) => name === 'Date',
    message: () => 'Unsupported type `Date`. Use a string in ISO 8601 format instead',
  },
  {
    test: (name) => name === 'next',
    message: () => 'Unsupported type `Iterator`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('asyncIterator'),
    message: () => 'Unsupported type `AsyncIterator`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('iterator'),
    message: () => 'Unsupported type `Iterable`. Use `Array` type instead',
  },
  {
    test: (name) => name.includes('asyncIterable'),
    message: () => 'Unsupported type `AsyncIterable`. Use `Array` type instead',
  },
  {
    test: (name) => name === 'Record',
    message: (name) => `Unsupported type \`${name}\`. Use a plain object or a \`Map\` type instead`,
  },
];

// ============================================================
// Unions (enum / tagged variant / result / optional / plain variant)
// ============================================================

function mapUnion(
  type: Extract<TsType, { kind: 'union' }>,
  scope: TypeScope | undefined,
  state: MapperState,
): EitherR {
  const normalizedUnionTypes = normalizeBooleanUnion(type.unionTypes);

  // Inbuilt `Result<T, E>`.
  const inbuilt = tryInbuiltResult(type, state);
  if (inbuilt) return inbuilt;

  // Union of only string literals -> enum.
  const enumResult = tryEnumUnion(type, state);
  if (enumResult) return enumResult;

  // All members tagged (`{ tag: 'a', ... } | { tag: 'b', ... }`) -> variant
  // (or user-defined `result` when the tags are exactly `ok` / `err`).
  const tagged = tryTaggedUnionAndProcess(type, state);
  if (tagged) return tagged;

  // Contains `null` / `undefined` / `void` -> option of the remainder.
  const optional = tryOptionalUnion(type, normalizedUnionTypes, scope, state);
  if (optional) return optional;

  // Otherwise a plain (anonymous-cased) variant.
  return buildPlainVariant(type, normalizedUnionTypes, state);
}

function tryInbuiltResult(
  type: Extract<TsType, { kind: 'union' }>,
  state: MapperState,
): EitherR | undefined {
  const unionTypes = type.unionTypes;
  const typeParams = type.typeParams;
  const hasOkCase = unionTypes.some((ut) => ut.name === 'Ok');
  const hasErrCase = unionTypes.some((ut) => ut.name === 'Err');
  const isInbuiltResult = type.name === 'Result' || type.originalTypeName === 'Result';

  if (!(isInbuiltResult && unionTypes.length === 2 && hasOkCase && hasErrCase)) {
    return undefined;
  }

  const okType = typeParams[0];
  const errType = typeParams[1];
  const okIsVoid = okType.kind === 'void';
  const errIsVoid = errType.kind === 'void';

  if (okIsVoid && errIsVoid) {
    return Either.right(
      r.result(
        undefined,
        undefined,
        { tag: 'inbuilt', okAbsent: 'undefined', errAbsent: 'undefined' },
        undefined,
        type.owner,
      ),
    );
  }

  if (okIsVoid) {
    return Either.map(mapType(errType, undefined, state), (err) =>
      r.result(
        undefined,
        err,
        { tag: 'inbuilt', okAbsent: 'undefined', errAbsent: undefined },
        undefined,
        type.owner,
      ),
    );
  }

  if (errIsVoid) {
    return Either.map(mapType(okType, undefined, state), (ok) =>
      r.result(
        ok,
        undefined,
        { tag: 'inbuilt', okAbsent: undefined, errAbsent: 'undefined' },
        undefined,
        type.owner,
      ),
    );
  }

  return Either.map(
    Either.zipBoth(mapType(okType, undefined, state), mapType(errType, undefined, state)),
    ([ok, err]) =>
      r.result(
        ok,
        err,
        { tag: 'inbuilt', okAbsent: undefined, errAbsent: undefined },
        undefined,
        type.owner,
      ),
  );
}

function tryEnumUnion(
  type: Extract<TsType, { kind: 'union' }>,
  state: MapperState,
): EitherR | undefined {
  const literals = tryUnionOfOnlyLiteral(type.unionTypes);
  if (Either.isLeft(literals)) return literals;

  const lits = literals.val;
  if (!lits) return undefined;

  const id = typeIdForName(type.name, type.owner);
  return registerComposite(
    id,
    () => Either.right(r.enum(lits.literals, type.name, type.owner)),
    state,
  );
}

function tryTaggedUnionAndProcess(
  type: Extract<TsType, { kind: 'union' }>,
  state: MapperState,
): EitherR | undefined {
  const tagged = tryTaggedUnion(type.unionTypes);
  if (Either.isLeft(tagged)) return tagged;
  if (!tagged.val) return undefined;

  return tagged.val.tag === 'result'
    ? convertUserDefinedResult(type.name, type.owner, tagged.val.val, state)
    : convertTaggedToVariant(type.name, type.owner, tagged.val.val, state);
}

function convertUserDefinedResult(
  typeName: string | undefined,
  typeOwner: string | undefined,
  resultType: UserDefinedResultType,
  state: MapperState,
): EitherR {
  const okValueName = resultType.okType ? resultType.okType[0] : undefined;
  const errValueName = resultType.errType ? resultType.errType[0] : undefined;

  let ok: ResolvedType | undefined;
  if (resultType.okType && resultType.okType[1].kind !== 'void') {
    const okR = mapType(resultType.okType[1], undefined, state);
    if (Either.isLeft(okR)) return okR;
    ok = okR.val;
  }

  let err: ResolvedType | undefined;
  if (resultType.errType && resultType.errType[1].kind !== 'void') {
    const errR = mapType(resultType.errType[1], undefined, state);
    if (Either.isLeft(errR)) return errR;
    err = errR.val;
  }

  return Either.right(
    r.result(ok, err, { tag: 'custom', okValueName, errValueName }, typeName, typeOwner),
  );
}

function convertTaggedToVariant(
  typeName: string | undefined,
  typeOwner: string | undefined,
  taggedTypes: TaggedTypeMetadata[],
  state: MapperState,
): EitherR {
  const id = typeIdForName(typeName, typeOwner);
  return registerComposite(
    id,
    () => {
      const cases: ResolvedVariantCase[] = [];
      for (const meta of taggedTypes) {
        if (meta.valueType && meta.valueType[1].kind === 'literal') {
          return Either.left('Tagged unions cannot have literal types in the value section');
        }
        if (!meta.valueType) {
          cases.push({ name: meta.tagLiteralName });
        } else {
          const payload = mapType(meta.valueType[1], undefined, state);
          if (Either.isLeft(payload)) return payload;
          cases.push({
            name: meta.tagLiteralName,
            payload: payload.val,
            valueKey: meta.valueType[0],
          });
        }
      }
      return Either.right(r.variant(true, cases, typeName, typeOwner));
    },
    state,
  );
}

function tryOptionalUnion(
  type: Extract<TsType, { kind: 'union' }>,
  normalizedUnionTypes: TsType[],
  scope: TypeScope | undefined,
  state: MapperState,
): EitherR | undefined {
  const emptyType = getFirstEmptyType(normalizedUnionTypes);
  if (!emptyType) return undefined;

  const noneRepr: AbsentRepr = emptyType === 'null' ? 'null' : 'undefined';

  const stripped = filterEmptyTypesFromUnion(type, normalizedUnionTypes, scope);
  if (Either.isLeft(stripped)) return stripped;

  return Either.map(mapType(stripped.val, undefined, state), (inner) =>
    r.option(inner, noneRepr, undefined, type.owner),
  );
}

function buildPlainVariant(
  type: Extract<TsType, { kind: 'union' }>,
  normalizedUnionTypes: TsType[],
  state: MapperState,
): EitherR {
  const id = typeIdForName(type.name, type.owner);
  return registerComposite(
    id,
    () => {
      let fieldIdx = 1;
      const cases: ResolvedVariantCase[] = [];

      for (const member of normalizedUnionTypes) {
        if (member.kind === 'literal') {
          if (!member.literalValue) {
            return Either.left('Unable to determine the literal value');
          }
          if (isNumberString(member.literalValue)) {
            return Either.left('Literals of number type are not supported');
          }
          cases.push({ name: trimQuotes(member.literalValue) });
          continue;
        }

        const mapped = mapType(member, undefined, state);
        if (Either.isLeft(mapped)) return mapped;

        cases.push({ name: generateVariantTermName(type.name, fieldIdx++), payload: mapped.val });
      }

      return Either.right(r.variant(false, cases, type.name, type.owner));
    },
    state,
  );
}

type EmptyKind = 'null' | 'undefined' | 'void';

function getFirstEmptyType(unionTypes: TsType[]): EmptyKind | undefined {
  for (const t of unionTypes) {
    switch (t.kind) {
      case 'null':
        return 'null';
      case 'undefined':
        return 'undefined';
      case 'void':
        return 'void';
    }
  }
  return undefined;
}

function filterEmptyTypesFromUnion(
  type: Extract<TsType, { kind: 'union' }>,
  normalizedUnionTypes: TsType[],
  scope: TypeScope | undefined,
): Either.Either<TsType, string> {
  const alternateTypes = normalizedUnionTypes.filter(
    (ut) => ut.kind !== 'undefined' && ut.kind !== 'null' && ut.kind !== 'void',
  );

  if (alternateTypes.length === 0) {
    const paramName = scope ? TypeScope.paramName(scope) : undefined;
    if (paramName) {
      return Either.left(
        `Parameter \`${paramName}\` in \`${scope?.name}\` has a union type that cannot be resolved to a valid type`,
      );
    }
    return Either.left('Union type cannot be resolved');
  }

  if (alternateTypes.length === 1) {
    return Either.right(alternateTypes[0]);
  }

  return Either.right({
    kind: 'union',
    name: type.name,
    owner: type.owner,
    unionTypes: alternateTypes,
    optional: type.optional,
    typeParams: type.typeParams,
    originalTypeName: type.originalTypeName,
  });
}

// In the TypeScript type system a `boolean` is `true | false`. Reflection can
// surface this (and `x?: boolean` as `true | false | undefined`), so collapse a
// `true` + `false` pair back into a single `boolean` member, preserving any
// optionality that was attached to one of the boolean literals.
function normalizeBooleanUnion(types: TsType[]): TsType[] {
  const hasTrue = types.some((t) => t.kind === 'literal' && t.literalValue === 'true');
  const hasFalse = types.some((t) => t.kind === 'literal' && t.literalValue === 'false');

  if (!hasTrue || !hasFalse) return types;

  const firstBooleanLiteralIdx = types.findIndex(
    (t) => t.kind === 'literal' && (t.literalValue === 'true' || t.literalValue === 'false'),
  );

  const optional = types.some(
    (t) =>
      t.kind === 'literal' &&
      (t.literalValue === 'true' || t.literalValue === 'false') &&
      t.optional,
  );

  const withoutBoolLiterals = types.filter(
    (t) => !(t.kind === 'literal' && (t.literalValue === 'true' || t.literalValue === 'false')),
  );

  const insertAt = Math.min(firstBooleanLiteralIdx, withoutBoolLiterals.length);

  return [
    ...withoutBoolLiterals.slice(0, insertAt),
    { kind: 'boolean', optional },
    ...withoutBoolLiterals.slice(insertAt),
  ];
}

// ============================================================
// Well-known shapes: principal / quota-token
// ============================================================

function buildUuid(state: MapperState): EitherR {
  return registerComposite(
    'Uuid',
    () =>
      Either.right(
        r.record([resolvedField('highBits', r.u64()), resolvedField('lowBits', r.u64())], 'Uuid'),
      ),
    state,
  );
}

function mapQuotaToken(): EitherR {
  // A quota-token is an opaque, unforgeable capability: it maps to the rich
  // `quota-token` schema type, never to a structural record. The runtime value
  // is an owned handle (see `GuestQuotaTokenHandle`) carried by ownership.
  return Either.right(r.quotaToken({}));
}

function mapSecret(
  type: Extract<TsType, { kind: 'secret' }>,
  scope: TypeScope | undefined,
  state: MapperState,
): EitherR {
  const inner = mapType(type.element, undefined, state);
  if (Either.isLeft(inner)) return inner;
  return Either.right(r.secret(inner.val));
}

function mapPrincipal(state: MapperState): EitherR {
  const uuidR = buildUuid(state);
  if (Either.isLeft(uuidR)) return uuidR;

  const accountIdR = registerComposite(
    'AccountId',
    () => Either.right(r.record([resolvedField('uuid', uuidR.val)], 'AccountId')),
    state,
  );
  if (Either.isLeft(accountIdR)) return accountIdR;

  const componentIdR = registerComposite(
    'ComponentId',
    () => Either.right(r.record([resolvedField('uuid', uuidR.val)], 'ComponentId')),
    state,
  );
  if (Either.isLeft(componentIdR)) return componentIdR;

  const agentIdR = registerComposite(
    'AgentId',
    () =>
      Either.right(
        r.record(
          [resolvedField('componentId', componentIdR.val), resolvedField('agentId', r.string())],
          'AgentId',
        ),
      ),
    state,
  );
  if (Either.isLeft(agentIdR)) return agentIdR;

  const oidcR = registerComposite(
    'OidcPrincipal',
    () =>
      Either.right(
        r.record(
          [
            resolvedField('sub', r.string()),
            resolvedField('issuer', r.string()),
            resolvedField('email', r.option(r.string(), 'undefined')),
            resolvedField('name', r.option(r.string(), 'undefined')),
            resolvedField('emailVerified', r.option(r.bool(), 'undefined')),
            resolvedField('givenName', r.option(r.string(), 'undefined')),
            resolvedField('familyName', r.option(r.string(), 'undefined')),
            resolvedField('picture', r.option(r.string(), 'undefined')),
            resolvedField('preferredUsername', r.option(r.string(), 'undefined')),
            resolvedField('claims', r.string()),
          ],
          'OidcPrincipal',
        ),
      ),
    state,
  );
  if (Either.isLeft(oidcR)) return oidcR;

  const agentPrincipalR = registerComposite(
    'AgentPrincipal',
    () => Either.right(r.record([resolvedField('agentId', agentIdR.val)], 'AgentPrincipal')),
    state,
  );
  if (Either.isLeft(agentPrincipalR)) return agentPrincipalR;

  const golemUserR = registerComposite(
    'GolemUserPrincipal',
    () =>
      Either.right(r.record([resolvedField('accountId', accountIdR.val)], 'GolemUserPrincipal')),
    state,
  );
  if (Either.isLeft(golemUserR)) return golemUserR;

  return registerComposite(
    'Principal',
    () =>
      Either.right(
        r.variant(
          true,
          [
            { name: 'oidc', payload: oidcR.val, valueKey: 'val' },
            { name: 'agent', payload: agentPrincipalR.val, valueKey: 'val' },
            { name: 'golem-user', payload: golemUserR.val, valueKey: 'val' },
            { name: 'anonymous' },
          ],
          'Principal',
        ),
      ),
    state,
  );
}
