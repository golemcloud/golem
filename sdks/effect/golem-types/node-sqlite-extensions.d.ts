// Type augmentations for wasm-rquickjs extensions to node:sqlite.
// These symbols exist at runtime in the wasm-rquickjs environment
// but are not part of @types/node.

declare module 'node:sqlite' {
  /** Session class — implemented as a class in wasm-rquickjs (interface in @types/node) */
  export class Session {
    changeset(): Uint8Array;
    patchset(): Uint8Array;
    close(): void;
    [Symbol.dispose](): void;
  }

  /** SQLTagStore — tagged template literal query cache (wasm-rquickjs only) */
  export class SQLTagStore {
    constructor(db: DatabaseSync, maxSize?: number);
    get(strings: TemplateStringsArray, ...values: unknown[]): unknown;
    all(strings: TemplateStringsArray, ...values: unknown[]): unknown[];
    run(strings: TemplateStringsArray, ...values: unknown[]): unknown;
    iterate(strings: TemplateStringsArray, ...values: unknown[]): IterableIterator<unknown>;
    clear(): void;
    readonly size: number;
    readonly capacity: number;
    readonly db: DatabaseSync;
  }

  /** Serialize a DatabaseSync instance to raw SQLite database bytes */
  export function serializeDatabaseSync(db: DatabaseSync): Uint8Array;

  /** Restore a DatabaseSync instance from raw SQLite database bytes */
  export function restoreDatabaseSync(db: DatabaseSync, bytes: Uint8Array): void;

  /** Check if a DatabaseSync instance is in autocommit mode (no open transaction) */
  export function isAutocommitDatabaseSync(db: DatabaseSync): boolean;
}
