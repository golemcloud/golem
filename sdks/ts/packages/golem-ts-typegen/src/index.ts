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

import {
  MethodDeclaration,
  Node as TsMorphNode,
  Scope,
  SourceFile,
  SyntaxKind,
  ts,
  Type as TsMorphType,
  ClassDeclaration,
  PropertyDeclaration,
  Symbol as TsMorphSymbol,
  TypeLiteralNode,
  UnionTypeNode,
  Project,
} from 'ts-morph';
import {
  buildJSONFromType,
  LiteTypeJSON,
  Node,
  Symbol,
  Type,
  TypeMetadata,
} from '@golemcloud/golem-ts-types-core';
import { createWellKnownTypes, WellKnown, WellKnownTypes } from './wellknownTypes.js';
import * as fs from 'node:fs';
import path from 'path';

// Tracks the types currently being expanded along a single traversal path, so a
// self-reference can be emitted as a finite recursive back-edge instead of being
// expanded forever. The value is the `owner` the type was first registered with
// on this path: a recursive back-edge must inherit it verbatim so the downstream
// mapper resolves the back-edge to the *same* type id as the root definition.
// (The owner is reference-site dependent — e.g. an imported type carries the
// import path at the root reference but resolves to `undefined` when re-derived
// from a same-module self-reference deep inside its own body — so recomputing it
// at the back-edge would break cross-module recursive types.)
type VisitedTypes = Map<TsMorphType, string | undefined>;

export function getTypeFromTsMorph(
  tsMorphType: TsMorphType,
  isOptional: boolean,
  wellKnownTypes: WellKnownTypes,
  sourceTypeNode?: TsMorphNode,
): Type.Type {
  try {
    return getTypeFromTsMorphInternal(
      tsMorphType,
      isOptional,
      wellKnownTypes,
      new Map(),
      sourceTypeNode,
    );
  } catch (e) {
    if (e instanceof Error) {
      let error = e.message;
      if (e.stack) {
        error = error + '\n\n' + e.stack;
      }

      return {
        kind: 'unresolved-type',
        name: undefined,
        owner: undefined,
        optional: isOptional,
        text: tsMorphType.getText(),
        error: error,
      };
    } else {
      throw e;
    }
  }
}

