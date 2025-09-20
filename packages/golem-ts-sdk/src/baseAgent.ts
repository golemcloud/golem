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

/**
 * BaseAgent is the foundational class for defining agent implementations.
 *
 * All agents must extend this class and **must** be decorated with the `@Agent()` decorator.
 * Do **not** need to override the methods and manually implement them in this class.
 * The `@agent()` decorator handles all runtime wiring (e.g., `getId()`, `createRemote()`, etc.).
 *
 * Example usage:
 *
 * ```ts
 * @agent()
 * class AssistantAgent extends BaseAgent {
 *   @Prompt("Ask your question")
 *   @Description("This method allows the agent to answer your question")
 *   async ask(name: string): Promise<string> {
 *      return `Hello ${name}, I'm the assistant agent (${this.getId()})!`;
 *   }
 * }
 * ```
 */
export class BaseAgent {
  /**
   * Returns the unique `AgentId` for this agent instance.
   *
   * This is automatically populated by the `@agent()` decorator at runtime.
   *
   * @throws Will throw if accessed before the agent is initialized.
   */
  getId(): AgentId {
    throw new Error('An agent ID will be created at runtime');
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
    const agentClassName = new AgentClassName(this.constructor.name);

    const agentType = AgentTypeRegistry.lookup(agentClassName);

    if (Option.isNone(agentType)) {
      throw new Error(
        `Failed to find agent type for ${this.constructor.name}. Ensure it is decorated with @Agent() and registered properly.`,
      );
    }

    return agentType.val;
  }

  /**
   * Loads the agent's state from a previously saved binary snapshot produced by `saveSnapshot()`.
   * @param bytes The binary snapshot data.
   * @throws String Can throw a string describing the load error.
   */
  loadSnapshot(bytes: Uint8Array): Promise<void> {
    throw new Error('loadSnapshot is not implemented for this agent type');
  }

  /**
   * Saves the agent's current state into a binary snapshot.
   */
  saveSnapshot(): Promise<Uint8Array> {
    throw new Error('saveSnapshot is not implemented for this agent type');
  }

  /**
   * Gets a remote client instance of this agent type.
   *
   * This remote client will communicate with an agent instance running
   * in a separate container, effectively offloading computation to that remote context.
   *
   * @param args - Constructor arguments for the agent
   * @returns A remote proxy instance of the agent
   *
   * @example
   * const remoteClient = MyAgent.createRemote("arg1", "arg2") where `arg1`, `arg2` are the constructor arguments
   * validated at compile time.
   */
  static get<T extends new (...args: any[]) => BaseAgent>(
    this: T,
    ...args: ConstructorParameters<T>
  ): WithRemoteMethods<InstanceType<T>> {
    throw new Error('A remote client will be created at runtime');
  }
}

export type WithRemoteMethods<T> = {
  [K in keyof T]: T[K] extends (...args: infer A) => infer R
    ? RemoteMethod<A, Awaited<R>>
    : T[K];
};

export type RemoteMethod<Args extends any[], R> = {
  (...args: Args): Promise<R>;
  trigger: (...args: Args) => Promise<R>;
  schedule: (ts: number, ...args: Args) => Promise<R>;
};
