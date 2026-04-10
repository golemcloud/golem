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

import { Symbol } from './symbol';
import { Node } from './node';
import { LiteTypeJSON } from './type-json';
import { Type } from './type-lite';

export function buildTypeFromJSON(json: LiteTypeJSON): Type {
  switch (json.kind) {
    case 'others':
      return {
        kind: 'others',
        name: json.name,
        owner: json.owner,
        optional: json.optional,
        recursive: json.recursive,
      };

    case 'unresolved-type':
      return {
        kind: 'unresolved-type',
        name: json.name,
        owner: json.owner,
        optional: json.optional,
        text: json.text,
        error: json.error,
      };

    case 'boolean':
      return { kind: 'boolean', name: json.name, owner: json.owner, optional: json.optional };

    case 'number':
      return { kind: 'number', name: json.name, owner: json.owner, optional: json.optional };

    case 'string':
      return { kind: 'string', name: json.name, owner: json.owner, optional: json.optional };

    case 'bigint':
      return { kind: 'bigint', name: json.name, owner: json.owner, optional: json.optional };

    case 'null':
      return { kind: 'null', name: json.name, owner: json.owner, optional: json.optional };

    case 'undefined':
      return { kind: 'undefined', name: json.name, owner: json.owner, optional: json.optional };

    case 'void':
      return { kind: 'void', name: json.name, owner: json.owner, optional: json.optional };

    case 'array': {
      const elem = buildTypeFromJSON(json.element);
      return {
        kind: 'array',
        name: json.name,
        owner: json.owner,
        element: elem,
        optional: json.optional,
      };
    }

    case 'tuple': {
      const elems = json.elements.map(buildTypeFromJSON);
      return {
        kind: 'tuple',
        name: json.name,
        owner: json.owner,
        elements: elems,
        optional: json.optional,
      };
    }

    case 'union': {
      const unionElems = json.types.map(buildTypeFromJSON);
      const unionTypeParams = json.typeParams.map(buildTypeFromJSON);
      return {
        kind: 'union',
        name: json.name,
        owner: json.owner,
        unionTypes: unionElems,
        optional: json.optional,
        typeParams: unionTypeParams,
        originalTypeName: json.originalTypeName,
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
        owner: json.owner,
        properties: props,
        optional: json.optional,
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
        owner: json.owner,
        properties: props,
        optional: json.optional,
        typeParams: json.typeParams.map((arg) => buildTypeFromJSON(arg)),
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
        owner: json.owner,
        properties: props,
        optional: json.optional,
        typeParams: json.typeParams.map((arg) => buildTypeFromJSON(arg)),
      };
    }

    case 'literal':
      return {
        kind: 'literal',
        name: json.name,
        owner: json.owner,
        literalValue: json.literalValue,
        optional: json.optional,
      };

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
        owner: json.owner,
        aliasSymbol: aliasSym,
        optional: json.optional,
      };
    }

    case 'promise':
      const elemType = buildTypeFromJSON(json.element);
      return {
        kind: 'promise',
        name: json.name ?? 'Promise',
        owner: json.owner,
        element: elemType,
        optional: json.optional,
      };

    case 'map':
      const typeArgs = (json.typeArgs ?? []).map(buildTypeFromJSON);

      if (typeArgs.length !== 2) {
        throw new Error(`Map type must have exactly 2 type arguments, got ${typeArgs.length}`);
      }

      return {
        kind: 'map',
        name: json.name,
        owner: json.owner,
        key: typeArgs[0],
        value: typeArgs[1],
        optional: json.optional,
      };

    case 'config': {
      const properties = json.properties.map(({ path, secret, type }) => ({
        path,
        secret,
        type: buildTypeFromJSON(type),
      }));
      return {
        kind: 'config',
        name: json.name,
        owner: json.owner,
        properties,
        optional: json.optional,
      };
    }

    case 'quota-token':
      return { kind: 'quota-token', name: json.name, owner: json.owner, optional: json.optional };
  }
}
