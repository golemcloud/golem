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
  Node as TsMorphNode,
  Scope,
  SourceFile,
  Type as TsMorphType,
} from "ts-morph";
import {
  buildJSONFromType,
  LiteTypeJSON,
  Node,
  Symbol,
  Type,
  TypeMetadata,
} from "@golemcloud/golem-ts-types-core";

import * as fs from "node:fs";
import path from "path";

export function getFromTsMorph(tsMorphType: TsMorphType): Type.Type {
  const type = unwrapAlias(tsMorphType);
  const rawName = getRawTypeName(type);
  const aliasName = getAliasTypeName(type);
  const name = aliasName ?? rawName ?? type.getText();

  switch (rawName) {
    case "Float64Array":
      return {
        kind: "array",
        name: "Float64Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Float32Array":
      return {
        kind: "array",
        name: "Float32Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Int8Array":
      return {
        kind: "array",
        name: "Int8Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Uint8Array":
      return {
        kind: "array",
        name: "Uint8Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Int16Array":
      return {
        kind: "array",
        name: "Int16Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Uint16Array":
      return {
        kind: "array",
        name: "Uint16Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Int32Array":
      return {
        kind: "array",
        name: "Int32Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "Uint32Array":
      return {
        kind: "array",
        name: "Uint32Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "BigInt64Array":
      return {
        kind: "array",
        name: "BigInt64Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
    case "BigUint64Array":
      return {
        kind: "array",
        name: "BigUint64Array",
        element: {
          kind: "number",
          name: "number",
        },
      };
  }

  // These will handle record types. However, record type is devoid
  // of details, and hence we don't support record type at the SDK level
  if (type.isObject() && type.getProperties().length === 0) {
    const name = rawName ?? aliasName ?? type.getText();

    return {
      kind: "others",
      name: name,
    };
  }

  if (rawName === "Promise" && type.getTypeArguments().length === 1) {
    const inner = type.getTypeArguments()[0];
    const promiseType = getFromTsMorph(inner);

    return {
      kind: "promise",
      name,
      element: promiseType,
    };
  }

  if (rawName === "Map" && type.getTypeArguments().length === 2) {
    const [keyT, valT] = type.getTypeArguments();
    const key = getFromTsMorph(keyT);
    const value = getFromTsMorph(valT);
    return {
      kind: "map",
      name,
      key: key,
      value: value,
    };
  }

  if (type.isVoid()) {
    return { kind: "void", name: "void" };
  }

  if (type.isBoolean() || rawName === "true" || rawName === "false") {
    return { kind: "boolean", name: "boolean" };
  }

  if (type.isLiteral()) {
    return { kind: "literal", name: type.getText() };
  }

  if (type.isTuple()) {
    const tupleElems = type.getTupleElements().map((el) => getFromTsMorph(el));

    return {
      kind: "tuple",
      name,
      elements: tupleElems,
    };
  }

  if (type.isArray()) {
    const elementType = type.getArrayElementType();
    if (!elementType) {
      throw new Error("Array type without element type");
    }

    const element = getFromTsMorph(elementType);

    return {
      kind: "array",
      name,
      element,
    };
  }

  if (type.isUnion()) {
    const unionTypes = type.getUnionTypes().map((t) => getFromTsMorph(t));

    return {
      kind: "union",
      name,
      unionTypes,
    };
  }

  if (type.isClass()) {
    const result: Symbol[] = type.getProperties().map((prop) => {
      const type = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());
      const nodes = prop.getDeclarations();
      const node = nodes[0];
      const tsType = getFromTsMorph(type);
      const propName = prop.getName();

      if (
        (TsMorphNode.isPropertySignature(node) ||
          TsMorphNode.isPropertyDeclaration(node)) &&
        node.hasQuestionToken()
      ) {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", true)],
          typeAtLocation: tsType,
        });
      } else {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", false)],
          typeAtLocation: tsType,
        });
      }
    });

    return {
      kind: "class",
      name,
      properties: result,
    };
  }

  if (type.isInterface()) {
    const result: Symbol[] = type.getProperties().map((prop) => {
      const type = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());
      const nodes = prop.getDeclarations();
      const node = nodes[0];
      const tsType = getFromTsMorph(type);
      const propName = prop.getName();

      if (
        (TsMorphNode.isPropertySignature(node) ||
          TsMorphNode.isPropertyDeclaration(node)) &&
        node.hasQuestionToken()
      ) {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", true)],
          typeAtLocation: tsType,
        });
      } else {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", false)],
          typeAtLocation: tsType,
        });
      }
    });

    return {
      kind: "interface",
      name,
      properties: result,
    };
  }

  if (type.isObject()) {
    const result: Symbol[] = type.getProperties().map((prop) => {
      const type = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());
      const nodes = prop.getDeclarations();
      const node = nodes[0];
      const tsType = getFromTsMorph(type);
      const propName = prop.getName();

      if (
        (TsMorphNode.isPropertySignature(node) ||
          TsMorphNode.isPropertyDeclaration(node)) &&
        node.hasQuestionToken()
      ) {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", true)],
          typeAtLocation: tsType,
        });
      } else {
        return new Symbol({
          name: propName,
          declarations: [new Node("PropertyDeclaration", false)],
          typeAtLocation: tsType,
        });
      }
    });

    return {
      kind: "object",
      name,
      properties: result,
    };
  }

  if (type.isNull()) {
    return { kind: "null", name: "null" };
  }

  if (type.isBigInt()) {
    return { kind: "bigint", name: "bigint" };
  }

  if (type.isUndefined()) {
    return { kind: "undefined", name: "undefined" };
  }

  if (type.isNumber()) {
    return { kind: "number", name: "number" };
  }

  if (type.isString()) {
    return { kind: "string", name: "string" };
  }

  return { kind: "others", name: type.getText() };
}

