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

import type { Uuid } from './uuid';

export interface AgentClientIdentity {
  readonly agentId: string;
  readonly phantomId?: Uuid;
}

const CLIENT_IDENTITIES = Symbol.for('@golemcloud/golem-ts-sdk/client-identities');
const globalState = globalThis as unknown as Record<symbol, unknown>;
const clientIdentities = (globalState[CLIENT_IDENTITIES] ??= new WeakMap()) as WeakMap<
  object,
  AgentClientIdentity
>;

/** Read a client's identity without reserving a method name on the client proxy. */
export function clientIdentity(client: object): AgentClientIdentity | undefined {
  return clientIdentities.get(client);
}

/** @internal Shared by the typed and reflected client implementations. */
export function registerClientIdentity(client: object, identity: AgentClientIdentity): void {
  clientIdentities.set(client, Object.freeze(identity));
}
