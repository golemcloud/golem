// Item 9 — the `Filter` DSL for getAgents (DNF compilation) + `RevertTarget`
// builders. Pure compilation logic, no host calls.

import { describe, expect, it } from 'vitest';
import { Filter, RevertTarget } from '../src/host/hostapi';

describe('Filter → agent-any-filter (DNF)', () => {
  it('a single leaf is one conjunction of one property', () => {
    expect(Filter.status('equal', 'idle').toRaw()).toEqual({
      filters: [{ filters: [{ tag: 'status', val: { comparator: 'equal', value: 'idle' } }] }],
    });
  });

  it('`.and` groups leaves into one conjunction', () => {
    const raw = Filter.status('equal', 'idle').and(Filter.name('starts-with', 'w-')).toRaw();
    expect(raw.filters).toHaveLength(1);
    expect(raw.filters[0]!.filters.map((f) => f.tag)).toEqual(['status', 'name']);
  });

  it('`.or` produces separate conjunctions', () => {
    const raw = Filter.status('equal', 'idle').or(Filter.status('equal', 'running')).toRaw();
    expect(raw.filters).toHaveLength(2);
    expect(raw.filters.every((c) => c.filters.length === 1)).toBe(true);
  });

  it('distributes AND over OR into disjunctive normal form', () => {
    // (A or B) and C  →  (A and C) or (B and C)
    const A = Filter.name('equal', 'a');
    const B = Filter.name('equal', 'b');
    const C = Filter.status('equal', 'idle');
    const raw = A.or(B).and(C).toRaw();
    expect(raw.filters).toHaveLength(2);
    for (const conj of raw.filters) {
      const tags = conj.filters.map((f) => f.tag).sort();
      expect(tags).toEqual(['name', 'status']);
    }
  });

  it('supports env/config/version/createdAt leaves', () => {
    expect(Filter.env('REGION', 'equal', 'eu').toRaw().filters[0]!.filters[0]).toEqual({
      tag: 'env',
      val: { name: 'REGION', comparator: 'equal', value: 'eu' },
    });
    expect(Filter.version('greater-equal', 3n).toRaw().filters[0]!.filters[0]).toEqual({
      tag: 'version',
      val: { comparator: 'greater-equal', value: 3n },
    });
  });
});

describe('RevertTarget builders', () => {
  it('toOplogIndex', () => {
    expect(RevertTarget.toOplogIndex(42n)).toEqual({ tag: 'revert-to-oplog-index', val: 42n });
  });
  it('lastInvocations coerces to bigint', () => {
    expect(RevertTarget.lastInvocations(3)).toEqual({ tag: 'revert-last-invocations', val: 3n });
    expect(RevertTarget.lastInvocations(5n)).toEqual({ tag: 'revert-last-invocations', val: 5n });
  });
});