function getTypeFromTsMorphInternal(
  tsMorphType: TsMorphType,
  isOptional: boolean,
  wellKnownTypes: WellKnownTypes,
  visitedTypes: VisitedTypes,
  sourceTypeNode?: TsMorphNode,
): Type.Type {
  const type = unwrapAlias(tsMorphType);
  const rawName = getRawTypeName(type);
  const aliasName = getAliasTypeName(tsMorphType) ?? getAliasTypeName(type);
  const owner = getTypeOwner(tsMorphType, sourceTypeNode) ?? getTypeOwner(type);

  if (visitedTypes.has(type)) {
    return {
      kind: 'others',
      name: rawName ?? aliasName ?? type.getText(),
      // Inherit the owner the root definition was registered with on this path,
      // not the (reference-site dependent) owner re-derived here, so the mapper
      // resolves this back-edge to the same type id as the root. See VisitedTypes.
      owner: visitedTypes.get(type),
      optional: isOptional,
      recursive: true,
    };
  }
  visitedTypes.set(type, owner);

  if (isExactly(type, wellKnownTypes.object)) {
    const name = rawName ?? aliasName ?? type.getText();
    return {
      kind: 'others',
      name,
      optional: isOptional,
      recursive: false,
    };
  }

  for (const [name, wk] of wellKnownTypes.containers.typedArrays) {
    if (isExactly(type, wk)) {
      return {
        kind: 'array',
        name,
        element: {
          kind: 'number',
          optional: false,
        },
        optional: isOptional,
      };
    }
  }

  if (isExactly(type, wellKnownTypes.containers.promise)) {
    const inner = type.getTypeArguments()[0];
    const promiseType = getTypeFromTsMorphInternal(inner, false, wellKnownTypes, visitedTypes);

    return {
      kind: 'promise',
      name: aliasName,
      element: promiseType,
      optional: isOptional,
    };
  }

  if (isExactly(type, wellKnownTypes.containers.map)) {
    const [keyT, valT] = type.getTypeArguments();
    const key = getTypeFromTsMorphInternal(keyT, false, wellKnownTypes, new Map(visitedTypes));
    const value = getTypeFromTsMorphInternal(valT, false, wellKnownTypes, new Map(visitedTypes));
    return {
      kind: 'map',
      name: aliasName,
      key: key,
      value: value,
      optional: isOptional,
    };
  }

  if (isExactly(type, wellKnownTypes.sdk.config)) {
    const rawInner = type.getTypeArguments()[0];
    const innerType = unwrapAlias(rawInner);

    const typeLiteral = resolveStrictTypeLiteralNode(innerType);
    if (typeLiteral == null)
      throw `Config<T> type parameter must be an inline object type (e.g. Config<{ key: string }>), got: ${innerType.getText()}`;

    const result = extractConfigPropertiesFromTypeLiteral(typeLiteral, [], false, wellKnownTypes);
    if (result == null)
      throw 'Config<T> must be an object type with only property signatures. Method signatures and index signatures are not supported.';

    return {
      kind: 'config',
      name: aliasName,
      owner: getTypeOwner(type),
      optional: isOptional,
      properties: result.properties,
      // Filter out the root entry (empty path) — it has no parent to prune.
      requiredMembers: result.requiredMembers.filter((e) => e.path.length > 0),
    };
  }

  if (isExactly(type, wellKnownTypes.sdk.quotaToken)) {
    return { kind: 'quota-token', name: aliasName, optional: isOptional };
  }

  if (isExactly(type, wellKnownTypes.sdk.path)) {
    return { kind: 'path', name: aliasName, optional: isOptional };
  }

  if (isExactlyBuiltIn(type, wellKnownTypes.sdk.url)) {
    return { kind: 'url', name: aliasName, optional: isOptional };
  }

  if (isExactlyBuiltIn(type, wellKnownTypes.sdk.date)) {
    return { kind: 'datetime', name: aliasName, optional: isOptional };
  }

  if (isExactly(type, wellKnownTypes.sdk.duration)) {
    return { kind: 'duration', name: aliasName, optional: isOptional };
  }

  if (isExactly(type, wellKnownTypes.sdk.quantity)) {
    return { kind: 'quantity', name: aliasName, optional: isOptional, spec: getQuantitySpec(type) };
  }

  if (!containsInvalidTypes(type) && type.isAssignableTo(wellKnownTypes.sdk.principal)) {
    return {
      kind: 'principal',
      name: aliasName,
      optional: isOptional,
    };
  }

  if (type.isVoid()) {
    return { kind: 'void', name: 'void', owner, optional: isOptional };
  }

  if (type.isBoolean()) {
    return { kind: 'boolean', owner, optional: isOptional };
  }

  if (type.isLiteral()) {
    const literalValue = type.getLiteralValue() ?? type.getText();

    return {
      kind: 'literal',
      name: aliasName,
      owner,
      literalValue: literalValue.toString(),
      optional: isOptional,
    };
  }

  if (type.isTuple()) {
    const tupleElems = type
      .getTupleElements()
      .map((el) => getTypeFromTsMorphInternal(el, false, wellKnownTypes, new Map(visitedTypes)));

    return {
      kind: 'tuple',
      name: aliasName,
      owner,
      elements: tupleElems,
      optional: isOptional,
    };
  }

  if (type.isArray()) {
    const elementType = type.getArrayElementType();

    let resolvedElementType;

    if (elementType?.isTypeParameter()) {
      resolvedElementType = tsMorphType.getAliasTypeArguments()[0];
    } else {
      resolvedElementType = elementType;
    }

    if (!resolvedElementType) {
      throw new Error('Array type without element type');
    }

    const element = getTypeFromTsMorphInternal(
      resolvedElementType,
      false,
      wellKnownTypes,
      visitedTypes,
    );

    return {
      kind: 'array',
      name: aliasName,
      owner,
      element,
      optional: isOptional,
    };
  }

  if (type.isUnion()) {
    const argsInternal = tsMorphType.getAliasTypeArguments();

    const aliased = getAliasTypeArgumentsSafe(tsMorphType);

    const unionTypes =
      getSourceOrderedUnionTypes(type, sourceTypeNode, wellKnownTypes, visitedTypes) ??
      getCanonicalFallbackUnionTypes(type.getUnionTypes(), wellKnownTypes, visitedTypes);

    const [aliasRawName, aliasedTypeArgs] = aliased;

    if (argsInternal.length > 0 || !aliasRawName) {
      const args = argsInternal.map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes));

      return {
        kind: 'union',
        name: aliasName,
        owner,
        unionTypes,
        optional: isOptional,
        typeParams: args,
        originalTypeName: undefined,
      };
    }

    const aliasedArgs = aliasedTypeArgs.map((arg) =>
      getTypeFromTsMorph(arg, false, wellKnownTypes),
    );

    return {
      kind: 'union',
      name: aliasName,
      owner,
      unionTypes,
      optional: isOptional,
      typeParams: aliasedArgs,
      originalTypeName: aliasRawName,
    };
  }

  if (type.isClass()) {
    return {
      kind: 'class',
      name: aliasName ?? rawName,
      owner,
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      optional: isOptional,
    };
  }

  if (type.isInterface()) {
    return {
      kind: 'interface',
      name: aliasName ?? rawName,
      owner,
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      optional: isOptional,
      typeParams: type
        .getAliasTypeArguments()
        .map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes)),
    };
  }

  // These will handle record types. However, record type is devoid
  // of details, and hence we don't support record type at the SDK level
  if (type.isObject() && type.getProperties().length === 0) {
    const name = rawName ?? aliasName ?? type.getText();

    return {
      kind: 'others',
      name: name,
      owner,
      optional: isOptional,
      recursive: false,
    };
  }

  if (type.isObject()) {
    const args = tsMorphType
      .getAliasTypeArguments()
      .map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes));

    return {
      kind: 'object',
      name: aliasName,
      owner,
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      typeParams: args,
      optional: isOptional,
    };
  }

  if (type.isNull()) {
    return { kind: 'null', name: aliasName, owner, optional: isOptional };
  }

  if (type.isBigInt()) {
    return { kind: 'bigint', name: aliasName, owner, optional: isOptional };
  }

  if (type.isUndefined()) {
    return { kind: 'undefined', name: aliasName, owner, optional: isOptional };
  }

  if (type.isNumber()) {
    return { kind: 'number', name: aliasName, owner, optional: isOptional };
  }

  if (type.isString()) {
    return { kind: 'string', name: aliasName, owner, optional: isOptional };
  }

  if (type.getTypeArguments().length === 1) {
    throw new Error(`Unhandled type with single type argument: ${type.getText()}`);
  }

  return {
    kind: 'others',
    name: aliasName ?? type.getText(),
    owner,
    optional: isOptional,
    recursive: false,
  };
}

