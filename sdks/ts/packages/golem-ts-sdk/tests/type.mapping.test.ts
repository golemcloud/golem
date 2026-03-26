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
  getTestInterfaceType,
  getRecordFieldsFromAnalysedType,
  getTestObjectType,
  getUnionType,
  getBooleanType,
  getNumberType,
  getStringType,
  getPromiseType,
  getUnionWithLiterals,
  getUnionWithBooleanInMiddle,
  getImportedSourceOrderedUnion,
  getObjectOrBooleanOrUndefined,
} from './testUtils';

import {
  AnalysedType,
  bool,
  f64,
  NameTypePair,
  str,
} from '../src/internal/mapping/types/analysedType';

// Interface type indirectly tests primitive types, union, list etc
describe('TypeScript Interface to AnalysedType', () => {
  const [analysed] = getTestInterfaceType();
  const recordFields = getRecordFieldsFromAnalysedType(analysed)!;

  it('Interface should be AnalysedType.Record', () => {
    expect(analysed).toBeDefined();
    expect(analysed.kind).toBe('record');
  });

  it('Primitive types within an interface', () => {
    checkPrimitiveFields(recordFields);
  });

  it('Optional fields within an interface', () => {
    checkOptionalFields(recordFields);
  });

  it('Union types within an interface', () => {
    checkUnionFields(recordFields);
    checkUnionComplexFields(recordFields);
  });

  it('Object types within an interface', () => {
    checkObjectFields(recordFields);
    checkObjectComplexFields(recordFields);
  });

  it('List type within an interface', () => {
    checkListFields(recordFields);
  });

  it('List of objects within an interface', () => {
    checkListObjectFields(recordFields);
  });

  it('Tuple type within an interface', () => {
    checkTupleFields(recordFields);
  });

  it('Tuple with object type within an interface', () => {
    checkTupleWithObjectFields(recordFields);
  });

  it('Map type within an interface', () => {
    checkMapFields(recordFields);
  });
});

describe('TypeScript primitives to AnalysedType', () => {
  it('Boolean type is converted to AnalysedType.Bool', () => {
    const [booleanType] = getBooleanType();
    expect(booleanType).toEqual(bool());
  });

  it('String type is converted to AnalysedType.String', () => {
    const [analysedType] = getStringType();
    expect(analysedType).toEqual(str());
  });

  it('Number type is converted to AnalysedType.F64', () => {
    const [analysedType] = getNumberType();
    expect(analysedType).toEqual(f64());
  });
});

// A promise<inner> type will be considered as AnalysedType<inner>,
// as TypeScript allows returning the value that the promise resolves to
describe('TypeScript Promise type to AnalysedType', () => {
  it('Promise type is converted to AnalysedType', () => {
    const [promiseType] = getPromiseType();
    expect(promiseType).toEqual(str());
  });
});

describe('TypeScript Object to AnalysedType', () => {
  it('transforms object with different properties successfully to analysed type', () => {
    const [analysedType] = getTestObjectType();

    expect(analysedType.kind).toBe('record');

    const recordFields = getRecordFieldsFromAnalysedType(analysedType)!;

    const expected: NameTypePair[] = [
      {
        name: 'a',
        typ: {
          kind: 'string',
        },
      },
      {
        name: 'b',
        typ: {
          kind: 'f64',
        },
      },
      {
        name: 'c',
        typ: {
          kind: 'bool',
        },
      },
    ];

    expect(recordFields).toEqual(expected);
  });
});

describe('TypeScript Union to AnalysedType.Variant', () => {
  it('Union is converted to Variant with the name of the type as case name', () => {
    const [enumType] = getUnionType();

    const expected: AnalysedType = {
      kind: 'variant',
      taggedTypes: [],
      value: {
        cases: [
          {
            name: 'UnionType1',
            typ: {
              kind: 'f64',
            },
          },
          {
            name: 'UnionType2',
            typ: {
              kind: 'string',
            },
          },
          {
            name: 'UnionType3',
            typ: {
              kind: 'bool',
            },
          },
          {
            name: 'UnionType4',
            typ: {
              kind: 'record',
              value: {
                fields: [
                  {
                    name: 'a',
                    typ: {
                      kind: 'string',
                    },
                  },
                  {
                    name: 'b',
                    typ: {
                      kind: 'f64',
                    },
                  },
                  {
                    name: 'c',
                    typ: {
                      kind: 'bool',
                    },
                  },
                ],
                name: 'ObjectType',
                owner: undefined,
              },
            },
          },
        ],
        name: 'UnionType',
        owner: undefined,
      },
    };

    expect(enumType).toEqual(expected);
  });
});

