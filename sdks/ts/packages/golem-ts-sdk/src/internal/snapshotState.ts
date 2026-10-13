import { SnapshotError } from '../snapshot';

const marker = '$golemSnapshot';
const decimal = /^(?:0|-?[1-9][0-9]*)$/;

/** Encode state only; principal metadata and binary parts retain their own formats. */
export function encodeSnapshotState(
  value: unknown,
  path = 'state',
  ancestors = new Set<object>(),
): unknown {
  if (typeof value === 'bigint') return { [marker]: ['bigint', value.toString()] };
  if (
    value === undefined ||
    typeof value === 'function' ||
    typeof value === 'symbol' ||
    (typeof value === 'number' && !Number.isFinite(value))
  ) {
    throw `Snapshot ${path} is not JSON-deserializable; use custom save/load functions.`;
  }
  if (value === null || typeof value !== 'object') return value;
  if (ancestors.has(value)) {
    throw `Snapshot ${path} contains a cycle; use custom save/load functions.`;
  }
  if (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype) {
    throw `Snapshot ${path} is not a plain JSON value; use custom save/load functions.`;
  }
  ancestors.add(value);
  let encoded: unknown;
  if (Array.isArray(value)) {
    encoded = value.map((item, index) => encodeSnapshotState(item, `${path}.${index}`, ancestors));
  } else {
    const entries = Object.entries(value).map(([key, item]) => [
      key,
      encodeSnapshotState(item, `${path}.${key}`, ancestors),
    ]);
    encoded = Object.hasOwn(value, marker)
      ? { [marker]: ['object', entries] }
      : Object.fromEntries(entries);
  }
  ancestors.delete(value);
  return encoded;
}

export function decodeSnapshotState(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(decodeSnapshotState);
  if (value === null || typeof value !== 'object') return value;
  if (Object.hasOwn(value, marker) && Object.keys(value).length === 1) {
    const tag = (value as Record<string, unknown>)[marker];
    if (Array.isArray(tag) && tag.length === 2) {
      if (
        tag[0] === 'bigint' &&
        typeof tag[1] === 'string' &&
        decimal.exec(tag[1])?.[0] === tag[1]
      ) {
        return BigInt(tag[1]);
      }
      if (tag[0] === 'object' && Array.isArray(tag[1])) {
        const keys = new Set<string>();
        return Object.fromEntries(
          tag[1].map((entry: unknown) => {
            if (
              !Array.isArray(entry) ||
              entry.length !== 2 ||
              typeof entry[0] !== 'string' ||
              keys.has(entry[0])
            ) {
              throw new SnapshotError('invalid snapshot state object entry');
            }
            keys.add(entry[0]);
            return [entry[0], decodeSnapshotState(entry[1])];
          }),
        );
      }
    }
    throw new SnapshotError('invalid snapshot state tag');
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, decodeSnapshotState(item)]),
  );
}