// This is intentionally used as a deterministic fallback order for union members.
// Source-order recovery (when AST nodes are available) overrides this fallback.
function getCanonicalFallbackUnionTypes(
  unionTypes: TsMorphType[],
  wellKnownTypes: WellKnownTypes,
  visitedTypes: VisitedTypes,
): Type.Type[] {
  const withKeys = unionTypes.map((member, index) => {
    const mapped = getTypeFromTsMorphInternal(member, false, wellKnownTypes, new Map(visitedTypes));
    return {
      index,
      mapped,
      key: getCanonicalUnionSortKey(mapped),
    };
  });

  withKeys.sort((a, b) => {
    if (a.key < b.key) return -1;
    if (a.key > b.key) return 1;
    return a.index - b.index;
  });

  return withKeys.map(({ mapped }) => mapped);
}

function getSourceOrderedUnionTypes(
  unionType: TsMorphType,
  sourceTypeNode: TsMorphNode | undefined,
  wellKnownTypes: WellKnownTypes,
  visitedTypes: VisitedTypes,
): Type.Type[] | undefined {
  const sourceUnionTypeNode =
    resolveUnionTypeNode(sourceTypeNode) ?? resolveUnionTypeNodeFromType(unionType);

  if (!sourceUnionTypeNode) return undefined;

  return sourceUnionTypeNode
    .getTypeNodes()
    .map((member) =>
      getTypeFromTsMorphInternal(
        member.getType(),
        false,
        wellKnownTypes,
        new Map(visitedTypes),
        member,
      ),
    );
}

function resolveUnionTypeNodeFromType(type: TsMorphType): UnionTypeNode | undefined {
  const aliasSymbol = type.getAliasSymbol();
  if (!aliasSymbol) return undefined;

  for (const declaration of aliasSymbol.getDeclarations()) {
    if (!TsMorphNode.isTypeAliasDeclaration(declaration)) continue;

    const unionTypeNode = resolveUnionTypeNode(declaration.getTypeNode());
    if (unionTypeNode) return unionTypeNode;
  }

  return undefined;
}

