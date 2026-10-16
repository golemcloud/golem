/** An opaque application-owned part. Names are case-sensitive, not paths. */
export interface SnapshotPart {
  readonly bytes: Uint8Array;
  readonly contentType: string;
}

/** Whole-buffer snapshot projection, independent of resource-bearing live state. */
export interface MultipartSnapshot<State> {
  readonly state: State;
  readonly parts: ReadonlyMap<string, SnapshotPart>;
}

export class SnapshotError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SnapshotError';
  }
}

/** @internal */
export function normalizeContentType(contentType: string): string {
  if (!/^[A-Za-z0-9!#$%&'*+.^_`|~-]+\/[A-Za-z0-9!#$%&'*+.^_`|~-]+$/.test(contentType)) {
    throw new SnapshotError(`invalid snapshot part Content-Type '${contentType}'`);
  }
  return contentType.toLowerCase();
}

/** @internal */
export function validatePartName(name: string): void {
  if (!/^[A-Za-z0-9_][A-Za-z0-9_.-]*$/.test(name)) {
    throw new SnapshotError(`invalid snapshot part name '${name}'`);
  }
}

/** Require a named part with a bare MIME type; returns its checked bytes. */
export function requirePart(
  parts: ReadonlyMap<string, SnapshotPart>,
  name: string,
  expectedContentType: string,
): Uint8Array {
  const part = parts.get(name);
  if (!part) throw new SnapshotError(`missing required part '${name}'`);
  if (normalizeContentType(part.contentType) !== normalizeContentType(expectedContentType)) {
    throw new SnapshotError(`part '${name}' has unexpected Content-Type '${part.contentType}'`);
  }
  return part.bytes;
}

export const Snapshot = { requirePart };
