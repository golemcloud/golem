import { bench, describe } from 'vitest';
import * as WitValue from '../../src/internal/mapping/values/WitValue';
import {
  AnalysedType,
  str,
  u32,
  variant,
  case_,
  unitCase,
} from '../../src/internal/mapping/types/analysedType';

// Small variant (4 cases) with a mix of unit and payload cases
const smallVariant: AnalysedType = variant(
  'SmallVariant',
  [],
  [unitCase('none'), case_('some-number', u32()), case_('some-string', str()), unitCase('unknown')],
);

// Large variant (50 cases) — amplifies redundant findIndex cost
const largeCases = Array.from({ length: 50 }, (_, i) =>
  i % 3 === 0 ? unitCase(`case-${i}`) : case_(`case-${i}`, i % 2 === 0 ? u32() : str()),
);
const largeVariant: AnalysedType = variant('LargeVariant', [], largeCases);

describe('Tagged variant serialization', () => {
  // --- Small variant ---
  bench(
    'small variant, unit case (first)',
    () => {
      WitValue.fromTsValueDefault({ tag: 'none' }, smallVariant);
    },
    { time: 1000 },
  );

  bench(
    'small variant, unit case (last)',
    () => {
      WitValue.fromTsValueDefault({ tag: 'unknown' }, smallVariant);
    },
    { time: 1000 },
  );

  bench(
    'small variant, payload case',
    () => {
      WitValue.fromTsValueDefault({ tag: 'some-number', value: 42 }, smallVariant);
    },
    { time: 1000 },
  );

  // --- Large variant (50 cases) ---
  bench(
    'large variant (50), unit case first',
    () => {
      WitValue.fromTsValueDefault({ tag: 'case-0' }, largeVariant);
    },
    { time: 1000 },
  );

  bench(
    'large variant (50), payload case middle',
    () => {
      WitValue.fromTsValueDefault({ tag: 'case-25', value: 'hello' }, largeVariant);
    },
    { time: 1000 },
  );

  bench(
    'large variant (50), unit case last',
    () => {
      WitValue.fromTsValueDefault({ tag: 'case-48' }, largeVariant);
    },
    { time: 1000 },
  );

  bench(
    'large variant (50), payload case last',
    () => {
      WitValue.fromTsValueDefault({ tag: 'case-49', value: 'world' }, largeVariant);
    },
    { time: 1000 },
  );

  // Batch: serialize 1000 tagged variants from large variant
  bench(
    '1000x large variant, mixed cases',
    () => {
      for (let i = 0; i < 1000; i++) {
        const idx = i % 50;
        const isUnit = idx % 3 === 0;
        const val = isUnit
          ? { tag: `case-${idx}` }
          : { tag: `case-${idx}`, value: idx % 2 === 0 ? idx : `str-${idx}` };
        WitValue.fromTsValueDefault(val, largeVariant);
      }
    },
    { time: 1000 },
  );
});
