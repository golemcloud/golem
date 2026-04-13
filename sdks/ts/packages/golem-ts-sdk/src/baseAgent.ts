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

import { AgentType, Principal } from 'golem:agent/common@1.5.0';
import { ParsedAgentId } from './agentId';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { AgentClassName } from './agentClassName';
import { Datetime } from 'wasi:clocks/wall-clock@0.2.3';
import { Uuid } from './uuid';
import { getAgentId } from './internal/registry/agentId';
import { Config, Secret } from './agentConfig';
import {
  DatabaseSync,
  StatementSync,
  Session,
  SQLTagStore,
  serializeDatabaseSync,
  restoreDatabaseSync,
  isAutocommitDatabaseSync,
} from './internal/sqlite';
import { encodeMultipart, decodeMultipart, MultipartPart } from './internal/multipart';

/**
 * BaseAgent is the foundational class for defining agent implementations.
 *
 * **Important: ** Classes that extend `BaseAgent`  **must** be decorated with the `@agent()` decorator.
 * Do **not** need to override the methods and manually implement them in this class.
 * The `@agent()` decorator handles all runtime wiring (e.g., `getId()`, `createRemote()`, etc.).
 *
 * Example usage:
 *
 * ```ts
 * @agent()
 * class AssistantAgent extends BaseAgent {
 *   @prompt("Ask your question")
 *   @description("This method allows the agent to answer your question")
 *   async ask(name: string): Promise<string> {
 *      return `Hello ${name}, I'm the assistant agent (${this.getId()})!`;
 *   }
 * }
 * ```
 */
export class BaseAgent {
  readonly agentClassName = new AgentClassName(this.constructor.name);
  cachedAgentType: AgentType | undefined = undefined;

  /**
   * Returns the unique `AgentId` for this agent instance.
   *
   * Automatically set by the `@agent()` decorator at runtime.
   *
   * @throws Will throw if accessed before the agent is initialized.
   */
  getId(): ParsedAgentId {
    const agentId = getAgentId();

    if (!agentId) {
      throw new Error(
        `AgentId is not available for \`${this.constructor.name}\`. ` +
          `Ensure the class is decorated with @agent()`,
      );
    }

    return agentId;
  }

  /**
   * Returns this agent's phantom ID, if any
   */
  phantomId(): Uuid | undefined {
    const [, , phantomId] = this.getId().parsed();
    return phantomId;
  }

  /**
   * Returns the `AgentType` metadata registered for this agent.
   *
   * This information is retrieved from the runtime agent registry and reflects
   * metadata defined via decorators like `@Agent()`, `@Prompt()`, etc.
   *
   * @throws Will throw if metadata is missing or the agent is not properly registered.
   */
  getAgentType(): AgentType {
    if (!this.cachedAgentType) {
      const agentType: AgentType | undefined = AgentTypeRegistry.get(this.agentClassName);

      if (!agentType) {
        throw new Error(
          `Agent type metadata is not available for \`${this.constructor.name}\`. ` +
            `Ensure the class is decorated with @agent()`,
        );
      }

      this.cachedAgentType = agentType;
    }

    return this.cachedAgentType;
  }

  /**
   * Loads the agent's state from a previously saved snapshot produced by `saveSnapshot()`.
   *
   * Override this method together with `saveSnapshot()` to implement a fully custom binary
   * snapshot format. If not overridden, the default implementation restores the agent's state
   * from a JSON snapshot, and automatically restores any `DatabaseSync` fields from
   * multipart database parts.
   *
   * @param bytes The snapshot data.
   * @param mimeType The MIME type of the snapshot. Defaults to 'application/json' for backward compatibility.
   * @throws String Can throw a string describing the load error.
   */
  async loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void> {
    if (mimeType && mimeType.startsWith('multipart/mixed')) {
      const boundaryMatch = mimeType.match(/boundary=([^\s;]+)/);
      if (!boundaryMatch) {
        throw 'multipart/mixed snapshot missing boundary parameter';
      }
      const parts = decodeMultipart(bytes, boundaryMatch[1]);

      const statePart = parts.find((p) => p.name === 'state');
      if (!statePart) {
        throw 'multipart snapshot missing "state" part';
      }

      const state = JSON.parse(new TextDecoder().decode(statePart.body)) as Record<string, unknown>;
      for (const [k, v] of Object.entries(state)) {
        if (k === 'cachedAgentType' || k === 'agentClassName') continue;
        this[k as keyof this] = v as this[keyof this];
      }

      const dbParts = parts.filter((p) => p.name.startsWith('db:'));
      for (const dbPart of dbParts) {
        const propName = dbPart.name.slice(3);
        const field = (this as Record<string, unknown>)[propName];
        if (field === undefined) {
          console.warn(
            `Snapshot contains database "${propName}" but no matching property exists on the agent; skipping`,
          );
          continue;
        }
        if (!(field instanceof DatabaseSync)) {
          throw `Snapshot database part "${propName}" maps to a non-DatabaseSync property`;
        }
        restoreDatabaseSync(field, dbPart.body);
      }
    } else {
      const text = new TextDecoder().decode(bytes);
      const state = JSON.parse(text) as Record<string, unknown>;

      for (const [k, v] of Object.entries(state)) {
        if (k === 'cachedAgentType' || k === 'agentClassName') continue;
        this[k as keyof this] = v as this[keyof this];
      }
    }
  }

