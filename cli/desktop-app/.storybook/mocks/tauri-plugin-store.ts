export class Store {
  private data: Map<string, unknown> = new Map();

  async get(key: string): Promise<unknown> {
    return this.data.get(key) ?? null;
  }

  async set(key: string, value: unknown): Promise<void> {
    this.data.set(key, value);
  }

  async save(): Promise<void> {}

  async delete(key: string): Promise<boolean> {
    return this.data.delete(key);
  }

  async clear(): Promise<void> {
    this.data.clear();
  }

  async keys(): Promise<string[]> {
    return [...this.data.keys()];
  }

  async values(): Promise<unknown[]> {
    return [...this.data.values()];
  }

  async entries(): Promise<[string, unknown][]> {
    return [...this.data.entries()];
  }

  async length(): Promise<number> {
    return this.data.size;
  }

  async has(key: string): Promise<boolean> {
    return this.data.has(key);
  }
}

export async function load(): Promise<Store> {
  return new Store();
}