export function getRawTypeName(type: TsMorphType): string | undefined {
  const rawName = type.getSymbol()?.getName();

  if (!rawName || rawName === "__type") {
    const alias = type.getAliasSymbol()?.getName();

    if (!alias || alias === "__type") {
      return type.getText();
    }

    return alias;
  }

  return rawName;
}

export function getAliasTypeName(type: TsMorphType): string | undefined {
  const alias = type.getAliasSymbol()?.getName();
  if (!alias || alias === "__type") {
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

export function generateMetadata(sourceFiles: SourceFile[]) {
  updateMetadataFromSourceFiles(sourceFiles);
  return saveAndClearInMemoryMetadata();
}

export function updateMetadataFromSourceFiles(sourceFiles: SourceFile[]) {
  for (const sourceFile of sourceFiles) {
    const classes = sourceFile.getClasses();

    for (const classDecl of classes) {
      const className = classDecl.getName();
      if (!className) continue;

      const publicConstructors = classDecl
        .getConstructors()
        .filter((ctor) => ctor.getScope() === Scope.Public);

      const constructorArgs =
        publicConstructors.length === 0
          ? []
          : publicConstructors[0].getParameters().map((p) => ({
              name: p.getName(),
              type: getFromTsMorph(p.getType()),
            }));

      const methods = new Map();

      const publicMethods = classDecl
        .getMethods()
        .filter((m) => m.getScope() === Scope.Public);

      for (const method of publicMethods) {
        const methodParams = new Map(
          method.getParameters().map((p) => {
            return [p.getName(), getFromTsMorph(p.getType())];
          }),
        );

        const returnType = getFromTsMorph(method.getReturnType());
        methods.set(method.getName(), { methodParams, returnType });
      }

      TypeMetadata.update(className, constructorArgs, methods);
    }
  }
}

const METADATA_DIR = ".metadata";
const METADATA_TS_FILE = "generated-types.ts";
const METADATA_JSON_FILE = "generated-types.json";

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

  fs.writeFileSync(tsFilePath, tsContent, "utf-8");
  fs.writeFileSync(jsonFilePath, jsonContent, "utf-8");

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

  const raw = fs.readFileSync(filePath, "utf-8");
  const json = JSON.parse(raw);

  TypeMetadata.loadFromJson(json);
}
