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

export type Type =
  | { kind: 'boolean'; name?: string }
  | { kind: 'number'; name?: string }
  | { kind: 'string'; name?: string }
  | { kind: 'bigint'; name?: string }
  | { kind: 'null'; name?: string }
  | { kind: 'undefined'; name?: string }
  | { kind: 'array'; name?: string; element: Type }
  | { kind: 'tuple'; name?: string; elements: Type[] }
  | { kind: 'union'; name?: string; unionTypes: Type[] }
  | { kind: 'object'; name?: string; properties: Symbol[] }
  | { kind: 'class'; name?: string; properties: Symbol[] }
  | { kind: 'interface'; name?: string; properties: Symbol[] }
  | { kind: 'promise'; name?: string; element: Type }
  | { kind: 'map'; name?: string; key: Type; value: Type }
  | { kind: 'literal'; name?: string }
  | { kind: 'alias'; name?: string; aliasSymbol: Symbol }
  | { kind: 'others'; name?: string };

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

// Get human-readable type name (exhaustive)
export function getTypeName(t: Type): string {
  switch (t.kind) {
    case 'boolean':
    case 'number':
    case 'string':
    case 'bigint':
    case 'null':
    case 'undefined':
    case 'array':
    case 'tuple':
    case 'union':
    case 'object':
    case 'interface':
    case 'promise':
    case 'map':
    case 'literal':
    case 'class':
    case 'alias':
    case 'others':
      return t.name ?? t.kind;
  }
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
