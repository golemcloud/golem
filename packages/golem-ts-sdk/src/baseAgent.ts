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

import { AgentType } from 'golem:agent/common';
import { AgentId } from './agentId';
import { AgentTypeRegistry } from './internal/registry/agentTypeRegistry';
import * as Option from './newTypes/option';
import { AgentClassName } from './newTypes/agentClassName';
import { Datetime } from 'golem:rpc/types@0.2.2';
import { Uuid } from 'golem:agent/host';

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
      const agentType = AgentTypeRegistry.get(this.agentClassName);

      if (Option.isNone(agentType)) {
        throw new Error(
          `Agent type metadata is not available for \`${this.constructor.name}\`. ` +
            `Ensure the class is decorated with @agent()`,
        );
      }

      this.cachedAgentType = agentType.val;
    }
    return this.cachedAgentType;
  }

  /**
   * Loads the agent's state from a previously saved binary snapshot produced by `saveSnapshot()`.
   * @param bytes The binary snapshot data.
   * @throws String Can throw a string describing the load error.
   */
  loadSnapshot(bytes: Uint8Array): Promise<void> {
    throw new Error(
      `\`loadSnapshot\` is not implemented for ${this.constructor.name}`,
    );
  }

  /**
   * Saves the agent's current state into a binary snapshot.
   */
  saveSnapshot(): Promise<Uint8Array> {
    throw new Error(
      `\`saveSnapshot\` is not implemented for ${this.constructor.name}`,
    );
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
   * The type of `remoteClient` is `WithRemoteMethods<MyAgent>` exposing more functionalities
   * such as `trigger` and `schedule`. See `WithRemoteMethods` documentation for details.
   *
   */
  static get<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    ...args: ConstructorParameters<T>
  ): WithRemoteMethods<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static getPhantom<
    T extends new (phantomId: Uuid | undefined, ...args: any[]) => BaseAgent,
  >(
    this: T,
    ...args: ConstructorParameters<T>
  ): WithRemoteMethods<InstanceType<T>> {
    throw new Error(
      `Remote client creation failed: \`${this.name}\` must be decorated with @agent()`,
    );
  }

  static newPhantom<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    ...args: ConstructorParameters<T>
  ): WithRemoteMethods<InstanceType<T>> {
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
 * const myAgent: WithRemoteMethods<MyAgent> = MyAgent.get("my-constructor-input")
 *
 * @agent()
 * class MyAgent extends BaseAgent {
 *
 *    constructor(readonly input: string) {}
 *
 *    function foo(input: string): Promise<void> {}
 * }
 *
 * // The type of myAgent is `WithRemoteMethods<MyAgent>` allowing you
 * // to call extra functionalities such as the following.
 *
 * myAgent.foo("my-input"); // normal invocation
 * myAgent.foo.trigger("my-input"); // fire and forget
 * myAgent.foo.schedule(scheduleTime, "my-input") // schedule an invocation
 *
 * ```
 */
export type WithRemoteMethods<T> = {
  [K in keyof T as T[K] extends (...args: any[]) => any
    ? K
    : never]: T[K] extends (...args: infer A) => infer R
    ? RemoteMethod<A, Awaited<R>>
    : never;
};

export type RemoteMethod<Args extends any[], R> = {
  (...args: Args): Promise<R>;
  trigger: (...args: Args) => void;
  schedule: (ts: Datetime, ...args: Args) => void;
};