test('Union with literals to AnalysedType', () => {
  const [unionWithLiterals] = getUnionWithLiterals();
  const expectedAnalysedType = {
    kind: 'variant',
    taggedTypes: [],
    value: {
      name: 'UnionWithLiterals',
      owner: './testTypes',
      cases: [
        { name: 'a' },
        { name: 'b' },
        { name: 'c' },
        {
          name: 'UnionWithLiterals1',
          typ: {
            kind: 'record',
            value: {
              fields: [{ name: 'n', typ: { kind: 'f64' } }],
              name: undefined,
              owner: undefined,
            },
          },
        },
      ],
    },
  };

  expect(unionWithLiterals).toEqual(expectedAnalysedType);
});

test('Union with true|false in the middle preserves boolean case position', () => {
  const [unionWithBooleanInMiddle] = getUnionWithBooleanInMiddle();
  const expectedAnalysedType = {
    kind: 'variant',
    taggedTypes: [],
    value: {
      name: 'UnionWithBooleanInMiddle',
      owner: undefined,
      cases: [
        {
          name: 'UnionWithBooleanInMiddle1',
          typ: {
            kind: 'string',
          },
        },
        {
          name: 'UnionWithBooleanInMiddle2',
          typ: {
            kind: 'bool',
          },
        },
        {
          name: 'UnionWithBooleanInMiddle3',
          typ: {
            kind: 'record',
            value: {
              fields: [{ name: 'n', typ: { kind: 'f64' } }],
              name: undefined,
              owner: undefined,
            },
          },
        },
      ],
    },
  };

  expect(unionWithBooleanInMiddle).toEqual(expectedAnalysedType);
});

test('Imported union alias keeps source order and not fallback canonical order', () => {
  const [importedSourceOrderedUnion] = getImportedSourceOrderedUnion();

  expect(importedSourceOrderedUnion.kind).toBe('variant');
  if (importedSourceOrderedUnion.kind !== 'variant') return;

  const cases = importedSourceOrderedUnion.value.cases;
  const actualCaseKinds = cases.map((c) => c.typ?.kind ?? 'none');

  expect(actualCaseKinds).toStrictEqual(['f64', 'string', 'bool', 'record']);
  expect(actualCaseKinds).not.toStrictEqual(['string', 'f64', 'bool', 'record']);

  expect(cases.map((c) => c.name)).toStrictEqual([
    'ImportedSourceOrderedUnion1',
    'ImportedSourceOrderedUnion2',
    'ImportedSourceOrderedUnion3',
    'ImportedSourceOrderedUnion4',
  ]);
});

test('Object|boolean|undefined becomes option(variant) preserving source order', () => {
  const [objectOrBooleanOrUndefined] = getObjectOrBooleanOrUndefined();

  expect(objectOrBooleanOrUndefined.kind).toBe('option');
  if (objectOrBooleanOrUndefined.kind !== 'option') return;

  const inner = objectOrBooleanOrUndefined.value.inner;
  expect(inner.kind).toBe('variant');
  if (inner.kind !== 'variant') return;

  expect(inner.value.cases.map((c) => c.typ?.kind ?? 'none')).toStrictEqual(['record', 'bool']);
  expect(inner.value.cases.map((c) => c.name)).toStrictEqual([
    'ObjectOrBooleanOrUndefined1',
    'ObjectOrBooleanOrUndefined2',
  ]);
});

function checkPrimitiveFields(fields: any[]) {
  const expected = {
    numberProp: {
      kind: 'f64',
    },
    stringProp: {
      kind: 'string',
    },
    booleanProp: {
      kind: 'bool',
    },
    bigintProp: {
      kind: 'u64',
    },
    trueProp: {
      kind: 'bool',
    },
    falseProp: {
      kind: 'bool',
    },
  };

  for (const [name, expectedType] of Object.entries(expected)) {
    const field = fields.find((f) => f.name === name);
    expect(field).toBeDefined();
    expect(field.typ).toMatchObject(expectedType);
  }
}

