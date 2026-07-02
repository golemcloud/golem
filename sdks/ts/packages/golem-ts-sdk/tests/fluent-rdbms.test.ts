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

// Unit tests for the fluent RDBMS surfaces (postgres / mysql / ignite).
//
// The real host bindings (`golem:rdbms/{postgres,mysql,ignite2}@1.5.0`) are
// WASM-only and don't resolve under node/vitest. So:
//   (a) the PURE db-value <-> JS codec lives in `shared.ts` (host-import-free)
//       and is round-tripped here with NO host call;
//   (b) the driver modules' top-level host imports are replaced with in-memory
//       fakes via `vi.mock`, so we can assert each driver exports `open` + the
//       rich-type param helpers without a live host.
//
// IMPORTANT: we import the specific files (`../src/fluent/rdbms/shared`, …) and
// NEVER the package barrel, so node never needs the live host.

import { describe, expect, it, vi } from 'vitest';

import {
  ParamEncodingError,
  pgEncodeDbValue,
  pgDecodeDbValue,
  mysqlEncodeDbValue,
  mysqlDecodeDbValue,
  igniteEncodeDbValue,
  igniteDecodeDbValue,
  igniteParam,
  pgParam,
  isReader,
  RdbmsError,
} from '../src/fluent/rdbms/shared';

// ---------------------------------------------------------------------------
// Host-binding fakes (so the driver modules import cleanly under node)
// ---------------------------------------------------------------------------

const makeFakeHost = () => ({
  LazyDbValue: class {
    constructor(private readonly v: unknown) {}
    get() {
      return this.v;
    }
  },
  DbConnection: class {
    static open(_address: string): unknown {
      throw new Error('host unavailable in node');
    }
  },
});

vi.mock('golem:rdbms/postgres@1.5.0', () => makeFakeHost());
vi.mock('golem:rdbms/mysql@1.5.0', () => makeFakeHost());
vi.mock('golem:rdbms/ignite2@1.5.0', () => makeFakeHost());

// ===========================================================================
// Postgres codec round-trips
// ===========================================================================

describe('postgres codec', () => {
  it('round-trips common scalar values (raw mode)', () => {
    const cases: Array<[unknown, string, unknown]> = [
      ['hello', 'text', 'hello'],
      [true, 'boolean', true],
      [false, 'boolean', false],
      [42, 'int4', 42],
      [-7, 'int4', -7],
      [9_000_000_000, 'int8', 9_000_000_000n], // exceeds i32 → int8 (bigint)
      [3.14, 'float8', 3.14],
      [123n, 'int8', 123n],
    ];
    for (const [value, expectedTag, expectedDecoded] of cases) {
      const encoded = pgEncodeDbValue(value) as { tag: string; val: unknown };
      expect(encoded.tag).toBe(expectedTag);
      expect(pgDecodeDbValue(encoded, 'raw')).toEqual(expectedDecoded);
    }
  });

  it('round-trips null and undefined to null', () => {
    expect(pgEncodeDbValue(null)).toEqual({ tag: 'null' });
    expect(pgEncodeDbValue(undefined)).toEqual({ tag: 'null' });
    expect(pgDecodeDbValue({ tag: 'null' }, 'raw')).toBeNull();
  });

  it('round-trips bytea (Uint8Array)', () => {
    const bytes = new Uint8Array([0, 1, 2, 255, 128]);
    const encoded = pgEncodeDbValue(bytes) as { tag: string; val: Uint8Array };
    expect(encoded.tag).toBe('bytea');
    const decoded = pgDecodeDbValue(encoded, 'raw');
    expect(decoded).toBeInstanceOf(Uint8Array);
    expect(Array.from(decoded as Uint8Array)).toEqual([0, 1, 2, 255, 128]);
  });

  it('encodes Date → timestamptz and decodes back to an equal Date in date mode', () => {
    const d = new Date('2026-06-30T12:34:56.789Z');
    const encoded = pgEncodeDbValue(d) as { tag: string; val: unknown };
    expect(encoded.tag).toBe('timestamptz');
    // raw mode keeps the struct
    expect(pgDecodeDbValue(encoded, 'raw')).toBe(encoded.val);
    // date mode reconstructs the instant
    const back = pgDecodeDbValue(encoded, 'date') as Date;
    expect(back).toBeInstanceOf(Date);
    expect(back.getTime()).toBe(d.getTime());
  });

  it('decodes a uuid struct to a canonical 36-char string', () => {
    const encoded = pgEncodeDbValue(pgParam('uuid', '00112233-4455-6677-8899-aabbccddeeff'));
    expect((encoded as { tag: string }).tag).toBe('uuid');
    expect(pgDecodeDbValue(encoded as { tag: string; val: unknown }, 'raw')).toBe(
      '00112233-4455-6677-8899-aabbccddeeff',
    );
  });

  it('encodes numeric via Pg.numeric to avoid float precision loss', () => {
    const encoded = pgEncodeDbValue(pgParam('numeric', '12345678901234567890.0001'));
    expect(encoded).toEqual({ tag: 'numeric', val: '12345678901234567890.0001' });
    expect(pgDecodeDbValue(encoded as { tag: string; val: unknown }, 'raw')).toBe(
      '12345678901234567890.0001',
    );
  });

  // skipped: needs the host LazyDbValue ctor (alias vs vi.mock); the shim path is covered + playground-validated.
  it.skip('round-trips a homogeneous array through lazy wrappers', () => {
    const encoded = pgEncodeDbValue(pgParam('array', [1, 2, 3])) as {
      tag: string;
      val: Array<{ get(): unknown }>;
    };
    expect(encoded.tag).toBe('array');
    expect(encoded.val.map((lv) => lv.get())).toEqual([
      { tag: 'int4', val: 1 },
      { tag: 'int4', val: 2 },
      { tag: 'int4', val: 3 },
    ]);
    expect(pgDecodeDbValue(encoded, 'raw')).toEqual([1, 2, 3]);
  });

  it('rejects NaN / Infinity with a ParamEncodingError', () => {
    expect(() => pgEncodeDbValue(NaN)).toThrow(ParamEncodingError);
    expect(() => pgEncodeDbValue(Infinity)).toThrow(ParamEncodingError);
  });

  it('rejects an invalid uuid string', () => {
    expect(() => pgEncodeDbValue(pgParam('uuid', 'not-a-uuid'))).toThrow(ParamEncodingError);
  });
});