  /**
   * Saves the agent's current state into a snapshot.
   *
   * Override this method together with `loadSnapshot()` to implement a custom
   * snapshot format. If not overridden, the default implementation JSON-serializes
   * the agent's own state properties, and automatically includes any `DatabaseSync`
   * fields as binary multipart parts.
   *
   * Custom overrides can return either:
   * - `Uint8Array` — treated as a binary snapshot (`application/octet-stream`)
   * - `{ data: Uint8Array; mimeType: string }` — to specify the mime type explicitly.
   *   Use `application/json` for JSON snapshots or `application/octet-stream` for binary.
   */
  async saveSnapshot(): Promise<Uint8Array | { data: Uint8Array; mimeType: string }> {
    const state: Record<string, unknown> = {};
    const databases: Array<{ name: string; bytes: Uint8Array }> = [];
    const seenDbs = new Set<unknown>();

    for (const [k, v] of Object.entries(this)) {
      if (k === 'cachedAgentType' || k === 'agentClassName') continue;
      if (typeof v === 'function') continue;

      if (v instanceof DatabaseSync) {
        if (seenDbs.has(v)) {
          throw `Multiple agent fields reference the same DatabaseSync instance (field "${k}"). Each database must be stored in exactly one field.`;
        }
        seenDbs.add(v);

        if (!isAutocommitDatabaseSync(v)) {
          throw `Cannot snapshot database "${k}": an open transaction exists. Commit or rollback before saving.`;
        }

        const dbListStmt = v.prepare('PRAGMA database_list');
        const dbList = dbListStmt.all() as Array<{ name: string }>;
        const nonStandardSchemas = dbList.filter(
          (row) => row.name !== 'main' && row.name !== 'temp',
        );
        if (nonStandardSchemas.length > 0) {
          throw `Cannot snapshot database "${k}": ATTACH'd databases are not supported (found: ${nonStandardSchemas.map((r) => r.name).join(', ')})`;
        }

        databases.push({ name: k, bytes: serializeDatabaseSync(v) });
        continue;
      }

      if (v instanceof StatementSync || v instanceof Session || v instanceof SQLTagStore) {
        continue;
      }

      state[k] = v;
    }

    if (databases.length === 0) {
      return {
        data: new TextEncoder().encode(JSON.stringify(state)),
        mimeType: 'application/json',
      };
    }

    const parts: MultipartPart[] = [
      {
        name: 'state',
        contentType: 'application/json',
        body: new TextEncoder().encode(JSON.stringify(state)),
      },
    ];
    for (const db of databases) {
      parts.push({
        name: `db:${db.name}`,
        contentType: 'application/x-sqlite3',
        body: db.bytes,
      });
    }

    const { data, boundary } = encodeMultipart(parts);
    return {
      data,
      mimeType: `multipart/mixed; boundary=${boundary}`,
    };
  }

  /**
   * Serializes all `DatabaseSync` fields on this agent.
   * Useful for custom `saveSnapshot()` overrides that want to include database snapshots.
   */
  protected serializeTrackedDatabases(): Array<{ name: string; bytes: Uint8Array }> {
    const databases: Array<{ name: string; bytes: Uint8Array }> = [];
    const seenDbs = new Set<unknown>();

    for (const [k, v] of Object.entries(this)) {
      if (v instanceof DatabaseSync) {
        if (seenDbs.has(v)) {
          throw `Multiple agent fields reference the same DatabaseSync instance (field "${k}").`;
        }
        seenDbs.add(v);

        if (!isAutocommitDatabaseSync(v)) {
          throw `Cannot snapshot database "${k}": an open transaction exists.`;
        }

        const dbListStmt = v.prepare('PRAGMA database_list');
        const dbList = dbListStmt.all() as Array<{ name: string }>;
        const nonStandardSchemas = dbList.filter(
          (row) => row.name !== 'main' && row.name !== 'temp',
        );
        if (nonStandardSchemas.length > 0) {
          throw `Cannot snapshot database "${k}": ATTACH'd databases are not supported.`;
        }

        databases.push({ name: k, bytes: serializeDatabaseSync(v) });
      }
    }

    return databases;
  }

  /**
   * Restores `DatabaseSync` fields from previously serialized database snapshots.
   * Useful for custom `loadSnapshot()` overrides that include database snapshots.
   */
  protected restoreTrackedDatabases(databases: Array<{ name: string; bytes: Uint8Array }>): void {
    for (const { name, bytes } of databases) {
      const field = (this as Record<string, unknown>)[name];
      if (field === undefined) {
        console.warn(`restoreTrackedDatabases: no property "${name}" found on agent; skipping`);
        continue;
      }
      if (!(field instanceof DatabaseSync)) {
        throw `restoreTrackedDatabases: property "${name}" is not a DatabaseSync instance`;
      }
      restoreDatabaseSync(field, bytes);
    }
  }