function resolveUnionTypeNode(node: TsMorphNode | undefined): UnionTypeNode | undefined {
  if (!node) return undefined;

  if (TsMorphNode.isUnionTypeNode(node)) {
    return node;
  }

  if (TsMorphNode.isParenthesizedTypeNode(node)) {
    return resolveUnionTypeNode(node.getTypeNode());
  }

  return undefined;
}

function getCanonicalUnionSortKey(type: Type.Type): string {
  const rank = getUnionTypeKindRank(type.kind).toString().padStart(2, '0');
  return `${rank}:${JSON.stringify(buildJSONFromType(type))}`;
}

function getUnionTypeKindRank(kind: Type.Type['kind']): number {
  switch (kind) {
    case 'undefined':
      return 0;
    case 'null':
      return 1;
    case 'void':
      return 2;
    case 'string':
      return 3;
    case 'number':
      return 4;
    case 'bigint':
      return 5;
    case 'boolean':
      return 6;
    case 'literal':
      return 7;
    case 'tuple':
      return 8;
    case 'array':
      return 9;
    case 'map':
      return 10;
    case 'object':
      return 11;
    case 'interface':
      return 12;
    case 'class':
      return 13;
    case 'promise':
      return 14;
    case 'union':
      return 15;
    case 'config':
      return 16;
    case 'quota-token':
      return 17;
    case 'principal':
      return 18;
    case 'path':
      return 19;
    case 'url':
      return 20;
    case 'datetime':
      return 21;
    case 'duration':
      return 22;
    case 'quantity':
      return 23;
    case 'alias':
      return 24;
    case 'others':
      return 25;
    case 'unresolved-type':
      return 26;
  }
}

function getQuantitySpec(type: TsMorphType): Type.QuantitySpec | undefined {
  const specType = type.getTypeArguments()[0];
  if (!specType) return undefined;

  const declaration =
    specType.getSymbol()?.getDeclarations()[0] ?? specType.getAliasSymbol()?.getDeclarations()[0];
  if (!declaration) return undefined;
  const baseUnit = literalProperty(specType, 'baseUnit');
  const allowedSuffixesType = specType
    .getProperty('allowedSuffixes')
    ?.getTypeAtLocation(declaration);
  const tupleElements = allowedSuffixesType?.getTupleElements();
  const allowedSuffixes = tupleElements?.map((element) => element.getLiteralValue());

  if (
    baseUnit === undefined ||
    tupleElements === undefined ||
    !allowedSuffixesType?.isTuple() ||
    allowedSuffixes === undefined ||
    allowedSuffixes.length !== tupleElements.length ||
    !allowedSuffixes.every((value): value is string => typeof value === 'string')
  ) {
    throw new Error(
      'Quantity<T> type parameter must have a literal baseUnit and a tuple of string-literal allowedSuffixes',
    );
  }

  return {
    baseUnit,
    allowedSuffixes,
  };
}

function literalProperty(type: TsMorphType, name: string): string | undefined {
  const declaration =
    type.getSymbol()?.getDeclarations()[0] ?? type.getAliasSymbol()?.getDeclarations()[0];
  if (!declaration) return undefined;
  const value = type.getProperty(name)?.getTypeAtLocation(declaration).getLiteralValue();
  return typeof value === 'string' ? value : undefined;
}

// TypeLiteral in TS AST is `type A = {}`. Union types, etc. get other node types
function resolveStrictTypeLiteralNode(type: TsMorphType): TypeLiteralNode | undefined {
  const symbol = type.getSymbol();
  if (!symbol) return undefined;

  const typeLiteralDecl = symbol.getDeclarations().find(TsMorphNode.isTypeLiteral);

  return typeLiteralDecl;
}

