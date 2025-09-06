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
import { Node } from './node';
import { LiteTypeJSON } from './type-json';
import { Type } from './type-lite';

export function buildTypeFromJSON(json: LiteTypeJSON): Type {
  switch (json.kind) {
    case 'others':
      return { kind: 'others', name: json.name };
    case 'boolean':
      return { kind: 'boolean', name: json.name };
    case 'number':
      return { kind: 'number', name: json.name };
    case 'string':
      return { kind: 'string', name: json.name };
    case 'bigint':
      return { kind: 'bigint', name: json.name };
    case 'null':
      return { kind: 'null', name: json.name };
    case 'undefined':
      return { kind: 'undefined', name: json.name };

    case 'array': {
      const elem = buildTypeFromJSON(json.element);
      return {
        kind: 'array',
        name: json.name,
        element: elem,
      };
    }

    case 'tuple': {
      const elems = json.elements.map(buildTypeFromJSON);
      return {
        kind: 'tuple',
        name: json.name ?? 'Tuple',
        elements: elems,
      };
    }

    case 'union': {
      const types = json.types.map(buildTypeFromJSON);
      return {
        kind: 'union',
        name: json.name ?? 'Union',
        unionTypes: types,
      };
    }

    case 'class': {
      const props = json.properties.map(
        (p) =>
          new Symbol({
            name: p.name,
            declarations: [new Node('PropertySignature', !!p.optional)],
            typeAtLocation: buildTypeFromJSON(p.type),
          }),
      );
      return {
        kind: 'class',
        name: json.name,
        properties: props,
      };
    }

    case 'object': {
      const props = json.properties.map(
        (p) =>
          new Symbol({
            name: p.name,
            declarations: [new Node('PropertySignature', !!p.optional)],
            typeAtLocation: buildTypeFromJSON(p.type),
          }),
      );
      return {
        kind: 'object',
        name: json.name,
        properties: props,
      };
    }

    case 'interface': {
      const props = json.properties.map(
        (p) =>
          new Symbol({
            name: p.name,
            declarations: [new Node('PropertyDeclaration', !!p.optional)],
            typeAtLocation: buildTypeFromJSON(p.type),
          }),
      );
      return {
        kind: 'interface',
        name: json.name,
        properties: props,
      };
    }

    case 'literal':
      return { kind: 'literal', name: json.name };

    case 'alias': {
      const target = buildTypeFromJSON(json.target);
      const aliasDecl = new Node('TypeAlias', false);
      const aliasSym = new Symbol({
        name: json.name,
        declarations: [aliasDecl],
        aliasTarget: target,
      });
      return {
        kind: 'alias',
        name: json.name,
        aliasSymbol: aliasSym,
      };
    }

    case 'promise':
      const elemType = buildTypeFromJSON(json.element);
      return {
        kind: 'promise',
        name: json.name ?? 'Promise',
        element: elemType,
      };

    case 'map':
      const typeArgs = (json.typeArgs ?? []).map(buildTypeFromJSON);

      if (typeArgs.length !== 2) {
        throw new Error(
          `Map type must have exactly 2 type arguments, got ${typeArgs.length}`,
        );
      }

      return {
        kind: 'map',
        name: json.name ?? 'Map',
        key: typeArgs[0],
        value: typeArgs[1],
      };
  }
}