  /**
   * Gets a remote client instance of this agent type.
   *
   * This remote client will communicate with an agent instance running
   * in a separate container, effectively offloading computation to that remote context.
   *
   *
   * @param args - Constructor arguments for the agent
   * @returns A remote proxy instance of the agent
   *
   * Example:
   *
   * ```ts
   *
   * @agent()
   * class MyAgent extends BaseAgent {
   *  constructor(arg1: string, arg2: number) { ... }
   *
   *  async myMethod(input: string): Promise<void> { ... }
   * }
   *
   * const remoteClient = MyAgent.get("arg1", "arg2")
   * remoteClient.myMethod("input")
   * ```
   *
   * The type of `remoteClient` is `Client<MyAgent>` exposing more functionalities
   * such as `trigger` and `schedule`. See `Client` documentation for details.
   *
   */
  static get<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgs<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static getWithConfig<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgsWithConfig<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static getPhantom<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    _phantomId: Uuid,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgs<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static getPhantomWithConfig<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    _phantomId: Uuid,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgsWithConfig<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static newPhantom<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgs<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static newPhantomWithConfig<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    ..._args: TransformGetArgsWithConfig<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }
}

/**
 *  Wrapper type of the remote agent obtained through `get` method
 *
 * Example:
 *
 * ```ts
 *
 * const myAgent: Client<MyAgent> = MyAgent.get("my-constructor-input")
 *
 * @agent()
 * class MyAgent extends BaseAgent {
 *
 *    constructor(readonly input: string) {}
 *
 *    function foo(input: string): Promise<void> {}
 * }
 *
 * // The type of myAgent is `Client<MyAgent>` allowing you
 * // to call extra functionalities such as the following.
 *
 * myAgent.foo("my-input"); // normal invocation
 * myAgent.foo.abortable(signal, "my-input"); // abortable invocation
 * myAgent.foo.trigger("my-input"); // fire and forget
 * myAgent.foo.schedule(scheduleTime, "my-input") // schedule an invocation
 *
 * ```
 */
type MethodKeys<T> = {
  [K in keyof T]-?: T[K] extends (...args: never[]) => unknown ? K : never;
}[keyof T];

export type Client<T> = {
  [K in MethodKeys<T>]: T[K] extends (...args: infer A) => infer R
    ? RemoteMethod<TransformMethodArgs<A>, Awaited<R>>
    : never;
};

export type RemoteMethod<Args extends unknown[], R> = {
  (...args: Args): Promise<R>;
  /**
   * Invoke the remote method with abort support. When the signal is aborted,
   * the returned promise rejects immediately, releasing the caller from waiting.
   *
   * **Important:** Aborting only cancels the local wait — the remote agent may
   * still execute the invoked method. Use this for racing multiple invocations
   * where you need the caller to proceed after the first result.
   */
  abortable: (signal: AbortSignal, ...args: Args) => Promise<R>;
  trigger: (...args: Args) => void;
  schedule: (ts: Datetime, ...args: Args) => void;
};

type IsPrincipal<T> = T extends Principal ? true : false;

type IsConfig<T> = T extends Config<any> ? true : false;

type TransformMethodArgs<T extends readonly unknown[]> = T extends readonly [
  infer Head,
  ...infer Tail,
]
  ? IsPrincipal<Head> extends true
    ? TransformMethodArgs<Tail>
    : [Head, ...TransformMethodArgs<Tail>]
  : T;

export type TransformGetArgs<T extends readonly unknown[]> = T extends readonly [
  infer Head,
  ...infer Tail,
]
  ? IsPrincipal<Head> extends true
    ? TransformGetArgs<Tail>
    : IsConfig<Head> extends true
      ? TransformGetArgs<Tail>
      : [Head, ...TransformGetArgs<Tail>]
  : T;

type TransformGetArgsWithConfig<
  T extends readonly unknown[],
  NonConfig extends unknown[] = [],
  Configs extends unknown[] = [],
> = T extends readonly [infer Head, ...infer Tail]
  ? IsPrincipal<Head> extends true
    ? TransformGetArgsWithConfig<Tail, NonConfig, Configs> // skip Principal
    : IsConfig<Head> extends true
      ? TransformGetArgsWithConfig<Tail, NonConfig, [...Configs, RpcConfigInput<Head>]>
      : TransformGetArgsWithConfig<Tail, [...NonConfig, Head], Configs>
  : [...NonConfig, ...Configs];

type RpcConfigInput<T> = T extends Config<infer C> ? RpcConfigInputInner<C> : T;

type RpcConfigInputInner<T> = T extends object
  ? { [K in keyof RemoveSecretFields<T>]?: RpcConfigInputInner<RemoveSecretFields<T>[K]> }
  : T;

type RemoveSecretFields<T> = {
  [K in keyof T as T[K] extends Secret<any> ? never : K]: T[K];
};
