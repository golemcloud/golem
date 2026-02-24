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

import { Symbol } from './symbol';

export type ConfigProperty = { path: string[], secret: boolean, type: Type };

export type Type =
  | { kind: 'boolean'; name?: string; optional: boolean }
  | { kind: 'number'; name?: string; optional: boolean }
  | { kind: 'string'; name?: string; optional: boolean }
  | { kind: 'bigint'; name?: string; optional: boolean }
  | { kind: 'null'; name?: string; optional: boolean }
  | { kind: 'undefined'; name?: string; optional: boolean }
  | { kind: 'array'; name?: string; element: Type; optional: boolean }
  | { kind: 'null'; name?: string; element: Type; optional: boolean }
  | { kind: 'tuple'; name?: string; elements: Type[]; optional: boolean }
  | {
      kind: 'union';
      name?: string;
      unionTypes: Type[];
      typeParams: Type[];
      optional: boolean;
      originalTypeName: string | undefined;
    }
  | {
      kind: 'object';
      name?: string;
      properties: Symbol[];
      typeParams: Type[];
      optional: boolean;
    }
  | { kind: 'class'; name?: string; properties: Symbol[]; optional: boolean }
  | {
      kind: 'interface';
      name?: string;
      properties: Symbol[];
      typeParams: Type[];
      optional: boolean;
    }
  | { kind: 'promise'; name?: string; element: Type; optional: boolean }
  | { kind: 'map'; name?: string; key: Type; value: Type; optional: boolean }
  | { kind: 'literal'; name?: string; literalValue?: string; optional: boolean }
  | { kind: 'alias'; name?: string; aliasSymbol: Symbol; optional: boolean }
  | { kind: 'void'; name?: string; optional: boolean }
  | { kind: 'others'; name?: string; optional: boolean; recursive: boolean }
  | { kind: 'config', name?: string, optional: boolean, properties: ConfigProperty[] }
  | {
      kind: 'unresolved-type';
      name?: string;
      optional: boolean;
      text: string;
      error: string;
    };

export function getName(t: Type): string | undefined {
  if (t.kind === 'others') return t.name;
  return undefined;
}

export function getTypeArguments(t: Type): Symbol[] {
  return t.kind === 'class' ? t.properties : [];
}

export function getTupleElements(t: Type): Type[] {
  return t.kind === 'tuple' ? t.elements : [];
}

export function getArrayElementType(t: Type): Type | undefined {
  return t.kind === 'array' ? t.element : undefined;
}

export function getUnionTypes(t: Type): Type[] {
  return t.kind === 'union' ? t.unionTypes : [];
}

export function getProperties(t: Type): Symbol[] {
  return t.kind === 'object' || t.kind === 'interface' ? t.properties : [];
}

export function getPromiseElementType(t: Type): Type | undefined {
  return t.kind === 'promise' ? t.element : undefined;
}

export function getAliasSymbol(t: Type): Symbol | undefined {
  return t.kind === 'alias' ? t.aliasSymbol : undefined;
}

export function getTypeName(t: Type): string {
  return t.name ? t.name : t.kind;
}

export function unwrapAlias(t: Type): Type {
  let current = t;
  const seen = new Set<Type>();
  while (true) {
    const alias = getAliasSymbol(current);
    if (!alias || seen.has(current)) break;
    seen.add(current);

    const decl = alias.getDeclarations()[0];
    if (!decl) break;

    const target = (alias as any)._getAliasTarget?.() as Type | undefined;
    if (!target || target === current) break;

    current = target;
  }
  return current;
}
