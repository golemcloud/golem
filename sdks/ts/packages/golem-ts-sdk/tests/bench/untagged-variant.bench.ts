import { bench, describe } from 'vitest';
import * as WitValue from '../../src/internal/mapping/values/WitValue';
import {
  AnalysedType,
  str,
  u32,
  bool,
  u64,
  field,
  record,
  variant,
  case_,
  unitCase,
} from '../../src/internal/mapping/types/analysedType';

// =====================================================================
// Untagged variant serialization benchmark
//
// This exercises the O(n×m) trial-matching path in serializeUnionToWitNodes
// (serializer.ts ~lines 744-778) where taggedTypes.length === 0.
// Each value is matched against every case via recursive matchesType().
// =====================================================================

// --- Small variant (4 cases, last is a record with 3 fields) ---
const smallVariant: AnalysedType = variant(
  'SmallVariant',
  [],
  [
    case_('text', str()),
    case_('number', u32()),
    case_('flag', bool()),
    case_(
      'payload',
      record('SmallPayload', [field('key', str()), field('value', u32()), field('active', bool())]),
    ),
  ],
);

// --- Medium variant (8 cases, records with 5-6 fields) ---
const mediumVariant: AnalysedType = variant(
  'MediumVariant',
  [],
  [
    case_('text', str()),
    case_('number', u32()),
    case_('flag', bool()),
    case_('bignum', u64(true)),
    case_(
      'rec1',
      record('MedRec1', [
        field('a', str()),
        field('b', u32()),
        field('c', bool()),
        field('d', str()),
        field('e', u32()),
      ]),
    ),
    case_(
      'rec2',
      record('MedRec2', [
        field('x', str()),
        field('y', u32()),
        field('z', bool()),
        field('w', str()),
        field('v', u32()),
        field('u', bool()),
      ]),
    ),
    case_(
      'rec3',
      record('MedRec3', [
        field('p', str()),
        field('q', u32()),
        field('r', bool()),
        field('s', str()),
        field('t', u32()),
      ]),
    ),
    unitCase('none'),
  ],
);

// --- Large variant (16 cases, records with 8 fields) ---
function makeRecord(name: string): AnalysedType {
  return record(name, [
    field('f1', str()),
    field('f2', u32()),
    field('f3', bool()),
    field('f4', str()),
    field('f5', u32()),
    field('f6', bool()),
    field('f7', str()),
    field('f8', u32()),
  ]);
}

const largeVariant: AnalysedType = variant(
  'LargeVariant',
  [],
  [
    case_('text', str()),
    case_('number', u32()),
    case_('flag', bool()),
    case_('bignum', u64(true)),
    case_('rec01', makeRecord('LgRec01')),
    case_('rec02', makeRecord('LgRec02')),
    case_('rec03', makeRecord('LgRec03')),
    case_('rec04', makeRecord('LgRec04')),
    case_('rec05', makeRecord('LgRec05')),
    case_('rec06', makeRecord('LgRec06')),
    case_('rec07', makeRecord('LgRec07')),
    case_('rec08', makeRecord('LgRec08')),
    case_('rec09', makeRecord('LgRec09')),
    case_('rec10', makeRecord('LgRec10')),
    case_('rec11', makeRecord('LgRec11')),
    unitCase('empty'),
  ],
);

// --- Test values ---
// Best case: first case matches
const smallFirst = 'hello';
// Worst case: last case matches (record)
const smallLast = { key: 'k', value: 99, active: true };

const mediumFirst = 'hello';
const mediumLast = 'none'; // unit case at position 7

// For medium, match the last record (position 6)
const mediumLastRecord = { p: 'pp', q: 5, r: true, s: 'ss', t: 10 };

// For large, match last record (position 14)
const largeLast = { f1: 'a', f2: 1, f3: true, f4: 'b', f5: 2, f6: false, f7: 'c', f8: 3 };

describe('Untagged variant - small (4 cases)', () => {
  bench(
    'first case (string)',
    () => {
      WitValue.fromTsValueDefault(smallFirst, smallVariant);
    },
    { time: 1000 },
  );

  bench(
    'last case (3-field record)',
    () => {
      WitValue.fromTsValueDefault(smallLast, smallVariant);
    },
    { time: 1000 },
  );
});

describe('Untagged variant - medium (8 cases)', () => {
  bench(
    'first case (string)',
    () => {
      WitValue.fromTsValueDefault(mediumFirst, mediumVariant);
    },
    { time: 1000 },
  );

  bench(
    'last record case (5-field record, pos 6)',
    () => {
      WitValue.fromTsValueDefault(mediumLastRecord, mediumVariant);
    },
    { time: 1000 },
  );

  bench(
    'unit case (pos 7)',
    () => {
      WitValue.fromTsValueDefault(mediumLast, mediumVariant);
    },
    { time: 1000 },
  );
});

describe('Untagged variant - large (16 cases, 8-field records)', () => {
  bench(
    'first case (string)',
    () => {
      WitValue.fromTsValueDefault('hello', largeVariant);
    },
    { time: 1000 },
  );

  bench(
    'last record case (8-field record, pos 14)',
    () => {
      WitValue.fromTsValueDefault(largeLast, largeVariant);
    },
    { time: 1000 },
  );
});
