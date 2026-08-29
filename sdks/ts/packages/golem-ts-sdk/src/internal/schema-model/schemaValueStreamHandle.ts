import type { SchemaValueStream, SchemaValueTree } from 'golem:core/types@2.0.0';

export type GuestSchemaValueStream =
  | { kind: 'wrapped'; value: SchemaValueStream }
  | { kind: 'native'; value: AsyncIterable<SchemaValueTree> };

export class GuestSchemaValueStreamHandle {
  #value: GuestSchemaValueStream | undefined;

  constructor(value: GuestSchemaValueStream) {
    this.#value = value;
  }

  peek(): GuestSchemaValueStream | undefined {
    return this.#value;
  }

  take(): GuestSchemaValueStream | undefined {
    const value = this.#value;
    this.#value = undefined;
    return value;
  }
}
