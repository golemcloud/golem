// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { describe, expect, it } from 'vitest';
import { bridge, Datetime } from '../src';

describe('Datetime', () => {
  it('is the same helper object in the root and bridge exports', () => {
    expect(bridge.Datetime).toBe(Datetime);
  });

  it.each([
    ['1970-01-01T00:00:00Z', { seconds: 0n, nanoseconds: 0 }, '1970-01-01T00:00:00Z'],
    [
      '2024-02-29T12:34:56.123456789Z',
      { seconds: 1709210096n, nanoseconds: 123456789 },
      '2024-02-29T12:34:56.123456789Z',
    ],
    [
      '2024-02-29T12:34:56.120Z',
      { seconds: 1709210096n, nanoseconds: 120000000 },
      '2024-02-29T12:34:56.12Z',
    ],
    [
      '2024-03-01T01:04:56.5+02:30',
      { seconds: 1709246096n, nanoseconds: 500000000 },
      '2024-02-29T22:34:56.5Z',
    ],
    ['1969-12-31t19:00:00-05:00', { seconds: 0n, nanoseconds: 0 }, '1970-01-01T00:00:00Z'],
    [
      '1969-12-31 23:59:59.999z',
      { seconds: -1n, nanoseconds: 999000000 },
      '1969-12-31T23:59:59.999Z',
    ],
    ['0000-01-01T00:00:00Z', { seconds: -62167219200n, nanoseconds: 0 }, '0000-01-01T00:00:00Z'],
    [
      '9999-12-31T23:59:59.999999999Z',
      { seconds: 253402300799n, nanoseconds: 999999999 },
      '9999-12-31T23:59:59.999999999Z',
    ],
  ])('parses %s and emits the canonical UTC form', (text, datetime, canonical) => {
    expect(Datetime.parse(text)).toEqual(datetime);
    expect(Datetime.toISOString(datetime)).toBe(canonical);
  });

  it.each([
    '2023-02-29T00:00:00Z',
    '2024-02-30T00:00:00Z',
    '2024-13-01T00:00:00Z',
    '2024-01-01T24:00:00Z',
    '2024-01-01T00:00:60Z',
    '2024-01-01',
    '2024-01-01T00:00:00',
    '2024-01-01T00:00:00+0100',
    '2024-01-01T00:00:00+24:00',
    '+2024-01-01T00:00:00Z',
  ])('rejects malformed text %s', (text) => {
    expect(() => Datetime.parse(text)).toThrow(/Invalid RFC 3339 datetime/);
  });

  it('rejects more than nine fraction digits', () => {
    expect(() => Datetime.parse('2024-01-01T00:00:00.1234567891Z')).toThrow(/nine fraction/);
  });

  it.each(['0000-01-01T00:00:00+00:01', '9999-12-31T23:59:59-00:01'])(
    'rejects %s, which is outside the years 0000..9999 in UTC',
    (text) => {
      expect(() => Datetime.parse(text)).toThrow(/outside the years 0000\.\.9999/);
    },
  );

  it('rejects formatting outside the years 0000..9999', () => {
    expect(() => Datetime.toISOString({ seconds: 253402300800n, nanoseconds: 0 })).toThrow(
      /outside the years 0000\.\.9999/,
    );
    expect(() => Datetime.toISOString({ seconds: -62167219201n, nanoseconds: 0 })).toThrow(
      /outside the years 0000\.\.9999/,
    );
  });

  it.each([1.5, Number.NaN, -1, 1_000_000_000])('rejects nanoseconds %s', (nanoseconds) => {
    expect(() => Datetime.toISOString({ seconds: 0n, nanoseconds })).toThrow(/nanoseconds/);
    expect(() => Datetime.toDate({ seconds: 0n, nanoseconds })).toThrow(/nanoseconds/);
  });

  it('converts to and from Date', () => {
    const date = new Date('2024-02-29T12:34:56.789Z');
    expect(Datetime.fromDate(date)).toEqual({ seconds: 1709210096n, nanoseconds: 789000000 });
    expect(Datetime.fromDate(new Date(-1))).toEqual({ seconds: -1n, nanoseconds: 999000000 });
    expect(Datetime.toDate({ seconds: 1709210096n, nanoseconds: 789654321 }).getTime()).toBe(
      date.getTime(),
    );
    expect(Datetime.toDate({ seconds: -1n, nanoseconds: 999999999 }).getTime()).toBe(-1);
    expect(() => Datetime.fromDate(new Date(Number.NaN))).toThrow(/Invalid Date/);
    expect(() => Datetime.toDate({ seconds: 9_000_000_000_000n, nanoseconds: 0 })).toThrow(
      /Date range/,
    );
  });
});
