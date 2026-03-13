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

import { describe, it, expect } from 'vitest';
import {
  deserializeDataValue,
  serializeToDataValue,
  ParameterDetail,
} from '../src/internal/mapping/values/dataValue';
import { TypeInfoInternal } from '../src/internal/typeInfoInternal';
import {
  u32,
  str,
  bool,
  option,
  tuple,
  record,
  field,
} from '../src/internal/mapping/types/analysedType';
import { Type } from '@golemcloud/golem-ts-types-core';
import { ElementValue, DataValue } from 'golem:core/types@1.5.0';

// Minimal dummy Type.Type for test purposes (the tsType field is only used
// for schema derivation / language-code extraction which we don't exercise here).
const dummyTsType: Type.Type = { kind: 'string' } as any;

function makeAnalysedTypeInfo(analysedType: ReturnType<typeof u32>): TypeInfoInternal {
  return {
    tag: 'analysed',
    val: analysedType,
    witType: { nodes: [] } as any,
    tsType: dummyTsType,
  };
}

const dummyPrincipal = { tag: 'anonymous' } as any;

/** Assert that a DataValue is a tuple and return its elements */
function expectTuple(dv: DataValue): ElementValue[] {
  expect(dv.tag).toBe('tuple');
  if (dv.tag !== 'tuple') throw new Error('Expected tuple DataValue');
  return dv.val;
}

/** Extract the first element from a serialized tuple DataValue */
function getFirstElement(dv: DataValue): ElementValue {
  return expectTuple(dv)[0];
}

// ─── Component-model element wrapping/unwrapping ───────────────────────────

describe('DataValue: component-model round-trip', () => {
  it('serializes a simple u32 into a tuple with a single component-model element', () => {
    const typeInfo = makeAnalysedTypeInfo(u32());
    const dv = serializeToDataValue(42, typeInfo);
    const elems = expectTuple(dv);
    expect(elems).toHaveLength(1);
    expect(elems[0].tag).toBe('component-model');
  });

  it('deserializes a component-model DataValue back to the original value', () => {
    const typeInfo = makeAnalysedTypeInfo(u32());
    const serialized = serializeToDataValue(42, typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'x', type: typeInfo }];
    const values = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(values).toEqual([42]);
  });

  it('round-trips a string value', () => {
    const typeInfo = makeAnalysedTypeInfo(str());
    const serialized = serializeToDataValue('hello', typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'msg', type: typeInfo }];
    const deserialized = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(deserialized).toEqual(['hello']);
  });

  it('round-trips a boolean value', () => {
    const typeInfo = makeAnalysedTypeInfo(bool());
    const serialized = serializeToDataValue(true, typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'flag', type: typeInfo }];
    const deserialized = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(deserialized).toEqual([true]);
  });

  it('round-trips a record value', () => {
    const recType = record(undefined, [field('name', str()), field('age', u32())]);
    const typeInfo = makeAnalysedTypeInfo(recType);
    const input = { name: 'Alice', age: 30 };
    const serialized = serializeToDataValue(input, typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'person', type: typeInfo }];
    const deserialized = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(deserialized).toEqual([input]);
  });

  it('round-trips an option value (Some)', () => {
    const optType = option(undefined, 'null', u32());
    const typeInfo = makeAnalysedTypeInfo(optType);
    const serialized = serializeToDataValue(42, typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'opt', type: typeInfo }];
    const deserialized = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(deserialized).toEqual([42]);
  });

  it('round-trips an option value (None → null)', () => {
    const optType = option(undefined, 'null', u32());
    const typeInfo = makeAnalysedTypeInfo(optType);
    const serialized = serializeToDataValue(null, typeInfo);

    const paramDetails: ParameterDetail[] = [{ name: 'opt', type: typeInfo }];
    const deserialized = deserializeDataValue(serialized, paramDetails, dummyPrincipal);
    expect(deserialized).toEqual([null]);
  });
});

// ─── Empty / void type handling ────────────────────────────────────────────

describe('DataValue: empty/void type', () => {
  it('serializes void return type as empty tuple', () => {
    const voidType = tuple(undefined, 'void', []);
    const typeInfo = makeAnalysedTypeInfo(voidType);
    const dv = serializeToDataValue(undefined, typeInfo);
    expect(dv.tag).toBe('tuple');
    expect(dv.val).toHaveLength(0);
  });
});

// ─── Principal auto-injection ──────────────────────────────────────────────

describe('DataValue: principal auto-injection', () => {
  it('auto-injects principal when parameter type is principal', () => {
    const principalType: TypeInfoInternal = {
      tag: 'principal',
      tsType: dummyTsType,
    };

    // DataValue with no elements (principal is auto-injected, not from input)
    const dataValue = { tag: 'tuple' as const, val: [] };

    const paramDetails: ParameterDetail[] = [{ name: 'caller', type: principalType }];
    const values = deserializeDataValue(dataValue, paramDetails, dummyPrincipal);
    expect(values).toEqual([dummyPrincipal]);
  });

  it('auto-injects principal even when mixed with regular params', () => {
    const u32TypeInfo = makeAnalysedTypeInfo(u32());
    const principalType: TypeInfoInternal = {
      tag: 'principal',
      tsType: dummyTsType,
    };
    const strTypeInfo = makeAnalysedTypeInfo(str());

    // Serialize a DataValue with just the non-principal params
    const u32Serialized = serializeToDataValue(42, u32TypeInfo);
    const strSerialized = serializeToDataValue('hello', strTypeInfo);

    // Build a combined tuple DataValue with the two component-model elements
    const combinedDataValue: DataValue = {
      tag: 'tuple',
      val: [getFirstElement(u32Serialized), getFirstElement(strSerialized)],
    };

    const paramDetails: ParameterDetail[] = [
      { name: 'count', type: u32TypeInfo },
      { name: 'caller', type: principalType },
      { name: 'message', type: strTypeInfo },
    ];

    const result = deserializeDataValue(combinedDataValue, paramDetails, dummyPrincipal);
    expect(result).toEqual([42, dummyPrincipal, 'hello']);
  });
});