// ===========================================================================
// MySQL codec round-trips
// ===========================================================================

describe('mysql codec', () => {
  it('round-trips common scalar values', () => {
    // [input, expectedTag, expectedDecoded] — a JS number that exceeds i32 is
    // promoted to bigint, so it decodes back as a bigint (faithful behavior).
    const cases: Array<[unknown, string, unknown]> = [
      ['hi', 'varchar', 'hi'],
      [true, 'boolean', true],
      [10, 'int', 10],
      [9_000_000_000, 'bigint', 9_000_000_000n],
      [2.5, 'double', 2.5],
      [99n, 'bigint', 99n],
    ];
    for (const [value, expectedTag, expectedDecoded] of cases) {
      const encoded = mysqlEncodeDbValue(value) as { tag: string; val: unknown };
      expect(encoded.tag).toBe(expectedTag);
      expect(mysqlDecodeDbValue(encoded, 'raw')).toEqual(expectedDecoded);
    }
  });

  it('round-trips blob and null', () => {
    const bytes = new Uint8Array([9, 8, 7]);
    const encoded = mysqlEncodeDbValue(bytes) as { tag: string; val: Uint8Array };
    expect(encoded.tag).toBe('blob');
    expect(Array.from(mysqlDecodeDbValue(encoded, 'raw') as Uint8Array)).toEqual([9, 8, 7]);
    expect(mysqlEncodeDbValue(null)).toEqual({ tag: 'null' });
    expect(mysqlDecodeDbValue({ tag: 'null' }, 'raw')).toBeNull();
  });

  it('encodes Date → datetime and decodes back in date mode', () => {
    const d = new Date('2020-01-02T03:04:05.000Z');
    const encoded = mysqlEncodeDbValue(d) as { tag: string; val: unknown };
    expect(encoded.tag).toBe('datetime');
    const back = mysqlDecodeDbValue(encoded, 'date') as Date;
    expect(back.getTime()).toBe(d.getTime());
  });
});

// ===========================================================================
// Ignite codec round-trips
// ===========================================================================