// Extracts all leaf config properties from a type literal node, recursing into
// nested type literals. Returns the flattened leaf properties and a
// path-to-required-keys mapping for every intermediate node (deepest first),
// which loadConfig uses for pruning.
//
// `hasOptionalAncestor` tracks whether any ancestor was declared optional (`?:`),
// which propagates `optional: true` to all descendant leaves.
function extractConfigPropertiesFromTypeLiteral(
  node: TypeLiteralNode,
  path: string[],
  hasOptionalAncestor: boolean,
  wellKnownTypes: WellKnownTypes,
):
  | {
      properties: Type.ConfigProperty[];
      requiredMembers: { path: string[]; requiredKeys: string[] }[];
    }
  | undefined {
  const members = node.getMembers();

  if (!members.every(TsMorphNode.isPropertySignature)) {
    return undefined;
  }

  const properties: Type.ConfigProperty[] = [];

  // Entries collected from nested nodes (depth first); this node's own entry
  // is appended after all children so the array stays depth-first.
  const nestedRequiredMembers: { path: string[]; requiredKeys: string[] }[] = [];
  const requiredKeys: string[] = [];

  for (const member of members) {
    const name = member.getName();
    const nextPath = [...path, name];
    const memberOptional = member.hasQuestionToken();
    const isOptional = memberOptional || hasOptionalAncestor;

    // For optional properties (`field?: T`), ts-morph returns `T | undefined`.
    // Strip the `undefined` member so well-known-type checks work correctly.
    const rawPropType = unwrapAlias(member.getType());
    const propType =
      memberOptional && rawPropType.isUnion()
        ? (rawPropType.getUnionTypes().find((t) => !t.isUndefined()) ?? rawPropType)
        : rawPropType;

    if (!memberOptional) requiredKeys.push(name);

    // 1. secret wrapper
    if (isExactly(propType, wellKnownTypes.sdk.secret)) {
      properties.push({
        path: nextPath,
        secret: true,
        type: getTypeFromTsMorph(propType.getTypeArguments()[0], isOptional, wellKnownTypes),
      });
      continue;
    }

    // 2. nested type literal
    const nestedTypeLiteral = resolveStrictTypeLiteralNode(propType);
    if (nestedTypeLiteral != null) {
      const nested = extractConfigPropertiesFromTypeLiteral(
        nestedTypeLiteral,
        nextPath,
        isOptional,
        wellKnownTypes,
      );
      if (nested == null) return undefined;
      properties.push(...nested.properties);
      nestedRequiredMembers.push(...nested.requiredMembers);
      continue;
    }

    // 3. scalar leaf
    properties.push({
      path: nextPath,
      secret: false,
      type: getTypeFromTsMorph(propType, isOptional, wellKnownTypes),
    });
  }

  const requiredMembers = [...nestedRequiredMembers, { path, requiredKeys }];

  return { properties, requiredMembers };
}

type RawName = string;

function getAliasTypeArgumentsSafe(type: TsMorphType): [RawName | undefined, TsMorphType[]] {
  const aliasSymbol = type.getAliasSymbol();
  if (!aliasSymbol) return [undefined, []];

  const decl = aliasSymbol.getDeclarations()[0];
  if (!decl || !decl.isKind(ts.SyntaxKind.TypeAliasDeclaration)) return [undefined, []];

  const typeNode = decl.getTypeNodeOrThrow();
  const typeRef = typeNode.asKind(ts.SyntaxKind.TypeReference);
  if (!typeRef) return [undefined, []];

  return [typeRef.getTypeName().getText(), typeRef.getTypeArguments().map((arg) => arg.getType())];
}

export function getRawTypeName(type: TsMorphType): string | undefined {
  const rawName = type.getSymbol()?.getName();

  if (!rawName || rawName === '__type') {
    const alias = type.getAliasSymbol()?.getName();

    if (!alias || alias === '__type') {
      return type.getText();
    }

    return alias;
  }

  return rawName;
}

export function getAliasTypeName(type: TsMorphType): string | undefined {
  const alias = type.getAliasSymbol()?.getName();
  if (!alias || alias === '__type') {
    return undefined;
  }
  return alias;
}

export function getTypeOwner(type: TsMorphType, sourceTypeNode?: TsMorphNode): string | undefined {
  return (
    getOwnerFromSourceTypeNode(sourceTypeNode) ??
    getOwnerFromDeclarations(type.getAliasSymbol()?.getDeclarations()) ??
    getOwnerFromDeclarations(type.getSymbol()?.getDeclarations())
  );
}

function getOwnerFromSourceTypeNode(node: TsMorphNode | undefined): string | undefined {
  if (!node) return undefined;

  if (TsMorphNode.isTypeReference(node)) {
    const typeNameSymbol = node.getTypeName().getSymbol();
    const fromTypeName = getOwnerFromDeclarations(typeNameSymbol?.getDeclarations());
    if (fromTypeName) return fromTypeName;

    return getOwnerFromDeclarations(node.getType().getAliasSymbol()?.getDeclarations());
  }

  return undefined;
}