function checkOptionalFields(fields: NameTypePair[]) {
  const optionalFields = fields.filter((f) => f.name.startsWith('optional'));

  optionalFields.forEach((field) => {
    expect(field.typ.kind).toBe('option');
  });
}

function checkUnionComplexFields(fields: NameTypePair[]) {
  const unionComplexFields = fields.filter((f) => f.name.startsWith('unionComplexProp'))[0];
  const expected = {
    name: 'unionComplexProp',
    typ: {
      kind: 'variant',
      taggedTypes: [],
      value: {
        name: 'UnionComplexType',
        cases: [
          { name: 'UnionComplexType1', typ: { kind: 'f64' } },
          { name: 'UnionComplexType2', typ: { kind: 'string' } },
          { name: 'UnionComplexType3', typ: { kind: 'bool' } },
          {
            name: 'UnionComplexType4',
            typ: {
              kind: 'record',
              value: {
                name: 'ObjectComplexType',
                fields: [
                  { name: 'a', typ: { kind: 'string' } },
                  { name: 'b', typ: { kind: 'f64' } },
                  { name: 'c', typ: { kind: 'bool' } },
                  {
                    name: 'd',
                    typ: {
                      kind: 'record',
                      value: {
                        name: 'ObjectType',
                        fields: [
                          { name: 'a', typ: { kind: 'string' } },
                          { name: 'b', typ: { kind: 'f64' } },
                          { name: 'c', typ: { kind: 'bool' } },
                        ],
                      },
                    },
                  },
                  {
                    name: 'e',
                    typ: {
                      kind: 'variant',
                      taggedTypes: [],
                      value: {
                        name: 'UnionType',
                        cases: [
                          { name: 'UnionType1', typ: { kind: 'f64' } },
                          { name: 'UnionType2', typ: { kind: 'string' } },
                          { name: 'UnionType3', typ: { kind: 'bool' } },
                          {
                            name: 'UnionType4',
                            typ: {
                              kind: 'record',
                              value: {
                                name: 'ObjectType',
                                fields: [
                                  { name: 'a', typ: { kind: 'string' } },
                                  { name: 'b', typ: { kind: 'f64' } },
                                  { name: 'c', typ: { kind: 'bool' } },
                                ],
                              },
                            },
                          },
                        ],
                      },
                    },
                  },
                  {
                    name: 'f',
                    typ: {
                      kind: 'list',
                      value: {
                        name: 'ListType',
                        inner: { kind: 'string' },
                      },
                    },
                  },
                  {
                    name: 'g',
                    typ: {
                      kind: 'list',
                      value: {
                        name: 'ListComplexType',
                        inner: {
                          kind: 'record',
                          value: {
                            name: 'ObjectType',
                            fields: [
                              { name: 'a', typ: { kind: 'string' } },
                              { name: 'b', typ: { kind: 'f64' } },
                              { name: 'c', typ: { kind: 'bool' } },
                            ],
                          },
                        },
                      },
                    },
                  },
                  {
                    name: 'h',
                    typ: {
                      kind: 'tuple',
                      value: {
                        name: 'TupleType',
                        items: [{ kind: 'string' }, { kind: 'f64' }, { kind: 'bool' }],
                      },
                    },
                  },
                  {
                    name: 'i',
                    typ: {
                      kind: 'tuple',
                      value: {
                        name: 'TupleComplexType',
                        items: [
                          { kind: 'string' },
                          { kind: 'f64' },
                          {
                            kind: 'record',
                            value: {
                              name: 'ObjectType',
                              fields: [
                                { name: 'a', typ: { kind: 'string' } },
                                { name: 'b', typ: { kind: 'f64' } },
                                { name: 'c', typ: { kind: 'bool' } },
                              ],
                            },
                          },
                        ],
                      },
                    },
                  },
                  {
                    name: 'j',
                    typ: {
                      kind: 'list',
                      mapType: { keyType: { kind: 'string' }, valueType: { kind: 'f64' } },
                      value: {
                        name: 'MapType',
                        inner: {
                          kind: 'tuple',
                          value: { items: [{ kind: 'string' }, { kind: 'f64' }] },
                        },
                      },
                    },
                  },
                  {
                    name: 'k',
                    typ: {
                      kind: 'record',
                      value: {
                        name: 'SimpleInterfaceType',
                        fields: [{ name: 'n', typ: { kind: 'f64' } }],
                      },
                    },
                  },
                ],
              },
            },
          },
          {
            name: 'UnionComplexType5',
            typ: {
              kind: 'variant',
              taggedTypes: [],
              value: {
                name: 'UnionType',
                cases: [
                  { name: 'UnionType1', typ: { kind: 'f64' } },
                  { name: 'UnionType2', typ: { kind: 'string' } },
                  { name: 'UnionType3', typ: { kind: 'bool' } },
                  {
                    name: 'UnionType4',
                    typ: {
                      kind: 'record',
                      value: {
                        name: 'ObjectType',
                        fields: [
                          { name: 'a', typ: { kind: 'string' } },
                          { name: 'b', typ: { kind: 'f64' } },
                          { name: 'c', typ: { kind: 'bool' } },
                        ],
                      },
                    },
                  },
                ],
              },
            },
          },
          {
            name: 'UnionComplexType6',
            typ: {
              kind: 'tuple',
              value: {
                name: 'TupleType',
                items: [{ kind: 'string' }, { kind: 'f64' }, { kind: 'bool' }],
              },
            },
          },
          {
            name: 'UnionComplexType7',
            typ: {
              kind: 'tuple',
              value: {
                name: 'TupleComplexType',
                items: [
                  { kind: 'string' },
                  { kind: 'f64' },
                  {
                    kind: 'record',
                    value: {
                      name: 'ObjectType',
                      fields: [
                        { name: 'a', typ: { kind: 'string' } },
                        { name: 'b', typ: { kind: 'f64' } },
                        { name: 'c', typ: { kind: 'bool' } },
                      ],
                    },
                  },
                ],
              },
            },
          },
          {
            name: 'UnionComplexType8',
            typ: {
              kind: 'record',
              value: {
                name: 'SimpleInterfaceType',
                fields: [{ name: 'n', typ: { kind: 'f64' } }],
              },
            },
          },
          {
            name: 'UnionComplexType9',
            typ: {
              kind: 'list',
              mapType: { keyType: { kind: 'string' }, valueType: { kind: 'f64' } },
              value: {
                name: 'MapType',
                inner: {
                  kind: 'tuple',
                  value: { items: [{ kind: 'string' }, { kind: 'f64' }] },
                },
              },
            },
          },
          {
            name: 'UnionComplexType10',
            typ: {
              kind: 'list',
              value: {
                name: 'ListType',
                inner: { kind: 'string' },
              },
            },
          },
          {
            name: 'UnionComplexType11',
            typ: {
              kind: 'list',
              value: {
                name: 'ListComplexType',
                inner: {
                  kind: 'record',
                  value: {
                    name: 'ObjectType',
                    fields: [
                      { name: 'a', typ: { kind: 'string' } },
                      { name: 'b', typ: { kind: 'f64' } },
                      { name: 'c', typ: { kind: 'bool' } },
                    ],
                  },
                },
              },
            },
          },
        ],
      },
    },
  };

  expect(unionComplexFields).toMatchObject(expected);
}

