// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { Datetime } from 'golem:core/types@2.0.0';
import { deepEqual, type SchemaGraph, type TypedSchemaValue } from '../internal/schema-model';
import { schemaValueConforms } from '../internal/tool/validation';

export function typedSchemaValueConforms(
  expectedGraph: SchemaGraph,
  typed: TypedSchemaValue,
): boolean {
  return (
    deepEqual(typed.graph, expectedGraph) &&
    schemaValueConforms(expectedGraph, expectedGraph.root, typed.value)
  );
}

/** Convert an ISO-8601 instant to the nanosecond-precise schema datetime shape. */
export function datetimeFromISOString(value: string): Datetime {
  const match = value.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/);
  if (!match) throw new Error(`Invalid UTC datetime '${value}'`);
  const wholeSeconds = `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}`;
  const milliseconds = Date.parse(`${wholeSeconds}Z`);
  if (!Number.isFinite(milliseconds)) throw new Error(`Invalid datetime '${value}'`);
  if (new Date(milliseconds).toISOString().slice(0, 19) !== wholeSeconds) {
    throw new Error(`Invalid datetime '${value}'`);
  }
  return {
    seconds: BigInt(milliseconds / 1000),
    nanoseconds: Number((match[7] ?? '').padEnd(9, '0') || 0),
  };
}

/** Convert a schema datetime to its canonical ISO-8601 UTC representation. */
export function datetimeToISOString(value: Datetime): string {
  if (
    !Number.isFinite(value.nanoseconds) ||
    !Number.isInteger(value.nanoseconds) ||
    value.nanoseconds < 0 ||
    value.nanoseconds >= 1_000_000_000
  ) {
    throw new Error(`Datetime nanoseconds out of range: ${value.nanoseconds}`);
  }
  const minimumSeconds = -62_167_219_200n;
  const maximumSeconds = 253_402_300_799n;
  if (value.seconds < minimumSeconds || value.seconds > maximumSeconds) {
    throw new Error(`Datetime seconds outside canonical year range 0000..9999: ${value.seconds}`);
  }
  const date = new Date(Number(value.seconds * 1000n));
  if (!Number.isFinite(date.getTime())) {
    throw new Error(`Invalid datetime seconds: ${value.seconds}`);
  }
  const base = date.toISOString().replace('.000Z', '');
  const fraction =
    value.nanoseconds === 0
      ? ''
      : `.${String(value.nanoseconds).padStart(9, '0').replace(/0+$/, '')}`;
  return `${base}${fraction}Z`;
}