function getOwnerFromDeclarations(declarations: TsMorphNode[] | undefined): string | undefined {
  if (!declarations || declarations.length === 0) return undefined;

  for (const declaration of declarations) {
    const importDeclaration = declaration.getFirstAncestorByKind(SyntaxKind.ImportDeclaration);
    if (importDeclaration) {
      return importDeclaration.getModuleSpecifierValue();
    }

    const moduleDeclaration = declaration.getFirstAncestorByKind(SyntaxKind.ModuleDeclaration);
    if (moduleDeclaration) {
      const moduleName = moduleDeclaration.getName();
      return moduleName.replace(/^['"]|['"]$/g, '');
    }
  }

  return undefined;
}

export function unwrapAlias(type: TsMorphType): TsMorphType {
  let current = type;

  const visited = new Set<TsMorphType>();

  while (true) {
    const aliasSymbol = current.getAliasSymbol();
    if (!aliasSymbol || visited.has(current)) break;
    visited.add(current);

    const decl = aliasSymbol.getDeclarations()[0];
    if (!decl) break;

    const realType = decl.getType();

    if (realType === current) break;
    current = realType;
  }

  return current;
}

/**
 *
 * Configuration for generating class metadata.
 * - sourceFiles: Array of ts-morph SourceFile objects to extract metadata from.
 * - classDecorators: Array of decorator names to filter classes. If empty, all classes are included.
 * - includeOnlyPublicScope: If true, only public constructors and methods are included
 */
export type ClassMetadataGenConfig = {
  sourceFiles: SourceFile[];
  classDecorators: string[];
  includeOnlyPublicScope: boolean;
  excludeOverriddenMethods: boolean;
  golemTsSdkImport: string;
};

export function generateClassMetadata(
  classMetadataGenConfig: ClassMetadataGenConfig,
  project: Project,
) {
  updateMetadataFromSourceFiles(classMetadataGenConfig, project);
  return saveAndClearInMemoryMetadata();
}

export function updateMetadataFromSourceFiles(
  classMetadataGenConfig: ClassMetadataGenConfig,
  project: Project,
) {
  const wellKnownTypes = createWellKnownTypes(project, classMetadataGenConfig.golemTsSdkImport);
  for (const sourceFile of classMetadataGenConfig.sourceFiles) {
    const classes = sourceFile.getClasses();

    for (const classDecl of classes) {
      if (classMetadataGenConfig.classDecorators.length > 0) {
        const hasAnyConfiguredDecorator = classDecl
          .getDecorators()
          .some((d) => classMetadataGenConfig.classDecorators.includes(d.getName()));

        if (!hasAnyConfiguredDecorator) {
          continue;
        }
      }

      const className = classDecl.getName();
      if (!className) continue;

      const publicConstructors = classMetadataGenConfig.includeOnlyPublicScope
        ? classDecl.getConstructors().filter((ctor) => ctor.getScope() === Scope.Public)
        : classDecl.getConstructors();

      const constructorArgs =
        publicConstructors.length === 0
          ? []
          : publicConstructors[0].getParameters().map((p) => ({
              name: p.getName(),
              type: getTypeFromTsMorph(
                p.getType(),
                p.isOptional(),
                wellKnownTypes,
                p.getTypeNode(),
              ),
            }));

      const methods = new Map();

      const publicMethods = classMetadataGenConfig.includeOnlyPublicScope
        ? classDecl.getMethods().filter((m) => m.getScope() === Scope.Public)
        : classDecl.getMethods();

      for (const method of publicMethods) {
        if (
          classMetadataGenConfig.excludeOverriddenMethods &&
          (method.hasOverrideKeyword() || isOverriddenMethod(method))
        ) {
          continue;
        }

        const methodParams = new Map(
          method.getParameters().map((p) => {
            return [
              p.getName(),
              getTypeFromTsMorph(p.getType(), p.isOptional(), wellKnownTypes, p.getTypeNode()),
            ];
          }),
        );

        const returnType = getTypeFromTsMorph(
          method.getReturnType(),
          false,
          wellKnownTypes,
          method.getReturnTypeNode(),
        );
        methods.set(method.getName(), { methodParams, returnType });
      }

      const isScopeAllowed = (decl: { getScope: () => Scope }) =>
        !classMetadataGenConfig.includeOnlyPublicScope || decl.getScope() === Scope.Public;

      const publicArrows = classDecl
        .getProperties()
        .filter(
          (p) =>
            isScopeAllowed(p) &&
            p.getType().getCallSignatures().length > 0 &&
            p.hasInitializer() &&
            (p.getInitializerIfKind(SyntaxKind.ArrowFunction) ||
              p.getInitializerIfKind(SyntaxKind.FunctionExpression)),
        );

      for (const publicArrow of publicArrows) {
        if (
          classMetadataGenConfig.excludeOverriddenMethods &&
          (publicArrow.hasOverrideKeyword() || isOverriddenProperty(publicArrow))
        ) {
          continue;
        }

        const arrowType = publicArrow.getType();
        const callSignature = arrowType.getCallSignatures()[0];
        if (!callSignature) continue;

        const methodParams = new Map(
          callSignature.getParameters().map((p) => {
            const decl = p.getDeclarations()[0];
            if (!decl) {
              throw new Error(
                `No declaration found for parameter ${p.getName()} in arrow method ${publicArrow.getName()} of class ${className}`,
              );
            }
            const paramType = p.getTypeAtLocation(decl);
            const isOptional = TsMorphNode.isParameterDeclaration(decl) ? decl.isOptional() : false;
            const sourceTypeNode = TsMorphNode.isParameterDeclaration(decl)
              ? decl.getTypeNode()
              : undefined;
            return [
              p.getName(),
              getTypeFromTsMorph(paramType, isOptional, wellKnownTypes, sourceTypeNode),
            ];
          }),
        );

        const returnType = getTypeFromTsMorph(callSignature.getReturnType(), false, wellKnownTypes);
        methods.set(publicArrow.getName(), { methodParams, returnType });
      }

      TypeMetadata.update(className, constructorArgs, methods);
    }
  }
}

function isOverriddenMethod(method: MethodDeclaration): boolean {
  const classDecl = method.getFirstAncestorByKind(SyntaxKind.ClassDeclaration);
  if (!classDecl) return false;

  let currentBase: ClassDeclaration | undefined = classDecl.getBaseClass();
  const methodName = method.getName();

  while (currentBase) {
    const baseMethod = currentBase.getInstanceMethod(methodName);
    if (baseMethod) return true;

    currentBase = currentBase.getBaseClass();
  }

  return false;
}

function isOverriddenProperty(prop: PropertyDeclaration): boolean {
  const classDecl = prop.getFirstAncestorByKind(SyntaxKind.ClassDeclaration);
  if (!classDecl) return false;

  let currentBase: ClassDeclaration | undefined = classDecl.getBaseClass();
  const propName = prop.getName();

  while (currentBase) {
    const baseProp = currentBase.getInstanceProperty(propName);
    if (baseProp) return true;

    // See if overriding a method with an arrow
    const baseMethod = currentBase.getInstanceMethod(propName);
    if (baseMethod) return true;

    currentBase = currentBase.getBaseClass();
  }

  return false;
}

const METADATA_DIR = '.metadata';
const METADATA_TS_FILE = 'generated-types.ts';
const METADATA_JSON_FILE = 'generated-types.json';

export function saveAndClearInMemoryMetadata() {
  if (!fs.existsSync(METADATA_DIR)) {
    fs.mkdirSync(METADATA_DIR);
  }

  const json: Record<string, any> = {};

  for (const [className, meta] of TypeMetadata.getAll().entries()) {
    const constructorArgsJSON = meta.constructorArgs.map((arg) => ({
      name: arg.name,
      type: buildJSONFromType(arg.type),
    }));

    const methodsObj: Record<string, any> = {};
    for (const [methodName, { methodParams, returnType }] of meta.methods) {
      const paramsJSON: Record<string, LiteTypeJSON> = {};
      for (const [paramName, paramType] of methodParams.entries()) {
        paramsJSON[paramName] = buildJSONFromType(paramType);
      }

      methodsObj[methodName] = {
        methodParams: paramsJSON,
        returnType: buildJSONFromType(returnType),
      };
    }

    json[className] = {
      constructorArgs: constructorArgsJSON,
      methods: methodsObj,
    };
  }

  const tsFilePath = path.join(METADATA_DIR, METADATA_TS_FILE);
  const jsonFilePath = path.join(METADATA_DIR, METADATA_JSON_FILE);

  const tsContent = `export const Metadata = ${JSON.stringify(json, null, 2)};`;
  const jsonContent = JSON.stringify(json, null, 2);

  fs.writeFileSync(tsFilePath, tsContent, 'utf-8');
  fs.writeFileSync(jsonFilePath, jsonContent, 'utf-8');

  TypeMetadata.clearAll();

  return tsFilePath;
}

export function lazyLoadTypeMetadata() {
  if (TypeMetadata.getAll().size === 0) {
    loadTypeMetadataFromJsonFile();
  }
}

export function loadTypeMetadataFromJsonFile() {
  TypeMetadata.clearMetadata();

  const filePath = path.join(METADATA_DIR, METADATA_JSON_FILE);
  if (!fs.existsSync(filePath)) {
    throw new Error(`${filePath} does not exist`);
  }

  const raw = fs.readFileSync(filePath, 'utf-8');
  const json = JSON.parse(raw);
  TypeMetadata.loadFromJson(json);
}

function propertiesAsSymbols(
  type: TsMorphType,
  wellknownTypes: WellKnownTypes,
  visitedTypes: VisitedTypes,
): Symbol[] {
  return type.getProperties().map((prop) => {
    const firstDeclaration = prop.getDeclarations()[0];
    // NOTE: falling back to firstDeclaration if no value declaration found,
    //       to support runtime generated or manipulated types
    const type = prop.getTypeAtLocation(getValueDeclaration(prop) ?? firstDeclaration);
    const sourceTypeNode =
      TsMorphNode.isPropertySignature(firstDeclaration) ||
      TsMorphNode.isPropertyDeclaration(firstDeclaration)
        ? firstDeclaration.getTypeNode()
        : undefined;
    const tsType = getTypeFromTsMorphInternal(
      type,
      false,
      wellknownTypes,
      new Map(visitedTypes),
      sourceTypeNode,
    );
    const propName = prop.getName();

    if (
      (TsMorphNode.isPropertySignature(firstDeclaration) ||
        TsMorphNode.isPropertyDeclaration(firstDeclaration)) &&
      firstDeclaration.hasQuestionToken()
    ) {
      return new Symbol({
        name: propName,
        declarations: [new Node('PropertyDeclaration', true)],
        typeAtLocation: tsType,
      });
    } else {
      return new Symbol({
        name: propName,
        declarations: [new Node('PropertyDeclaration', false)],
        typeAtLocation: tsType,
      });
    }
  });
}

function getValueDeclaration(symbol: TsMorphSymbol): TsMorphNode | undefined {
  try {
    return symbol.getValueDeclarationOrThrow();
  } catch {
    return undefined;
  }
}

export function getNominalSymbol(type: TsMorphType): TsMorphSymbol | undefined {
  const aliasSymbol = type.getAliasSymbol();
  if (aliasSymbol) {
    const declared = aliasSymbol.getDeclaredType?.();
    if (declared) {
      type = declared;
    }
  }

  const target = type.getTargetType?.();
  if (target) {
    type = target;
  }

  return type.getSymbol();
}

function isExactly(type: TsMorphType, wellKnown: WellKnown): boolean {
  const nominalSymbol = getNominalSymbol(type);
  if (nominalSymbol == null) {
    return false;
  }

  return (
    nominalSymbol === wellKnown.symbol || sameSymbolDeclaration(nominalSymbol, wellKnown.symbol)
  );
}

function isExactlyBuiltIn(type: TsMorphType, wellKnown: WellKnown): boolean {
  const nominalSymbol = getNominalSymbol(type);
  if (nominalSymbol == null || !isExactly(type, wellKnown)) return false;
  const declarationPaths = nominalSymbol
    .getDeclarations()
    .map((declaration) => declaration.getSourceFile().getFilePath());
  return (
    declarationPaths.some((filePath) => filePath.includes('/typescript/lib/')) &&
    declarationPaths.every((filePath) => filePath.includes('/node_modules/'))
  );
}

function sameSymbolDeclaration(left: TsMorphSymbol, right: TsMorphSymbol): boolean {
  return left
    .getDeclarations()
    .some((leftDeclaration) =>
      right
        .getDeclarations()
        .some(
          (rightDeclaration) =>
            leftDeclaration.getSourceFile().getFilePath() ===
              rightDeclaration.getSourceFile().getFilePath() &&
            leftDeclaration.getStart() === rightDeclaration.getStart(),
        ),
    );
}

function isConcreteType(type: TsMorphType): boolean {
  const flags = type.getFlags();
  return !(flags & ts.TypeFlags.Any || flags & ts.TypeFlags.Never || flags & ts.TypeFlags.Unknown);
}

function containsInvalidTypes(type: TsMorphType): boolean {
  if (!type.isUnion()) {
    return !isConcreteType(type);
  }

  return type.getUnionTypes().some((t) => !isConcreteType(t));
}
