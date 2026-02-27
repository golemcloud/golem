// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
import { AgentId } from './agentId';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import { AgentClassName } from './agentClassName';
import { Datetime } from 'wasi:clocks/wall-clock@0.2.3';
import { Uuid } from 'golem:agent/host@1.5.0';

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
  getId(): AgentId {
    throw new Error(
      `AgentId is not available for \`${this.constructor.name}\`. ` +
        `Ensure the class is decorated with @agent()`,
    );
  }

  /**
   * Returns this agent's phantom ID, if any
   */
  phantomId(): Uuid | undefined {
    const [_typeName, _params, phantomId] = this.getId().parsed();
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
   * from a JSON snapshot.
   *
   * @param bytes The snapshot data.
   * @throws String Can throw a string describing the load error.
   */
  async loadSnapshot(bytes: Uint8Array): Promise<void> {
    const text = new TextDecoder().decode(bytes);
    const state = JSON.parse(text) as Partial<this>;

    for (const [k, v] of Object.entries(state)) {
      if (k === 'cachedAgentType' || k === 'agentClassName') continue;
      this[k as keyof this] = v;
    }
  }

  /**
   * Saves the agent's current state into a snapshot.
   *
   * Override this method together with `loadSnapshot()` to implement a custom
   * snapshot format. If not overridden, the default implementation JSON-serializes
   * the agent's own state properties.
   *
   * Custom overrides can return either:
   * - `Uint8Array` — treated as a binary snapshot (`application/octet-stream`)
   * - `{ data: Uint8Array; mimeType: string }` — to specify the mime type explicitly.
   *   Use `application/json` for JSON snapshots or `application/octet-stream` for binary.
   */
  async saveSnapshot(): Promise<Uint8Array | { data: Uint8Array; mimeType: string }> {
    const state: Record<string, unknown> = {};

    for (const [k, v] of Object.entries(this)) {
      if (k === 'cachedAgentType' || k === 'agentClassName') continue;
      if (typeof v === 'function') continue;
      state[k] = v;
    }

    return {
      data: new TextEncoder().encode(JSON.stringify(state)),
      mimeType: 'application/json',
    };
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
  static get<T extends new (...args: never[]) => BaseAgent>(
    this: T,
    ...args: GetArgs<ConstructorParameters<T>>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static getPhantom<
    T extends new (phantomId: Uuid | undefined, ...args: never[]) => BaseAgent,
  >(
    this: T,
    ...args: ConstructorParameters<T>
  ): Client<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static newPhantom<T extends new (...args: never[]) => BaseAgent>(
    this: T,
    ...args: ConstructorParameters<T>
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
 * myAgent.foo.trigger("my-input"); // fire and forget
 * myAgent.foo.schedule(scheduleTime, "my-input") // schedule an invocation
 *
 * ```
 */
type MethodKeys<T> = {
  [K in keyof T]-?: T[K] extends (...args: never[]) => unknown ? K : never;
}[keyof T];

export type Client<T> = {
  [K in MethodKeys<T>]: T[K] extends (
    ...args: infer A
  ) => infer R
    ? RemoteMethod<GetArgs<A>, Awaited<R>>
    : never;
};

export type RemoteMethod<Args extends unknown[], R> = {
  (...args: Args): Promise<R>;
  trigger: (...args: Args) => void;
  schedule: (ts: Datetime, ...args: Args) => void;
};

// GetArgs extracts the argument types for the remote agent's get method
// by removing the Principal parameter
type GetArgs<T extends readonly unknown[]> =
  SplitOnPrincipal<T> extends {
    found: infer F extends boolean;
    before: infer B extends unknown[];
    after: infer A extends unknown[];
  }
    ? F extends true
      ? AllOptional<A> extends true
        ? B | [...B, ...A]
        : [...B, ...A]
      : T
    : never;

// Handles any trailing parameters (optional) after `Principal`
// See `tests/agentWithPrincipalAutoInjection.ts` for usage example
type IsOptional<T extends readonly unknown[], K extends keyof T> =
  {} extends Pick<T, K> ? true : false;

type AllOptional<T extends readonly unknown[], I extends unknown[] = []> = T extends readonly [
  unknown,
  ...infer R,
]
  ? IsOptional<T, I['length']> extends true
    ? AllOptional<R, [...I, 0]>
    : false
  : true;

type SplitOnPrincipal<
  T extends readonly unknown[],
  Before extends unknown[] = [],
> = T extends readonly [infer H, ...infer R]
  ? [H] extends [Principal]
    ? { found: true; before: Before; after: R }
    : SplitOnPrincipal<R, [...Before, H]>
  : { found: false; before: Before; after: [] };