// ─── Unstructured-text ─────────────────────────────────────────────────────

describe('DataValue: unstructured-text', () => {
  it('serializes unstructured text (inline) to DataValue', () => {
    const textTypeInfo: TypeInfoInternal = {
      tag: 'unstructured-text',
      val: { restrictions: [] },
      tsType: { kind: 'object', properties: [] } as any,
    };

    const input = { tag: 'inline', val: 'hello world' };
    const dv = serializeToDataValue(input, textTypeInfo);
    const elems = expectTuple(dv);
    expect(elems).toHaveLength(1);
    expect(elems[0].tag).toBe('unstructured-text');
  });

  it('serializes unstructured text (url) to DataValue', () => {
    const textTypeInfo: TypeInfoInternal = {
      tag: 'unstructured-text',
      val: { restrictions: [] },
      tsType: { kind: 'object', properties: [] } as any,
    };

    const input = { tag: 'url', val: 'https://example.com/text.txt' };
    const dv = serializeToDataValue(input, textTypeInfo);
    const elems = expectTuple(dv);
    expect(elems[0].tag).toBe('unstructured-text');
    const elem = elems[0] as ElementValue & { tag: 'unstructured-text' };
    expect(elem.val.tag).toBe('url');
  });
});

// ─── Unstructured-binary ───────────────────────────────────────────────────

describe('DataValue: unstructured-binary', () => {
  it('serializes unstructured binary (url) to DataValue', () => {
    const binaryTypeInfo: TypeInfoInternal = {
      tag: 'unstructured-binary',
      val: { restrictions: [] },
      tsType: { kind: 'object', properties: [] } as any,
    };

    const input = { tag: 'url', val: 'https://example.com/image.png' };
    const dv = serializeToDataValue(input, binaryTypeInfo);
    const elems = expectTuple(dv);
    expect(elems).toHaveLength(1);
    expect(elems[0].tag).toBe('unstructured-binary');
    const elem = elems[0] as ElementValue & { tag: 'unstructured-binary' };
    expect(elem.val.tag).toBe('url');
  });

  it('serializes unstructured binary (inline) to DataValue', () => {
    const binaryTypeInfo: TypeInfoInternal = {
      tag: 'unstructured-binary',
      val: { restrictions: [] },
      tsType: { kind: 'object', properties: [] } as any,
    };

    const input = {
      tag: 'inline',
      val: new Uint8Array([1, 2, 3]),
      mimeType: 'application/octet-stream',
    };
    const dv = serializeToDataValue(input, binaryTypeInfo);
    const elems = expectTuple(dv);
    expect(elems[0].tag).toBe('unstructured-binary');
    const elem = elems[0] as ElementValue & { tag: 'unstructured-binary' };
    expect(elem.val.tag).toBe('inline');
  });
});

// ─── Error paths ───────────────────────────────────────────────────────────

describe('DataValue: error paths', () => {
  it('throws when serializing Principal type', () => {
    const principalType: TypeInfoInternal = {
      tag: 'principal',
      tsType: dummyTsType,
    };
    expect(() => serializeToDataValue({}, principalType)).toThrow();
  });

  it('throws when serializing Config type', () => {
    const configType: TypeInfoInternal = {
      tag: 'config',
      tsType: dummyTsType,
    };
    expect(() => serializeToDataValue({}, configType)).toThrow();
  });
});

// ─── Mixed parameter lists ─────────────────────────────────────────────────

describe('DataValue: mixed parameter lists', () => {
  it('deserializes multiple component-model parameters', () => {
    const u32Info = makeAnalysedTypeInfo(u32());
    const strInfo = makeAnalysedTypeInfo(str());
    const boolInfo = makeAnalysedTypeInfo(bool());

    const u32Elem = serializeToDataValue(42, u32Info);
    const strElem = serializeToDataValue('hello', strInfo);
    const boolElem = serializeToDataValue(true, boolInfo);

    const combined: DataValue = {
      tag: 'tuple',
      val: [getFirstElement(u32Elem), getFirstElement(strElem), getFirstElement(boolElem)],
    };

    const paramDetails: ParameterDetail[] = [
      { name: 'count', type: u32Info },
      { name: 'message', type: strInfo },
      { name: 'flag', type: boolInfo },
    ];

    const result = deserializeDataValue(combined, paramDetails, dummyPrincipal);
    expect(result).toEqual([42, 'hello', true]);
  });

  it('handles optional question-mark parameters missing from DataValue', () => {
    const u32Info = makeAnalysedTypeInfo(u32());

    // Create an optional type info that will match isOptionalWithQuestionMark
    const optionalInfo: TypeInfoInternal = {
      tag: 'analysed',
      val: option(undefined, 'question-mark', u32()),
      witType: { nodes: [] } as any,
      tsType: {
        kind: 'union',
        unionTypes: [{ kind: 'number' } as any, { kind: 'undefined' } as any],
      } as any,
    };

    // Only provide one element in DataValue but declare two params
    const u32Elem = serializeToDataValue(42, u32Info);
    const dataValue: DataValue = {
      tag: 'tuple',
      val: [getFirstElement(u32Elem)],
    };

    const paramDetails: ParameterDetail[] = [
      { name: 'required', type: u32Info },
      { name: 'optional', type: optionalInfo },
    ];

    const result = deserializeDataValue(dataValue, paramDetails, dummyPrincipal);
    expect(result).toEqual([42, undefined]);
  });
});
