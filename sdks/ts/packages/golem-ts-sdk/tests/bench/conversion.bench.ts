import { bench, describe } from 'vitest';
import * as WitValue from '../../src/internal/mapping/values/WitValue';
import {
  AnalysedType,
  str,
  u32,
  bool,
  u64,
  s64,
  f64,
  field,
  record,
  option,
  list,
  tuple,
  result,
  variant,
  case_,
  unitCase,
} from '../../src/internal/mapping/types/analysedType';
import { getTestInterfaceType } from '../testUtils';
import { TestInterfaceType } from '../testTypes';
import {
  serializeToDataValue,
  deserializeDataValue,
} from '../../src/internal/mapping/values/dataValue';

// --- Helper ---
function serialize(tsValue: any, typ: AnalysedType): WitValue.WitValue {
  return WitValue.fromTsValueDefault(tsValue, typ);
}

// =====================================================================
// 1. Primitives
// =====================================================================

const stringType: AnalysedType = str();
const u32Type: AnalysedType = u32();
const boolType: AnalysedType = bool();
const u64Type: AnalysedType = u64(true);
const s64Type: AnalysedType = s64(true);
const f64Type: AnalysedType = f64();

const testString = 'hello world benchmark test string';
const testNumber = 42;
const testBool = true;
const testBigint = 9007199254740993n;
const testSignedBigint = 123456789n;
const testFloat = 3.14159;

const stringWit = serialize(testString, stringType);
const u32Wit = serialize(testNumber, u32Type);
const boolWit = serialize(testBool, boolType);
const u64Wit = serialize(testBigint, u64Type);
const s64Wit = serialize(testSignedBigint, s64Type);
const f64Wit = serialize(testFloat, f64Type);