function checkUnionFields(fields: any[]) {
  const unionField = fields.find((f) => f.name === 'unionProp');

  const expected = {
    name: 'unionProp',
    typ: {
      kind: 'variant',
      taggedTypes: [],
      value: {
        name: 'UnionType',
        owner: undefined,
        cases: [
          { name: 'UnionType1', typ: { kind: 'f64' } },
          { name: 'UnionType2', typ: { kind: 'string' } },
          { name: 'UnionType3', typ: { kind: 'bool' } },
          {
            name: 'UnionType4',
            typ: {
              kind: 'record',
              value: {
                name: 'ObjectType',
                owner: undefined,
                fields: [
                  { name: 'a', typ: { kind: 'string' } },
                  { name: 'b', typ: { kind: 'f64' } },
                  { name: 'c', typ: { kind: 'bool' } },
                ],
              },
            },
          },
        ],
      },
    },
  };

  expect(unionField).toEqual(expected);
}

function checkObjectFields(fields: any[]) {
  const objectFields = fields.filter((f) => f.name.startsWith('objectProp'));
  expect(objectFields.length).toBeGreaterThan(0);

  const expected = [
    {
      name: 'a',
      typ: { kind: 'string' },
    },
    {
      name: 'b',
      typ: { kind: 'f64' },
    },
    {
      name: 'c',
      typ: { kind: 'bool' },
    },
  ];

  objectFields.forEach((field) => {
    expect(field.typ.kind).toBe('record');
    expect(field.typ.value.fields).toEqual(expected);
  });
}

