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

import { LiteTypeJSON } from './type-json';
import * as Type from './type-lite';

export function buildJSONFromType(type: Type.Type): LiteTypeJSON {
  switch (type.kind) {
    case 'number':
      return { kind: 'number', name: type.name };

    case 'string':
      return { kind: 'string', name: type.name };

    case 'bigint':
      return { kind: 'bigint', name: type.name };

    case 'null':
      return { kind: 'null', name: type.name };

    case 'undefined':
      return { kind: 'undefined', name: type.name };

    case 'void':
      return { kind: 'void', name: type.name };

    case 'array':
      const elem = Type.getArrayElementType(type);

      if (!elem) throw new Error('Missing element type in Array');

      return {
        kind: 'array',
        name: type.name,
        element: buildJSONFromType(elem),
      };

    case 'tuple':
      return {
        kind: 'tuple',
        name: type.name,
        elements: type.elements.map(buildJSONFromType),
      };

    case 'union':
      return {
        kind: 'union',
        name: type.name,
        types: type.unionTypes.map(buildJSONFromType),
      };

    case 'object':
      const props = type.properties.map((sym) => {
        const decl = sym.getDeclarations()[0];
        const optional = decl.hasQuestionToken?.() ?? false;
        const propType = sym.getTypeAtLocation(decl);
        return {
          name: sym.getName(),
          type: buildJSONFromType(propType),
          optional: optional || undefined,
        };
      });

      return {
        kind: 'object',
        name: type.name,
        properties: props,
      };

    case 'class':
      const classProps = type.properties.map((sym) => {
        const decl = sym.getDeclarations()[0];
        const optional = decl.hasQuestionToken?.() ?? false;
        const propType = sym.getTypeAtLocation(decl);
        return {
          name: sym.getName(),
          type: buildJSONFromType(propType),
          optional: optional || undefined,
        };
      });

      return {
        kind: 'class',
        name: type.name,
        properties: classProps,
      };

    case 'interface':
      const interfaceProps = type.properties.map((sym) => {
        const decl = sym.getDeclarations()[0];
        const optional = decl.hasQuestionToken?.() ?? false;
        const propType = sym.getTypeAtLocation(decl);
        return {
          name: sym.getName(),
          type: buildJSONFromType(propType),
          optional: optional || undefined,
        };
      });

      return {
        kind: 'interface',
        name: type.name,
        properties: interfaceProps,
      };

    case 'promise':
      const elementType = type.element;

      if (!elementType) throw new Error('Missing element type in Promise');

      return {
        kind: 'promise',
        name: type.name,
        element: buildJSONFromType(elementType),
      };

    case 'map':
      const key = type.key;
      const value = type.value;

      const keyJson = buildJSONFromType(key);
      const valueJson = buildJSONFromType(value);

      return {
        kind: 'map',
        name: type.name,
        typeArgs: [keyJson, valueJson],
      };

    case 'literal':
      return {
        kind: 'literal',
        name: type.name,
        literalValue: type.literalValue,
      };

    case 'alias':
      const aliasSym = type.aliasSymbol;
      if (!aliasSym) throw new Error('Alias missing symbol');
      const target = aliasSym.getAliasTarget();
      if (!target) throw new Error('Alias missing target');
      return {
        kind: 'alias',
        name: type.name ?? 'alias',
        target: buildJSONFromType(target),
      };

    case 'others':
      return { kind: 'others', name: type.name };

    case 'boolean':
      return { kind: 'boolean', name: type.name };
  }
}
