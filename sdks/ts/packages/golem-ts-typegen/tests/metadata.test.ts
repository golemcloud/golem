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

import { describe, expect, it } from 'vitest';
import {
  getBooleanType,
  getNumberType,
  getStringType,
  getTestListOfObjectType,
  getTestMapType,
  getObjectType,
  getTupleType,
  getUnionComplexType,
  getUnionType,
  getComplexObjectType,
  getInterfaceType,
  getClassType,
  getLiterallyObjectType,
  getRecursiveType,
  getObjectWithTypeParameter,
  getUnionWithTypeParameter,
  getMethodParams,
  getConstructorParams,
} from './util.js';

import { Type } from '@golemcloud/golem-ts-types-core';

// While golem-ts-sdk has some of these tests repeated within its context,
// these shouldn't be removed from golem-ts-typegen as it helps with easier debugging
describe('golem-ts-typegen can work correctly read types from .metadata directory', () => {
  it('track interface type', () => {
    const stringType = getStringType();
    expect(stringType.kind).toEqual('string');
  });

  it('track number type', () => {
    const numberType = getNumberType();
    expect(numberType.kind).toEqual('number');
  });

  it('track boolean type', () => {
    const booleanType = getBooleanType();
    expect(booleanType.kind).toEqual('boolean');
  });

  it('track map type', () => {
    const mapType = getTestMapType();
    expect(mapType.kind).toEqual('map');
  });

  it('track tuple type', () => {
    const tupleType = getTupleType();
    expect(tupleType.kind).toEqual('tuple');
  });

  it('track array type', () => {
    const arrayType = getTestListOfObjectType();
    expect(arrayType.kind).toEqual('array');
  });

  it('track object type', () => {
    const objectType1 = getObjectType();
    expect(objectType1.kind).toEqual('object');

    const objectType2 = getComplexObjectType();
    expect(objectType2.kind).toEqual('object');
  });

  it('track union type', () => {
    const unionType1 = getUnionComplexType();
    expect(unionType1.kind).toEqual('union');

    const unionType2 = getUnionType();
    expect(unionType2.kind).toEqual('union');
  });

  it('track interface type', () => {
    const tupleType = getInterfaceType();
    expect(tupleType.kind).toEqual('interface');
  });

  it('track class type', () => {
    const classType = getClassType();
    expect(classType.kind).toEqual('class');
  });

  it('track Object type', () => {
    const literallyObjectType = getLiterallyObjectType();
    expect(literallyObjectType.kind).toEqual('others');
  });

  it('track recursive type', () => {
    const recursiveType = getRecursiveType();
    expect(recursiveType.kind).toEqual('object');
  });

  it('track object with type parameter', () => {
    const classType = getObjectWithTypeParameter();

    const typeArgs = classType.kind === 'object' ? classType.typeParams : [];

    expect(typeArgs).toHaveLength(1);

    const tupleType = typeArgs[0];
    const tupleArgs = tupleType.kind === 'tuple' ? tupleType.elements : [];

    const args = tupleArgs.map((tupleArg) => {
      return tupleArg.kind === 'literal' ? tupleArg.literalValue : null;
    });

    expect(args).toEqual(['en', 'de']);
  });

  it('track union with type parameter', () => {
    const classType = getUnionWithTypeParameter();

    const typeArgs = classType.kind === 'union' ? classType.typeParams : [];

    expect(typeArgs).toHaveLength(1);

    const tupleType = typeArgs[0];
    const tupleArgs = tupleType.kind === 'tuple' ? tupleType.elements : [];

    const args = tupleArgs.map((tupleArg) => {
      return tupleArg.kind === 'literal' ? tupleArg.literalValue : null;
    });

    expect(args).toEqual(['en', 'de']);
  });

  it('track optional parameters with ? syntax', () => {
    const params = getMethodParams('MyAgent', 'methodWithOptionalQMark');
    const optionalParam = params.get('optional');
    expect(optionalParam).toBeDefined();
    expect(optionalParam?.optional).toBe(true);
    expect(optionalParam?.kind).toBe('union');
    if (optionalParam?.kind === 'union') {
      expect(optionalParam?.unionTypes.map((t) => t.kind)).toStrictEqual(['undefined', 'number']);
    }
  });

  it('track optional parameters with | undefined syntax', () => {
    const params = getMethodParams('MyAgent', 'methodWithOptionalUnion');
    const optionalParam = params.get('optional');
    expect(optionalParam).toBeDefined();
    expect(optionalParam?.optional).toBe(false);
    expect(optionalParam?.kind).toBe('union');
    if (optionalParam?.kind === 'union') {
      expect(optionalParam?.unionTypes.map((t) => t.kind)).toStrictEqual(['undefined', 'number']);
    }
  });

  it('correctly extracts config type', () => {
    const param = getConstructorParams('ConfigAgent')[0];
    expect(param.name).toBe('config');
    expect(param.type.optional).toBe(false);
    expect(param.type.kind).toBe('config');
    assert(param.type.kind === 'config');
    expect(param.type.properties).toHaveLength(7);
    expect(param.type.properties).toEqual(
      expect.arrayContaining([
        { path: ['foo'], secret: false, type: { kind: 'number', optional: false } },
        { path: ['bar'], secret: false, type: { kind: 'string', optional: false } },
        { path: ['secret'], secret: true, type: { kind: 'boolean', optional: false } },
        {
          path: ['nested', 'nestedSecret'],
          secret: true,
          type: { kind: 'number', optional: false },
        },
        { path: ['nested', 'a'], secret: false, type: { kind: 'boolean', optional: false } },
        {
          path: ['nested', 'b'],
          secret: false,
          type: { kind: 'array', element: { kind: 'number', optional: false }, optional: false },
        },
        { path: ['aliasedNested', 'c'], secret: false, type: { kind: 'number', optional: false } },
      ]),
    );
  });
});