function checkListFields(fields: any[]) {
  const listFields = fields.filter((f) => f.name.startsWith('listProp'));
  expect(listFields.length).toBeGreaterThan(0);

  listFields.forEach((field) => {
    expect(field.typ.kind).toBe('list');
    expect(field.typ.value.inner.kind).toBe('string'); // Assuming the inner type is string
  });
}

function checkListObjectFields(fields: any[]) {
  const listObjectFields = fields.filter((f) => f.name.startsWith('listObjectProp'));
  expect(listObjectFields.length).toBeGreaterThan(0);

  listObjectFields.forEach((field) => {
    expect(field.typ.kind).toBe('list');
    expect(field.typ.value.inner.kind).toBe('record');
    const innerFields = getRecordFieldsFromAnalysedType(field.typ.value.inner)!;
    expect(innerFields.length).toBe(3); // Assuming 3 fields in the object type
  });
}

function checkTupleFields(fields: any[]) {
  const tupleFields = fields.filter((f) => f.name.startsWith('tupleProp'));

  tupleFields.forEach((field) => {
    expect(field.typ.kind).toBe('tuple');
    if (field.typ.kind == 'tuple') {
      const expected: AnalysedType[] = [
        {
          kind: 'string',
        },
        { kind: 'f64' },
        { kind: 'bool' },
      ];
      expect(field.typ.value.items).toEqual(expected);
    }
  });
}

function checkTupleWithObjectFields(fields: any[]) {
  const tupleObjectFields = fields.filter((f) => f.name.startsWith('tupleObjectProp'));
  expect(tupleObjectFields.length).toBeGreaterThan(0);

  tupleObjectFields.forEach((field) => {
    expect(field.typ.kind).toBe('tuple');
    if (field.typ.kind == 'tuple') {
      const expected: AnalysedType[] = [
        {
          kind: 'string',
        },
        { kind: 'f64' },
        {
          kind: 'record',
          value: {
            fields: [
              {
                name: 'a',
                typ: {
                  kind: 'string',
                },
              },
              {
                name: 'b',
                typ: {
                  kind: 'f64',
                },
              },
              {
                name: 'c',
                typ: {
                  kind: 'bool',
                },
              },
            ],
            name: 'ObjectType',
            owner: undefined,
          },
        },
      ];
      expect(field.typ.value.items).toEqual(expected);
    }
  });
}

function checkMapFields(fields: any[]) {
  const mapFields = fields.filter((f) => f.name.startsWith('mapProp'));
  expect(mapFields.length).toBeGreaterThan(0);

  // list of tuples, where each tuple is a key-value pair
  mapFields.forEach((field) => {
    expect(field.typ.kind).toBe('list');
    if (field.typ.kind == 'list') {
      expect(field.typ.value.inner.kind).toBe('tuple');
      const inner = field.typ.value.inner;
      expect(inner.value.items.length).toBe(2);
      expect(inner.value.items[0].kind).toBe('string');
      expect(inner.value.items[1].kind).toBe('f64');
    }
  });
}

