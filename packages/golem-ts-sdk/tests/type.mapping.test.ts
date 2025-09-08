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
  getUnionOfLiterals,
} from './testUtils';

import * as AnalysedType from '../src/internal/mapping/types/AnalysedType';

import * as Either from '../src/newTypes/either';
import { NameTypePair } from '../src/internal/mapping/types/AnalysedType';

// Interface type indirectly tests primitive types, union, list etc
describe('TypeScript Interface to AnalysedType', () => {
  const interfaceType = getTestInterfaceType();
  const analysed = Either.getOrThrowWith(
    AnalysedType.fromTsType(interfaceType),
    (err) => {
      throw new Error(`Failed to construct analysed type: ${err}`);
    },
  );

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

  it('Union types (aliased) within an interface', () => {
    ///checkUnionFields(recordFields);
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
    const booleanType = getBooleanType();
    const result = AnalysedType.fromTsType(booleanType);
    expect(Either.getRight(result)).toEqual(AnalysedType.bool());
  });

  it('String type is converted to AnalysedType.String', () => {
    const stringType = getStringType();
    const result = AnalysedType.fromTsType(stringType);
    expect(Either.getRight(result)).toEqual(AnalysedType.str());
  });

  it('Number type is converted to AnalysedType.S32', () => {
    const numberType = getNumberType();
    const result = AnalysedType.fromTsType(numberType);
    expect(Either.getRight(result)).toEqual(AnalysedType.s32());
  });
});

// A promise<inner> type will be considered as AnalysedType<inner>,
// as TypeScript allows returning the value that the promise resolves to
describe('TypeScript Promise type to AnalysedType', () => {
  it('Promise type is converted to AnalysedType', () => {
    const promiseType = getPromiseType();
    const result = Either.getOrElse(
      AnalysedType.fromTsType(promiseType),
      (error) => {
        throw new Error(`Failed to construct analysed type: ${error}`);
      },
    );

    expect(result).toEqual(AnalysedType.str());
  });
});

describe('TypeScript Object to AnalysedType', () => {
  it('transforms object with different properties successfully to analysed type', () => {
    const interfaceType = getTestObjectType();
    const analysed = Either.getOrThrow(AnalysedType.fromTsType(interfaceType));

    expect(analysed).toBeDefined();
    expect(analysed.kind).toBe('record');

    const recordFields = getRecordFieldsFromAnalysedType(analysed)!;

    const expected: NameTypePair[] = [
      {
        name: 'a',
        typ: { kind: 'string' },
      },
      {
        name: 'b',
        typ: { kind: 's32' },
      },
      {
        name: 'c',
        typ: { kind: 'bool' },
      },
    ];

    expect(recordFields).toEqual(expected);
  });
});

describe('TypeScript Union to AnalysedType.Variant', () => {
  it('Union is converted to Variant with the name of the type as case name', () => {
    const enumType = getUnionType();
    const analysedType = Either.getOrElse(
      AnalysedType.fromTsType(enumType),
      (error) => {
        throw new Error(`Failed to construct analysed type: ${error}`);
      },
    );

    const expected: AnalysedType.AnalysedType = {
      kind: 'variant',
      value: {
        cases: [
          { name: 'type-first', typ: { kind: 'string' } },
          { name: 'type-second', typ: { kind: 's32' } },
          { name: 'type-third', typ: { kind: 'bool' } },
          {
            name: 'type-fourth',
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
                      kind: 's32',
                    },
                  },
                  {
                    name: 'c',
                    typ: {
                      kind: 'bool',
                    },
                  },
                ],
                name: 'object-type',
                owner: undefined,
              },
            },
          },
        ],
        name: 'union-type',
        owner: undefined,
      },
    };

    expect(analysedType).toEqual(expected);
  });
});

