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

// Benchmarks for the schema-native value codec (`golem:core/types@2.0.0`
// `schema-value-tree`). This replaces the legacy `AnalysedType` / `WitValue` /
// `DataValue` benchmark: the SDK now thinks only in the new schema model, so we
// measure the round-trip between the in-memory `SchemaValue` and its flat,
// index-based WIT wire form (`schemaValueToWit` / `schemaValueFromWit`).

import { bench, describe } from './harness';
import {
  SchemaValue,
  v,
  schemaValueToWit,
  schemaValueFromWit,
} from '../../src/internal/schema-model';

const TIME = 1000;

// =====================================================================
// Fixtures
// =====================================================================

const stringVal = v.string('hello world benchmark test string');
const u32Val = v.u32(42);
const boolVal = v.bool(true);
const u64Val = v.u64(9007199254740993n);
const s64Val = v.s64(123456789n);
const f64Val = v.f64(3.14159);

const recordVal = v.record([v.string('Alice'), v.u32(30), v.bool(true)]);
const optionSomeVal = v.option(v.string('present'));
const optionNoneVal = v.option();
const tupleVal = v.tuple([v.string('x'), v.u32(7), v.bool(false)]);
const resultOkVal = v.ok(v.u32(1));
const resultErrVal = v.err(v.string('boom'));
const variantVal = v.variant(1, v.string('payload'));

const listVal = v.list(Array.from({ length: 100 }, (_, i) => v.u32(i)));

// A nested record-of-lists-of-records, exercising deep traversal.
const nestedVal = v.record([
  v.string('root'),
  v.list(
    Array.from({ length: 20 }, (_, i) =>
      v.record([v.u32(i), v.string(`item-${i}`), v.option(v.bool(i % 2 === 0))]),
    ),
  ),
]);

// A deeply recursive tree value (a major advantage of the new schema): each
// node is `record { value: u32, children: list<node> }`. Values carry no schema
// of their own, so recursion shows up purely as nested record/list shapes.
function makeTree(depth: number, breadth: number, counter: { n: number }): SchemaValue {
  const children: SchemaValue[] = [];
  if (depth > 0) {
    for (let i = 0; i < breadth; i++) {
      children.push(makeTree(depth - 1, breadth, counter));
    }
  }
  return v.record([v.u32(counter.n++), v.list(children)]);
}

const recursiveVal = makeTree(6, 2, { n: 0 }); // 2^7 - 1 = 127 nodes

// Pre-encoded wire forms for the decode benchmarks.
const stringWit = schemaValueToWit(stringVal);
const u32Wit = schemaValueToWit(u32Val);
const boolWit = schemaValueToWit(boolVal);
const u64Wit = schemaValueToWit(u64Val);
const s64Wit = schemaValueToWit(s64Val);
const f64Wit = schemaValueToWit(f64Val);

const recordWit = schemaValueToWit(recordVal);
const optionSomeWit = schemaValueToWit(optionSomeVal);
const optionNoneWit = schemaValueToWit(optionNoneVal);
const tupleWit = schemaValueToWit(tupleVal);
const resultOkWit = schemaValueToWit(resultOkVal);
const resultErrWit = schemaValueToWit(resultErrVal);
const variantWit = schemaValueToWit(variantVal);
const listWit = schemaValueToWit(listVal);
const nestedWit = schemaValueToWit(nestedVal);
const recursiveWit = schemaValueToWit(recursiveVal);

// =====================================================================
// 1. Primitives
// =====================================================================

describe('Primitives: encode (SchemaValue -> wire)', () => {
  bench('string', () => void schemaValueToWit(stringVal), { time: TIME });
  bench('u32', () => void schemaValueToWit(u32Val), { time: TIME });
  bench('bool', () => void schemaValueToWit(boolVal), { time: TIME });
  bench('u64', () => void schemaValueToWit(u64Val), { time: TIME });
  bench('s64', () => void schemaValueToWit(s64Val), { time: TIME });
  bench('f64', () => void schemaValueToWit(f64Val), { time: TIME });
});

describe('Primitives: decode (wire -> SchemaValue)', () => {
  bench('string', () => void schemaValueFromWit(stringWit), { time: TIME });
  bench('u32', () => void schemaValueFromWit(u32Wit), { time: TIME });
  bench('bool', () => void schemaValueFromWit(boolWit), { time: TIME });
  bench('u64', () => void schemaValueFromWit(u64Wit), { time: TIME });
  bench('s64', () => void schemaValueFromWit(s64Wit), { time: TIME });
  bench('f64', () => void schemaValueFromWit(f64Wit), { time: TIME });
});

// =====================================================================
// 2. Composites
// =====================================================================

describe('Composites: encode (SchemaValue -> wire)', () => {
  bench('record', () => void schemaValueToWit(recordVal), { time: TIME });
  bench('option some', () => void schemaValueToWit(optionSomeVal), { time: TIME });
  bench('option none', () => void schemaValueToWit(optionNoneVal), { time: TIME });
  bench('tuple', () => void schemaValueToWit(tupleVal), { time: TIME });
  bench('result ok', () => void schemaValueToWit(resultOkVal), { time: TIME });
  bench('result err', () => void schemaValueToWit(resultErrVal), { time: TIME });
  bench('variant', () => void schemaValueToWit(variantVal), { time: TIME });
  bench('list (100)', () => void schemaValueToWit(listVal), { time: TIME });
});

describe('Composites: decode (wire -> SchemaValue)', () => {
  bench('record', () => void schemaValueFromWit(recordWit), { time: TIME });
  bench('option some', () => void schemaValueFromWit(optionSomeWit), { time: TIME });
  bench('option none', () => void schemaValueFromWit(optionNoneWit), { time: TIME });
  bench('tuple', () => void schemaValueFromWit(tupleWit), { time: TIME });
  bench('result ok', () => void schemaValueFromWit(resultOkWit), { time: TIME });
  bench('result err', () => void schemaValueFromWit(resultErrWit), { time: TIME });
  bench('variant', () => void schemaValueFromWit(variantWit), { time: TIME });
  bench('list (100)', () => void schemaValueFromWit(listWit), { time: TIME });
});

// =====================================================================
// 3. Nested and recursive shapes
// =====================================================================

describe('Nested / recursive', () => {
  bench('nested encode', () => void schemaValueToWit(nestedVal), { time: TIME });
  bench('nested decode', () => void schemaValueFromWit(nestedWit), { time: TIME });
  bench('nested roundtrip', () => void schemaValueFromWit(schemaValueToWit(nestedVal)), {
    time: TIME,
  });

  bench('recursive tree encode (127 nodes)', () => void schemaValueToWit(recursiveVal), {
    time: TIME,
  });
  bench('recursive tree decode (127 nodes)', () => void schemaValueFromWit(recursiveWit), {
    time: TIME,
  });
  bench(
    'recursive tree roundtrip (127 nodes)',
    () => void schemaValueFromWit(schemaValueToWit(recursiveVal)),
    { time: TIME },
  );
});