describe('Primitives', () => {
  bench(
    'serialize string',
    () => {
      WitValue.fromTsValueDefault(testString, stringType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize string',
    () => {
      WitValue.toTsValue(stringWit, stringType);
    },
    { time: 1000 },
  );

  bench(
    'serialize u32',
    () => {
      WitValue.fromTsValueDefault(testNumber, u32Type);
    },
    { time: 1000 },
  );

  bench(
    'deserialize u32',
    () => {
      WitValue.toTsValue(u32Wit, u32Type);
    },
    { time: 1000 },
  );

  bench(
    'serialize bool',
    () => {
      WitValue.fromTsValueDefault(testBool, boolType);
    },
    { time: 1000 },
  );

  bench(
    'serialize u64 (bigint)',
    () => {
      WitValue.fromTsValueDefault(testBigint, u64Type);
    },
    { time: 1000 },
  );

  bench(
    'serialize s64 (bigint)',
    () => {
      WitValue.fromTsValueDefault(testSignedBigint, s64Type);
    },
    { time: 1000 },
  );

  bench(
    'deserialize s64',
    () => {
      WitValue.toTsValue(s64Wit, s64Type);
    },
    { time: 1000 },
  );

  bench(
    'serialize f64',
    () => {
      WitValue.fromTsValueDefault(testFloat, f64Type);
    },
    { time: 1000 },
  );

  bench(
    'deserialize f64',
    () => {
      WitValue.toTsValue(f64Wit, f64Type);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 2. Flat record (10 fields)
// =====================================================================

const flatRecordType: AnalysedType = record('FlatRecord', [
  field('name', str()),
  field('email', str()),
  field('city', str()),
  field('age', u32()),
  field('score', u32()),
  field('count', u32()),
  field('active', bool()),
  field('verified', bool()),
  field('id', u64(true)),
  field('nickname', option('nickname', 'undefined', str())),
]);

const flatRecordValue = {
  name: 'Alice',
  email: 'alice@example.com',
  city: 'Wonderland',
  age: 30,
  score: 100,
  count: 5,
  active: true,
  verified: false,
  id: 123456789n,
  nickname: 'ally',
};

const flatRecordWit = serialize(flatRecordValue, flatRecordType);

describe('Flat record (10 fields)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(flatRecordValue, flatRecordType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(flatRecordWit, flatRecordType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 3. Nested record (3 levels)
// =====================================================================

const innerRecordType: AnalysedType = record('Inner', [
  field('x', u32()),
  field('y', u32()),
  field('label', str()),
]);

const middleRecordType: AnalysedType = record('Middle', [
  field('inner', innerRecordType),
  field('name', str()),
  field('active', bool()),
]);

const outerRecordType: AnalysedType = record('Outer', [
  field('middle', middleRecordType),
  field('id', str()),
  field('count', u32()),
]);

const nestedRecordValue = {
  middle: {
    inner: { x: 10, y: 20, label: 'point' },
    name: 'mid',
    active: true,
  },
  id: 'outer-1',
  count: 42,
};

const nestedRecordWit = serialize(nestedRecordValue, outerRecordType);

describe('Nested record (3 levels)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(nestedRecordValue, outerRecordType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(nestedRecordWit, outerRecordType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 4. List of primitives (1000 strings)
// =====================================================================

const listStringType: AnalysedType = list('StringList', undefined, undefined, str());
const stringList = Array.from({ length: 1000 }, (_, i) => `item-${i}`);
const stringListWit = serialize(stringList, listStringType);

describe('List of 1000 strings', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(stringList, listStringType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(stringListWit, listStringType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 5. TypedArray (Uint8Array of 1000 bytes)
// =====================================================================

const listU8Type: AnalysedType = list('ByteList', 'u8', undefined, { kind: 'u8' });
const byteArray = new Uint8Array(1000);
for (let i = 0; i < 1000; i++) byteArray[i] = i & 0xff;
const byteArrayWit = serialize(byteArray, listU8Type);

describe('TypedArray (Uint8Array, 1000 bytes)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(byteArray, listU8Type);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(byteArrayWit, listU8Type);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 6. Complex type (TestInterfaceType)
// =====================================================================

const [complexAnalysedType] = getTestInterfaceType();

const complexValue: TestInterfaceType = {
  numberProp: 42,
  stringProp: 'benchmark',
  booleanProp: true,
  bigintProp: 9007199254740993n,
  trueProp: true,
  falseProp: false,
  optionalProp: 99,
  nestedProp: { n: 7 },
  unionProp: 'hello',
  unionComplexProp: 1,
  objectProp: { a: 'x', b: 1, c: true },
  objectComplexProp: {
    a: 'a',
    b: 2,
    c: false,
    d: { a: 'da', b: 3, c: true },
    e: 42,
    f: ['f1', 'f2'],
    g: [{ a: 'ga', b: 4, c: false }],
    h: ['h', 5, true],
    i: ['i', 6, { a: 'ia', b: 7, c: true }],
    j: new Map([
      ['k1', 1],
      ['k2', 2],
    ]),
    k: { n: 8 },
  },
  listProp: ['a', 'b', 'c'],
  listObjectProp: [{ a: 'la', b: 9, c: false }],
  tupleProp: ['t', 10, true],
  tupleObjectProp: ['to', 11, { a: 'ta', b: 12, c: false }],
  mapProp: new Map([
    ['m1', 100],
    ['m2', 200],
  ]),
  uint8ArrayProp: new Uint8Array([1, 2, 3]),
  uint16ArrayProp: new Uint16Array([1, 2, 3]),
  uint32ArrayProp: new Uint32Array([1, 2, 3]),
  uint64ArrayProp: new BigUint64Array([1n, 2n, 3n]),
  int8ArrayProp: new Int8Array([1, 2, 3]),
  int16ArrayProp: new Int16Array([1, 2, 3]),
  int32ArrayProp: new Int32Array([1, 2, 3]),
  int64ArrayProp: new BigInt64Array([1n, 2n, 3n]),
  float32ArrayProp: new Float32Array([1.1, 2.2, 3.3]),
  float64ArrayProp: new Float64Array([1.1, 2.2, 3.3]),
  objectPropInlined: { a: 'inl', b: 13, c: true },
  unionPropInlined: 'foo',
};

const complexWit = serialize(complexValue, complexAnalysedType);

describe('Complex type (TestInterfaceType)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(complexValue, complexAnalysedType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(complexWit, complexAnalysedType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 7. List of 1000 records (extractor allocation stress test)
// =====================================================================

const listRecordType: AnalysedType = list(
  'RecordList',
  undefined,
  undefined,
  record('Item', [
    field('id', u32()),
    field('name', str()),
    field('active', bool()),
    field('score', u32()),
    field('email', str()),
    field('count', u32()),
    field('verified', bool()),
    field('code', str()),
    field('level', u32()),
    field('label', str()),
  ]),
);

const recordList = Array.from({ length: 1000 }, (_, i) => ({
  id: i,
  name: `user-${i}`,
  active: i % 2 === 0,
  score: i * 10,
  email: `user${i}@test.com`,
  count: i % 100,
  verified: i % 3 === 0,
  code: `CODE-${i}`,
  level: i % 10,
  label: `label-${i}`,
}));

const recordListWit = serialize(recordList, listRecordType);

describe('List of 1000 records (10 fields each)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(recordList, listRecordType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(recordListWit, listRecordType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 8. Variant/union
// =====================================================================

const variantType: AnalysedType = variant(
  'MyVariant',
  [],
  [
    case_('text', str()),
    case_('number', u32()),
    case_('payload', record('Payload', [field('key', str()), field('value', u32())])),
    unitCase('empty'),
  ],
);

const variantStringVal = { tag: 'text', val: 'hello' };
const variantNumberVal = { tag: 'number', val: 42 };
const variantRecordVal = { tag: 'payload', val: { key: 'k', value: 99 } };

describe('Variant', () => {
  bench(
    'serialize string case',
    () => {
      WitValue.fromTsValueDefault(variantStringVal, variantType);
    },
    { time: 1000 },
  );

  bench(
    'serialize number case',
    () => {
      WitValue.fromTsValueDefault(variantNumberVal, variantType);
    },
    { time: 1000 },
  );

  bench(
    'serialize record case',
    () => {
      WitValue.fromTsValueDefault(variantRecordVal, variantType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 9. Full invoke round-trip simulation
// =====================================================================

const invokeArgType: AnalysedType = record('InvokeArg', [
  field('name', str()),
  field('count', u32()),
  field('active', bool()),
]);

const invokeArg = { name: 'test', count: 10, active: true };
const invokeTypeInfo = {
  tag: 'analysed' as const,
  val: invokeArgType,
  witType: undefined as any,
  tsType: undefined as any,
};

const preSerializedDataValue = serializeToDataValue(invokeArg, invokeTypeInfo);

describe('Full invoke round-trip', () => {
  bench(
    'serialize to DataValue',
    () => {
      serializeToDataValue(invokeArg, invokeTypeInfo);
    },
    { time: 1000 },
  );

  bench(
    'deserialize from DataValue',
    () => {
      deserializeDataValue(preSerializedDataValue, [{ name: 'arg', type: invokeTypeInfo }], {
        tag: 'anonymous',
      });
    },
    { time: 1000 },
  );
});

// =====================================================================
// 10. Result type
// =====================================================================

const resultType: AnalysedType = result(
  undefined,
  { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: undefined },
  str(),
  str(),
);
const resultOkVal = { tag: 'ok', val: 'success' };
const resultErrVal = { tag: 'err', val: 'failure' };
const resultOkWit = serialize(resultOkVal, resultType);
const resultErrWit = serialize(resultErrVal, resultType);

// =====================================================================
// 10b. Map serialization (100 entries) – targets #8
// =====================================================================

const mapInnerType: AnalysedType = tuple('MapEntry', undefined, [str(), u32()]);
const mapType: AnalysedType = list('StringToU32Map', undefined, undefined, mapInnerType);
const smallMap = new Map(Array.from({ length: 100 }, (_, i) => [`key-${i}`, i] as [string, number]));
const smallMapWit = serialize(smallMap, mapType);

describe('Map (100 string→u32 entries)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(smallMap, mapType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(smallMapWit, mapType);
    },
    { time: 1000 },
  );
});

// =====================================================================
// 10c. Map serialization (1000 entries) – targets #8
// =====================================================================

const largeMap = new Map(
  Array.from({ length: 1000 }, (_, i) => [`key-${i}`, i] as [string, number]),
);
const largeMapWit = serialize(largeMap, mapType);

describe('Map (1000 string→u32 entries)', () => {
  bench(
    'serialize',
    () => {
      WitValue.fromTsValueDefault(largeMap, mapType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize',
    () => {
      WitValue.toTsValue(largeMapWit, mapType);
    },
    { time: 1000 },
  );
});

describe('Result type', () => {
  bench(
    'serialize ok',
    () => {
      WitValue.fromTsValueDefault(resultOkVal, resultType);
    },
    { time: 1000 },
  );

  bench(
    'serialize err',
    () => {
      WitValue.fromTsValueDefault(resultErrVal, resultType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize ok',
    () => {
      WitValue.toTsValue(resultOkWit, resultType);
    },
    { time: 1000 },
  );

  bench(
    'deserialize err',
    () => {
      WitValue.toTsValue(resultErrWit, resultType);
    },
    { time: 1000 },
  );
});