test('Union of literals to AnalysedType', () => {
  const unstructuredTextType = getUnionOfLiterals();

  const analysedType = Either.getOrThrow(
    AnalysedType.fromTsType(unstructuredTextType),
  );

  const expectedAnalysedType: AnalysedType.AnalysedType = {
    kind: 'variant',
    value: {
      cases: [
        {
          name: 'null-type',
          typ: {
            kind: 'tuple',
            value: {
              items: [],
              name: 'null-type',
              owner: undefined,
            },
          },
        },
        {
          name: 'type-first',
          typ: {
            kind: 'bool',
          },
        },
        {
          name: 'a',
        },
        {
          name: 'b',
        },
        {
          name: 'c',
        },
      ],
      name: 'union-of-literals',
      owner: undefined,
    },
  };

  expect(analysedType).toEqual(expectedAnalysedType);
});

function checkPrimitiveFields(fields: any[]) {
  const expected = {
    numberProp: { kind: 's32' },
    stringProp: { kind: 'string' },
    booleanProp: { kind: 'bool' },
    bigintProp: { kind: 'u64' },
    nullProp: { kind: 'tuple', value: { items: [] } },
    trueProp: { kind: 'bool' },
    falseProp: { kind: 'bool' },
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
  const unionComplexFields = fields.filter((f) =>
    f.name.startsWith('unionComplexProp'),
  )[0];

  const expected: NameTypePair = {
    name: 'unionComplexProp',
    typ: {
      kind: 'variant',
      value: {
        cases: [
          { name: 'type-first', typ: { kind: 'string' } },
          { name: 'type-second', typ: { kind: 's32' } },
          { name: 'type-third', typ: { kind: 'bool' } },
          {
            name: 'type-fourth',
            typ: {
              kind: 'record',
              value: {
                fields: [{ name: 'n', typ: { kind: 's32' } }],
                name: 'simple-interface-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-fifth',
            typ: {
              kind: 'record',
              value: {
                fields: [
                  { name: 'a', typ: { kind: 'string' } },
                  { name: 'b', typ: { kind: 's32' } },
                  { name: 'c', typ: { kind: 'bool' } },
                ],
                name: 'object-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-sixth',
            typ: {
              kind: 'record',
              value: {
                fields: [
                  { name: 'a', typ: { kind: 'string' } },
                  { name: 'b', typ: { kind: 's32' } },
                  { name: 'c', typ: { kind: 'bool' } },
                  {
                    name: 'd',
                    typ: {
                      kind: 'record',
                      value: {
                        fields: [
                          { name: 'a', typ: { kind: 'string' } },
                          { name: 'b', typ: { kind: 's32' } },
                          { name: 'c', typ: { kind: 'bool' } },
                        ],
                        name: 'object-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'e',
                    typ: {
                      kind: 'variant',
                      value: {
                        cases: [
                          { name: 'type-first', typ: { kind: 'string' } },
                          { name: 'type-second', typ: { kind: 's32' } },
                          { name: 'type-third', typ: { kind: 'bool' } },
                          {
                            name: 'type-fourth',
                            typ: {
                              kind: 'record',
                              value: {
                                fields: [
                                  { name: 'a', typ: { kind: 'string' } },
                                  { name: 'b', typ: { kind: 's32' } },
                                  { name: 'c', typ: { kind: 'bool' } },
                                ],
                                name: 'object-type',
                                owner: undefined,
                              },
                            },
                          },
                        ],
                        name: 'union-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'f',
                    typ: {
                      kind: 'list',
                      value: {
                        inner: { kind: 'string' },
                        name: 'list-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'g',
                    typ: {
                      kind: 'list',
                      value: {
                        inner: {
                          kind: 'record',
                          value: {
                            fields: [
                              { name: 'a', typ: { kind: 'string' } },
                              { name: 'b', typ: { kind: 's32' } },
                              { name: 'c', typ: { kind: 'bool' } },
                            ],
                            name: 'object-type',
                            owner: undefined,
                          },
                        },
                        name: 'list-complex-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'h',
                    typ: {
                      kind: 'tuple',
                      value: {
                        items: [
                          { kind: 'string' },
                          { kind: 's32' },
                          { kind: 'bool' },
                        ],
                        name: 'tuple-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'i',
                    typ: {
                      kind: 'tuple',
                      value: {
                        items: [
                          { kind: 'string' },
                          { kind: 's32' },
                          {
                            kind: 'record',
                            value: {
                              fields: [
                                { name: 'a', typ: { kind: 'string' } },
                                { name: 'b', typ: { kind: 's32' } },
                                { name: 'c', typ: { kind: 'bool' } },
                              ],
                              name: 'object-type',
                              owner: undefined,
                            },
                          },
                        ],
                        name: 'tuple-complex-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'j',
                    typ: {
                      kind: 'list',
                      value: {
                        inner: {
                          kind: 'tuple',
                          value: {
                            items: [{ kind: 'string' }, { kind: 's32' }],
                            name: undefined,
                            owner: undefined,
                          },
                        },
                        name: 'map-type',
                        owner: undefined,
                      },
                    },
                  },
                  {
                    name: 'k',
                    typ: {
                      kind: 'record',
                      value: {
                        fields: [{ name: 'n', typ: { kind: 's32' } }],
                        name: 'simple-interface-type',
                        owner: undefined,
                      },
                    },
                  },
                ],
                name: 'object-complex-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-seventh',
            typ: {
              kind: 'tuple',
              value: {
                items: [{ kind: 'string' }, { kind: 's32' }, { kind: 'bool' }],
                name: 'tuple-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-eighth',
            typ: {
              kind: 'tuple',
              value: {
                items: [
                  { kind: 'string' },
                  { kind: 's32' },
                  {
                    kind: 'record',
                    value: {
                      fields: [
                        { name: 'a', typ: { kind: 'string' } },
                        { name: 'b', typ: { kind: 's32' } },
                        { name: 'c', typ: { kind: 'bool' } },
                      ],
                      name: 'object-type',
                      owner: undefined,
                    },
                  },
                ],
                name: 'tuple-complex-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-ninth',
            typ: {
              kind: 'list',
              value: {
                inner: {
                  kind: 'tuple',
                  value: {
                    items: [
                      {
                        kind: 'string',
                      },
                      {
                        kind: 's32',
                      },
                    ],
                    name: undefined,
                    owner: undefined,
                  },
                },
                name: 'map-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-tenth',
            typ: {
              kind: 'list',
              value: {
                inner: {
                  kind: 'string',
                },
                name: 'list-type',
                owner: undefined,
              },
            },
          },
          {
            name: 'type-eleventh',
            typ: {
              kind: 'list',
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
                          kind: 's32',
                        },
                      },
                      {
                        name: 'c',
                        typ: {
                          kind: 'bool',
                        },
                      },
                    ],
                    name: 'object-type',
                    owner: undefined,
                  },
                },
                name: 'list-complex-type',
                owner: undefined,
              },
            },
          },
        ],
        name: 'union-complex-type',
        owner: undefined,
      },
    },
  };

  expect(unionComplexFields).toEqual(expected);
}

function checkUnionFields(fields: any[]) {
  const unionFields = fields.filter((f) => f.name.startsWith('unionProp'));
  expect(unionFields.length).toBeGreaterThan(0);

  const expectedCases: NameTypePair[] = [
    { name: 'type-first', typ: { kind: 'string' } },
    { name: 'type-second', typ: { kind: 's32' } },
    { name: 'type-third', typ: { kind: 'bool' } },
    {
      name: 'type-fourth',
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
                kind: 's32',
              },
            },
            {
              name: 'c',
              typ: {
                kind: 'bool',
              },
            },
          ],
          name: undefined,
          owner: undefined,
        },
      },
    },
  ];

  // This implies wit value will be a variant with these cases
  unionFields.forEach((field) => {
    expect(field.typ.kind).toBe('variant');
    expect(field.typ.value.cases).toEqual(expectedCases);
  });
}

function checkObjectFields(fields: any[]) {
  const objectFields = fields.filter((f) => f.name.startsWith('objectProp'));
  expect(objectFields.length).toBeGreaterThan(0);

  const expected = [
    { name: 'a', typ: { kind: 'string' } },
    { name: 'b', typ: { kind: 's32' } },
    { name: 'c', typ: { kind: 'bool' } },
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
  const listObjectFields = fields.filter((f) =>
    f.name.startsWith('listObjectProp'),
  );
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
      const expected: AnalysedType.AnalysedType[] = [
        { kind: 'string' },
        { kind: 's32' },
        { kind: 'bool' },
      ];
      expect(field.typ.value.items).toEqual(expected);
    }
  });
}

function checkTupleWithObjectFields(fields: any[]) {
  const tupleObjectFields = fields.filter((f) =>
    f.name.startsWith('tupleObjectProp'),
  );
  expect(tupleObjectFields.length).toBeGreaterThan(0);

  tupleObjectFields.forEach((field) => {
    expect(field.typ.kind).toBe('tuple');
    if (field.typ.kind == 'tuple') {
      const expected: AnalysedType.AnalysedType[] = [
        { kind: 'string' },
        { kind: 's32' },
        {
          kind: 'record',
          value: {
            fields: [
              { name: 'a', typ: { kind: 'string' } },
              { name: 'b', typ: { kind: 's32' } },
              { name: 'c', typ: { kind: 'bool' } },
            ],
            name: 'object-type',
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
      expect(inner.value.items[1].kind).toBe('s32');
    }
  });
}

function checkObjectComplexFields(fields: any[]) {
  const objectFields = fields.filter((f) =>
    f.name.startsWith('objectComplexProp'),
  );
  expect(objectFields.length).toBeGreaterThan(0);

  const expected = [
    { name: 'a', typ: { kind: 'string' } },
    { name: 'b', typ: { kind: 's32' } },
    { name: 'c', typ: { kind: 'bool' } },
    {
      name: 'd',
      typ: {
        kind: 'record',
        value: {
          fields: [
            { name: 'a', typ: { kind: 'string' } },
            { name: 'b', typ: { kind: 's32' } },
            { name: 'c', typ: { kind: 'bool' } },
          ],
          name: 'object-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'e',
      typ: {
        kind: 'variant',
        value: {
          cases: [
            { name: 'type-first', typ: { kind: 'string' } },
            { name: 'type-second', typ: { kind: 's32' } },
            { name: 'type-third', typ: { kind: 'bool' } },
            {
              name: 'type-fourth',
              typ: {
                kind: 'record',
                value: {
                  fields: [
                    { name: 'a', typ: { kind: 'string' } },
                    { name: 'b', typ: { kind: 's32' } },
                    { name: 'c', typ: { kind: 'bool' } },
                  ],
                  name: 'object-type',
                  owner: undefined,
                },
              },
            },
          ],
          name: 'union-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'f',
      typ: {
        kind: 'list',
        value: {
          inner: { kind: 'string' },
          name: 'list-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'g',
      typ: {
        kind: 'list',
        value: {
          inner: {
            kind: 'record',
            value: {
              fields: [
                { name: 'a', typ: { kind: 'string' } },
                { name: 'b', typ: { kind: 's32' } },
                { name: 'c', typ: { kind: 'bool' } },
              ],
              name: 'object-type',
              owner: undefined,
            },
          },
          name: 'list-complex-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'h',
      typ: {
        kind: 'tuple',
        value: {
          items: [{ kind: 'string' }, { kind: 's32' }, { kind: 'bool' }],
          name: 'tuple-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'i',
      typ: {
        kind: 'tuple',
        value: {
          items: [
            { kind: 'string' },
            { kind: 's32' },
            {
              kind: 'record',
              value: {
                fields: [
                  { name: 'a', typ: { kind: 'string' } },
                  { name: 'b', typ: { kind: 's32' } },
                  { name: 'c', typ: { kind: 'bool' } },
                ],
                name: 'object-type',
                owner: undefined,
              },
            },
          ],
          name: 'tuple-complex-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'j',
      typ: {
        kind: 'list',
        value: {
          inner: {
            kind: 'tuple',
            value: {
              items: [{ kind: 'string' }, { kind: 's32' }],
              name: undefined,
              owner: undefined,
            },
          },
          name: 'map-type',
          owner: undefined,
        },
      },
    },
    {
      name: 'k',
      typ: {
        kind: 'record',
        value: {
          fields: [{ name: 'n', typ: { kind: 's32' } }],
          name: 'simple-interface-type',
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
