// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { SchemaValue } from './model';

type Cleanup = () => void | Promise<unknown>;
let active: Cleanup[] | undefined;

/** Register a take-once cleanup while synchronous native conversion is running. */
export function ownStream(cleanup: Cleanup): void {
  active?.push(cleanup);
}

/**
 * Native guest bridge conversion boundary. `convert` must be synchronous: stream
 * moves and lifts performed by it are retained until `handoff` succeeds. If either
 * fails, remaining owned endpoints are closed once, preserving the original error.
 * Successful results remain owned by the caller; this is not a stream pump.
 *
 * Keep awaits in `handoff`, not `convert`, so concurrent RPCs cannot acquire each
 * other's endpoints. Item callbacks use the same boundary lazily, when pulled.
 */
export async function withNativeStreamScope<T, R = T>(
  convert: () => T,
  handoff: (value: T) => R | Promise<R> = (value) => value as unknown as R,
): Promise<R> {
  const cleanups: Cleanup[] = [];
  const previous = active;
  try {
    let value: T;
    active = cleanups;
    try {
      value = convert();
    } finally {
      active = previous;
    }
    return await handoff(value);
  } catch (error) {
    for (const cleanup of cleanups.reverse()) {
      try {
        await cleanup();
      } catch {
        // Cleanup must not replace the original conversion or invocation failure.
      }
    }
    throw error;
  }
}

/**
 * Call inside `withNativeStreamScope` before decoding an already lifted result.
 * This also owns unread siblings that a failing typed decoder never reaches.
 */
export function ownSchemaValueStreams(value: SchemaValue | undefined): void {
  if (value === undefined) return;
  switch (value.tag) {
    case 'stream':
      ownStream(() => value.handle.close());
      break;
    case 'record':
      value.fields.forEach(ownSchemaValueStreams);
      break;
    case 'list':
    case 'fixed-list':
    case 'tuple':
      value.elements.forEach(ownSchemaValueStreams);
      break;
    case 'map':
      value.entries.forEach((entry) => {
        ownSchemaValueStreams(entry.key);
        ownSchemaValueStreams(entry.value);
      });
      break;
    case 'variant':
      ownSchemaValueStreams(value.payload);
      break;
    case 'option':
      ownSchemaValueStreams(value.value);
      break;
    case 'result':
      ownSchemaValueStreams(value.result.value);
      break;
    case 'union':
      ownSchemaValueStreams(value.body);
      break;
  }
}
