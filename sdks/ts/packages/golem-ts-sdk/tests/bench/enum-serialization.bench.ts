import { bench, describe } from 'vitest';
import * as WitValue from '../../src/internal/mapping/values/WitValue';
import { AnalysedType, enum_ } from '../../src/internal/mapping/types/analysedType';

// Small enum (4 cases) — worst-case is last element
const smallEnum: AnalysedType = enum_('SmallEnum', ['alpha', 'beta', 'gamma', 'delta']);

// Large enum (50 cases) — amplifies O(n) cost
const largeCases = Array.from({ length: 50 }, (_, i) => `case_${i}`);
const largeEnum: AnalysedType = enum_('LargeEnum', largeCases);

describe('Enum serialization', () => {
  // Small enum — first case (best case for linear scan)
  bench('small enum, first case', () => {
    WitValue.fromTsValueDefault('alpha', smallEnum);
  }, { time: 1000 });

  // Small enum — last case (worst case for linear scan)
  bench('small enum, last case', () => {
    WitValue.fromTsValueDefault('delta', smallEnum);
  }, { time: 1000 });

  // Large enum — first case
  bench('large enum (50), first case', () => {
    WitValue.fromTsValueDefault('case_0', largeEnum);
  }, { time: 1000 });

  // Large enum — middle case
  bench('large enum (50), middle case', () => {
    WitValue.fromTsValueDefault('case_25', largeEnum);
  }, { time: 1000 });

  // Large enum — last case (worst case)
  bench('large enum (50), last case', () => {
    WitValue.fromTsValueDefault('case_49', largeEnum);
  }, { time: 1000 });

  // Batch: serialize 1000 enums from large enum
  bench('1000x large enum, mixed cases', () => {
    for (let i = 0; i < 1000; i++) {
      WitValue.fromTsValueDefault(`case_${i % 50}`, largeEnum);
    }
  }, { time: 1000 });
});
