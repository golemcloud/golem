export class IncomingValue {
  constructor(private readonly bytes: Uint8Array) {}

  incomingValueConsumeSync(): Uint8Array {
    return this.bytes;
  }
}

export class OutgoingValue {
  bytes: Uint8Array = new Uint8Array(0);

  static newOutgoingValue(): OutgoingValue {
    return new OutgoingValue();
  }

  outgoingValueWriteBody(data: AsyncIterable<number>): void {
    __blobTestState.lastOutgoingBody = data;
    this.bytes = Uint8Array.from(data as AsyncIterable<number> & Iterable<number>);
  }
}

export class StreamObjectNames {
  constructor(private remaining: string[]) {}

  async *[Symbol.asyncIterator](): AsyncIterableIterator<string> {
    while (this.remaining.length > 0) {
      yield this.remaining.shift()!;
    }
  }

  readStreamObjectNames(len: bigint): [string[], boolean] {
    const take = this.remaining.splice(0, Number(len));
    return [take, this.remaining.length === 0];
  }

  skipStreamObjectNames(num: bigint): [bigint, boolean] {
    const n = Math.min(Number(num), this.remaining.length);
    this.remaining.splice(0, n);
    return [BigInt(n), this.remaining.length === 0];
  }
}

export const __blobTestState = {
  containers: new Map<string, Container>(),
  endExclusive: false,
  lastOutgoingBody: undefined as AsyncIterable<number> | undefined,
};

export class Container {
  objects = new Map<string, Uint8Array>();

  constructor(readonly cname: string) {}

  name(): string {
    return this.cname;
  }

  info(): { name: string; createdAt: bigint } {
    return { name: this.cname, createdAt: 1000n };
  }

  getData(name: string, start: bigint, end: bigint): IncomingValue {
    const data = this.objects.get(name);
    if (data === undefined) throw new Error('no such object');
    const endIndex = __blobTestState.endExclusive ? Number(end) : Number(end) + 1;
    return new IncomingValue(data.subarray(Number(start), endIndex));
  }

  writeData(name: string, value: OutgoingValue): void {
    this.objects.set(name, value.bytes);
  }

  listObjects(): StreamObjectNames {
    return new StreamObjectNames(Array.from(this.objects.keys()));
  }

  deleteObject(name: string): void {
    this.objects.delete(name);
  }

  deleteObjects(names: string[]): void {
    for (const name of names) this.objects.delete(name);
  }

  hasObject(name: string): boolean {
    return this.objects.has(name);
  }

  objectInfo(name: string): {
    name: string;
    container: string;
    createdAt: bigint;
    size: bigint;
  } {
    const data = this.objects.get(name);
    if (data === undefined) throw new Error('no such object');
    return { name, container: this.cname, createdAt: 2000n, size: BigInt(data.length) };
  }

  clear(): void {
    this.objects.clear();
  }
}

export function createContainer(name: string): Container {
  const existing = __blobTestState.containers.get(name);
  if (existing !== undefined) return existing;
  const container = new Container(name);
  __blobTestState.containers.set(name, container);
  return container;
}

export function getContainer(name: string): Container {
  const container = __blobTestState.containers.get(name);
  if (container === undefined) throw new Error('no such container');
  return container;
}

export function deleteContainer(name: string): void {
  __blobTestState.containers.delete(name);
}

export function containerExists(name: string): boolean {
  return __blobTestState.containers.has(name);
}

export function copyObject(): void {}

export function moveObject(): void {}
