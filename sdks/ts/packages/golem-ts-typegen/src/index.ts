// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import * as fs from 'node:fs';
import path from 'path';
import { createWellKnownTypes, WellKnown, WellKnownTypes } from './wellknownTypes';

export function getTypeFromTsMorph(tsMorphType: TsMorphType, isOptional: boolean, wellKnownTypes: WellKnownTypes): Type.Type {
  try {
    return getTypeFromTsMorphInternal(tsMorphType, isOptional, wellKnownTypes, new Set());
  } catch (e) {
    if (e instanceof Error) {
      let error = e.message;
      if (e.stack) {
        error = error + '\n\n' + e.stack;
      }

      return {
        kind: 'unresolved-type',
        name: undefined,
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
  visitedTypes: Set<TsMorphType>,
): Type.Type {
  const type = unwrapAlias(tsMorphType);
  const rawName = getRawTypeName(type);
  const aliasName = getAliasTypeName(type);

  if (visitedTypes.has(tsMorphType)) {
    return {
      kind: 'others',
      name: rawName ?? aliasName ?? type.getText(),
      optional: isOptional,
      recursive: true,
    };
  }
  visitedTypes.add(tsMorphType);

  if (isExactly(type, wellKnownTypes.object)) {
    return {
      kind: 'others',
      name: rawName,
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

  // These will handle record types. However, record type is devoid
  // of details, and hence we don't support record type at the SDK level
  if (type.isObject() && type.getProperties().length === 0) {
    const name = rawName ?? aliasName ?? type.getText();

    return {
      kind: 'others',
      name: name,
      optional: isOptional,
      recursive: false,
    };
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
    const key = getTypeFromTsMorphInternal(keyT, false, wellKnownTypes,  new Set(visitedTypes));
    const value = getTypeFromTsMorphInternal(valT, false, wellKnownTypes,  new Set(visitedTypes));
    return {
      kind: 'map',
      name: aliasName,
      key: key,
      value: value,
      optional: isOptional,
    };
  }

  if (type.isVoid()) {
    return { kind: 'void', name: 'void', optional: isOptional };
  }

  if (type.isBoolean()) {
    return { kind: 'boolean', optional: isOptional };
  }

  if (type.isLiteral()) {
    const literalValue = type.getLiteralValue() ?? type.getText();
    return {
      kind: 'literal',
      name: aliasName,
      literalValue: literalValue.toString(),
      optional: isOptional,
    };
  }

  if (type.isTuple()) {
    const tupleElems = type
      .getTupleElements()
      .map((el) => getTypeFromTsMorphInternal(el, false, wellKnownTypes, new Set(visitedTypes)));

    return {
      kind: 'tuple',
      name: aliasName,
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

    const element = getTypeFromTsMorphInternal(resolvedElementType, false, wellKnownTypes, visitedTypes);

    return {
      kind: 'array',
      name: aliasName,
      element,
      optional: isOptional,
    };
  }

  if (type.isUnion()) {
    const argsInternal = tsMorphType.getAliasTypeArguments();

    const aliased = getAliasTypeArgumentsSafe(tsMorphType);

    const unionTypes = type
      .getUnionTypes()
      .map((t) => getTypeFromTsMorphInternal(t, false, wellKnownTypes, new Set(visitedTypes)));

    const [aliasRawName, aliasedTypeArgs] = aliased;

    if (argsInternal.length > 0 || !aliasRawName) {
      const args = argsInternal.map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes));

      return {
        kind: 'union',
        name: aliasName,
        unionTypes,
        optional: isOptional,
        typeParams: args,
        originalTypeName: undefined,
      };
    }

    const aliasedArgs = aliasedTypeArgs.map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes));

    return {
      kind: 'union',
      name: aliasName,
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
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      optional: isOptional,
    };
  }

  if (type.isInterface()) {
    return {
      kind: 'interface',
      name: aliasName ?? rawName,
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      optional: isOptional,
      typeParams: type.getAliasTypeArguments().map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes)),
    };
  }

  if (type.isObject()) {
    const args = tsMorphType.getAliasTypeArguments().map((arg) => getTypeFromTsMorph(arg, false, wellKnownTypes));

    return {
      kind: 'object',
      name: aliasName,
      properties: propertiesAsSymbols(type, wellKnownTypes, visitedTypes),
      typeParams: args,
      optional: isOptional,
    };
  }

  if (type.isNull()) {
    return { kind: 'null', name: aliasName, optional: isOptional };
  }

  if (type.isBigInt()) {
    return { kind: 'bigint', name: aliasName, optional: isOptional };
  }

  if (type.isUndefined()) {
    return { kind: 'undefined', name: aliasName, optional: isOptional };
  }

  if (type.isNumber()) {
    return { kind: 'number', name: aliasName, optional: isOptional };
  }

  if (type.isString()) {
    return { kind: 'string', name: aliasName, optional: isOptional };
  }

  if (type.getTypeArguments().length === 1) {
    throw new Error(`Unhandled type with single type argument: ${type.getText()}`);
  }

  return {
    kind: 'others',
    name: aliasName ?? type.getText(),
    optional: isOptional,
    recursive: false,
  };
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
};

export function generateClassMetadata(classMetadataGenConfig: ClassMetadataGenConfig, project: Project) {
  updateMetadataFromSourceFiles(classMetadataGenConfig, project);
  return saveAndClearInMemoryMetadata();
}

export function updateMetadataFromSourceFiles(classMetadataGenConfig: ClassMetadataGenConfig, project: Project) {
  const wellKnownTypes = createWellKnownTypes(project);
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
              type: getTypeFromTsMorph(p.getType(), p.isOptional(), wellKnownTypes),
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
            return [p.getName(), getTypeFromTsMorph(p.getType(), p.isOptional(), wellKnownTypes)];
          }),
        );

        const returnType = getTypeFromTsMorph(method.getReturnType(), false, wellKnownTypes);
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
            return [p.getName(), getTypeFromTsMorph(paramType, isOptional, wellKnownTypes)];
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

function propertiesAsSymbols(type: TsMorphType, wellKnownTypes: WellKnownTypes,  visitedTypes: Set<TsMorphType>): Symbol[] {
  return type.getProperties().map((prop) => {
    const firstDeclaration = prop.getDeclarations()[0];
    // NOTE: falling back to firstDeclaration if no value declaration found,
    //       to support runtime generated or manipulated types
    const type = prop.getTypeAtLocation(getValueDeclaration(prop) ?? firstDeclaration);
    const tsType = getTypeFromTsMorphInternal(type, false, wellKnownTypes, new Set(visitedTypes));
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
  // eslint-disable-next-line eqeqeq
  if (nominalSymbol == null) {
    return false;
  }

  return nominalSymbol === wellKnown.symbol;
}