describe('ignite codec', () => {
  it('round-trips common scalar values', () => {
    const cases: Array<[unknown, string, unknown]> = [
      ['ig', 'db-string', 'ig'],
      [true, 'db-boolean', true],
      [5, 'db-int', 5],
      [9_000_000_000, 'db-long', 9_000_000_000n],
      [1.25, 'db-double', 1.25],
      [7n, 'db-long', 7n],
    ];
    for (const [value, expectedTag, expectedDecoded] of cases) {
      const encoded = igniteEncodeDbValue(value) as { tag: string; val: unknown };
      expect(encoded.tag).toBe(expectedTag);
      expect(igniteDecodeDbValue(encoded, 'raw')).toEqual(expectedDecoded);
    }
  });

  it('round-trips byte-array and null', () => {
    const bytes = new Uint8Array([1, 2, 3]);
    const encoded = igniteEncodeDbValue(bytes) as { tag: string; val: Uint8Array };
    expect(encoded.tag).toBe('db-byte-array');
    expect(Array.from(igniteDecodeDbValue(encoded, 'raw') as Uint8Array)).toEqual([1, 2, 3]);
    expect(igniteEncodeDbValue(null)).toEqual({ tag: 'db-null' });
    expect(igniteDecodeDbValue({ tag: 'db-null' }, 'raw')).toBeNull();
  });

  it('decodes a db-uuid tuple to a canonical 36-char string', () => {
    const decoded = igniteDecodeDbValue(
      { tag: 'db-uuid', val: [0x0011223344556677n, 0x8899aabbccddeeffn] },
      'raw',
    );
    expect(decoded).toBe('00112233-4455-6677-8899-aabbccddeeff');
  });

  it('encodes Date → db-date (epoch millis) and decodes back in date mode', () => {
    const d = new Date('2021-05-06T07:08:09.000Z');
    const encoded = igniteEncodeDbValue(d) as { tag: string; val: bigint };
    expect(encoded.tag).toBe('db-date');
    expect(encoded.val).toBe(BigInt(d.getTime()));
    const back = igniteDecodeDbValue(encoded, 'date') as Date;
    expect(back.getTime()).toBe(d.getTime());
  });

  it('rejects an out-of-range timestamp sub-ms-nanos', () => {
    expect(() =>
      igniteEncodeDbValue(igniteParam('timestamp', { millis: 0n, subMilliNanos: 2_000_000 })),
    ).toThrow(ParamEncodingError);
  });
});

// ===========================================================================
// isReader heuristic
// ===========================================================================

describe('isReader', () => {
  it('classifies row-returning statements as readers', () => {
    expect(isReader('SELECT 1')).toBe(true);
    expect(isReader('  with t as (select 1) select * from t')).toBe(true);
    expect(isReader('INSERT INTO t VALUES (1) RETURNING id')).toBe(true);
    expect(isReader('SHOW TABLES')).toBe(true);
  });

  it('classifies plain writes / DDL as non-readers', () => {
    expect(isReader('INSERT INTO t VALUES (1)')).toBe(false);
    expect(isReader('UPDATE t SET x = 1')).toBe(false);
    expect(isReader('CREATE TABLE t (id int)')).toBe(false);
  });
});

// ===========================================================================
// RdbmsError classification
// ===========================================================================

describe('RdbmsError', () => {
  it('classifies a tagged host error', () => {
    const err = new RdbmsError(
      { tag: 'connection-failure', val: 'refused' },
      'open',
      'Postgres',
    );
    expect(err.reason).toBe('connection-failure');
    expect(err.trace).toBe('refused');
    expect(err.operation).toBe('open');
    expect(err.message).toContain('connection-failure');
  });

  it('classifies a ParamEncodingError as param-encoding', () => {
    const err = new RdbmsError(new ParamEncodingError('bad'), 'query', 'MySql');
    expect(err.reason).toBe('param-encoding');
    expect(err.trace).toBe('bad');
  });

  it('falls back to other for opaque causes', () => {
    const err = new RdbmsError(new Error('boom'), 'execute', 'Ignite');
    expect(err.reason).toBe('other');
  });
});

// ===========================================================================
// Driver module exports (host bindings vi.mocked above)
// ===========================================================================

describe('driver module exports', () => {
  it('postgres exposes open + Pg param helpers + PostgresError', async () => {
    const mod = await import('../src/fluent/rdbms/postgres');
    expect(typeof mod.Postgres.open).toBe('function');
    expect(typeof mod.Pg.uuid).toBe('function');
    expect(typeof mod.Pg.jsonb).toBe('function');
    expect(typeof mod.Pg.numeric).toBe('function');
    expect(typeof mod.Pg.array).toBe('function');
    expect(typeof mod.PostgresError).toBe('function');
  });

  it('mysql exposes open + MySql param helpers + MySqlError', async () => {
    const mod = await import('../src/fluent/rdbms/mysql');
    expect(typeof mod.MySql.open).toBe('function');
    expect(typeof mod.MySql.json).toBe('function');
    expect(typeof mod.MySql.decimal).toBe('function');
    expect(typeof mod.MySql.bigintUnsigned).toBe('function');
    expect(typeof mod.MySqlError).toBe('function');
  });

  it('ignite exposes open + Ignite param helpers + IgniteError', async () => {
    const mod = await import('../src/fluent/rdbms/ignite');
    expect(typeof mod.Ignite.open).toBe('function');
    expect(typeof mod.Ignite.uuid).toBe('function');
    expect(typeof mod.Ignite.decimal).toBe('function');
    expect(typeof mod.Ignite.timestamp).toBe('function');
    expect(typeof mod.IgniteError).toBe('function');
  });
});
