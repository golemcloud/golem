import { SchemaValueStream, type SchemaValueTree } from 'golem:core/types@2.0.0';
import { ownStream } from './streamScope';

export type GuestSchemaValueStream =
  | { kind: 'wrapped'; value: SchemaValueStream }
  | { kind: 'native'; value: AsyncIterable<SchemaValueTree> };

export class GuestSchemaValueStreamHandle {
  #value: GuestSchemaValueStream | undefined;

  constructor(value: GuestSchemaValueStream) {
    this.#value = value;
    ownStream(() => this.close());
  }

  peek(): GuestSchemaValueStream | undefined {
    return this.#value;
  }

  async close(): Promise<void> {
    const endpoint = this.take();
    if (endpoint?.kind === 'wrapped') {
      const source = await SchemaValueStream.unwrap(endpoint.value);
      await source[Symbol.asyncIterator]().return?.();
    } else if (endpoint?.kind === 'native') {
      await endpoint.value[Symbol.asyncIterator]().return?.();
    }
  }

  take(): GuestSchemaValueStream | undefined {
    const value = this.#value;
    this.#value = undefined;
    return value;
  }
}