function checkObjectComplexFields(fields: any[]) {
  const objectFields = fields.filter((f) => f.name.startsWith('objectComplexProp'));
  expect(objectFields.length).toBeGreaterThan(0);

  const expected = [
    {
      name: 'a',
      typ: { kind: 'string' },
    },
    {
      name: 'b',
      typ: { kind: 'f64' },
    },
    {
      name: 'c',
      typ: { kind: 'bool' },
    },
    {
      name: 'd',
      typ: {
        kind: 'record',
        value: {
          fields: [
            {
              name: 'a',
              typ: {
                kind: 'string',
              },
            },
            {
              name: 'b',
              typ: {
                kind: 'f64',
              },
            },
            {
              name: 'c',
              typ: {
                kind: 'bool',
              },
            },
          ],
          name: 'ObjectType',
          owner: undefined,
        },
      },
    },
    {
      name: 'e',
      typ: {
        kind: 'variant',
        taggedTypes: [],
        value: {
          cases: [
            {
              name: 'UnionType1',
              typ: {
                kind: 'f64',
              },
            },
            {
              name: 'UnionType2',
              typ: {
                kind: 'string',
              },
            },
            {
              name: 'UnionType3',
              typ: {
                kind: 'bool',
              },
            },
            {
              name: 'UnionType4',
              typ: {
                kind: 'record',
                value: {
                  fields: [
                    {
                      name: 'a',
                      typ: {
                        kind: 'string',
                      },
                    },
                    {
                      name: 'b',
                      typ: {
                        kind: 'f64',
                      },
                    },
                    {
                      name: 'c',
                      typ: {
                        kind: 'bool',
                      },
                    },
                  ],
                  name: 'ObjectType',
                  owner: undefined,
                },
              },
            },
          ],
          name: 'UnionType',
          owner: undefined,
        },
      },
    },
    {
      name: 'f',
      typ: {
        kind: 'list',
        typedArray: undefined,
        mapType: undefined,
        value: {
          inner: {
            kind: 'string',
          },
          name: 'ListType',
          owner: undefined,
        },
      },
    },
    {
      name: 'g',
      typ: {
        kind: 'list',
        mapType: undefined,
        typedArray: undefined,
        value: {
          inner: {
            kind: 'record',
            value: {
              fields: [
                {
                  name: 'a',
                  typ: {
                    kind: 'string',
                  },
                },
                {
                  name: 'b',
                  typ: {
                    kind: 'f64',
                  },
                },
                {
                  name: 'c',
                  typ: {
                    kind: 'bool',
                  },
                },
              ],
              name: 'ObjectType',
              owner: undefined,
            },
          },
          name: 'ListComplexType',
          owner: undefined,
        },
      },
    },
    {
      name: 'h',
      typ: {
        kind: 'tuple',
        emptyType: undefined,
        value: {
          items: [
            {
              kind: 'string',
            },
            { kind: 'f64' },
            { kind: 'bool' },
          ],
          name: 'TupleType',
          owner: undefined,
        },
      },
    },
    {
      name: 'i',
      typ: {
        kind: 'tuple',
        emptyType: undefined,
        value: {
          items: [
            {
              kind: 'string',
            },
            { kind: 'f64' },
            {
              kind: 'record',
              value: {
                fields: [
                  {
                    name: 'a',
                    typ: {
                      kind: 'string',
                    },
                  },
                  {
                    name: 'b',
                    typ: {
                      kind: 'f64',
                    },
                  },
                  {
                    name: 'c',
                    typ: {
                      kind: 'bool',
                    },
                  },
                ],
                name: 'ObjectType',
                owner: undefined,
              },
            },
          ],
          name: 'TupleComplexType',
          owner: undefined,
        },
      },
    },
    {
      name: 'j',
      typ: {
        kind: 'list',
        mapType: {
          keyType: { kind: 'string' },
          valueType: { kind: 'f64' },
        },
        typedArray: undefined,
        value: {
          inner: {
            kind: 'tuple',
            emptyType: undefined,
            value: {
              items: [
                {
                  kind: 'string',
                },
                {
                  kind: 'f64',
                },
              ],
              name: undefined,
              owner: undefined,
            },
          },
          name: 'MapType',
          owner: undefined,
        },
      },
    },
    {
      name: 'k',
      typ: {
        kind: 'record',
        value: {
          fields: [
            {
              name: 'n',
              typ: {
                kind: 'f64',
              },
            },
          ],
          name: 'SimpleInterfaceType',
          owner: undefined,
        },
      },
    },
  ];

  objectFields.forEach((field) => {
    expect(field.typ.kind).toBe('record');
    expect(field.typ.value.fields).toEqual(expected);
  });
}
