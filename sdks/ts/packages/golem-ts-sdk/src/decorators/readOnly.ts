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

import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentMethodRegistry } from '../internal/registry/agentMethodRegistry';
import { CachePolicy } from 'golem:agent/common@2.0.0';
import ms from 'ms';

export type CachePolicyOption = 'no-cache' | 'until-write' | { ttl: string };

export type ReadOnlyOptions = {
  cache?: CachePolicyOption;
};

function parseDurationToNanoseconds(duration: string): bigint {
  const milliseconds = ms(duration as ms.StringValue);
  if (milliseconds === undefined) {
    throw new Error(
      `Invalid duration string: '${duration}'. Use formats like '5s', '10m', '1h', '2 days', etc.`,
    );
  }
  return BigInt(milliseconds) * 1_000_000n;
}

function resolveCachePolicy(option: CachePolicyOption | undefined): CachePolicy {
  if (option === undefined || option === 'until-write') {
    return { tag: 'until-write' };
  }
  if (option === 'no-cache') {
    return { tag: 'no-cache' };
  }
  if (typeof option === 'object' && 'ttl' in option) {
    return { tag: 'ttl', val: parseDurationToNanoseconds(option.ttl) };
  }
  throw new Error(`Invalid cache policy: ${JSON.stringify(option)}`);
}

/**
 * Marks an agent method as read-only.
 *
 * A read-only method does not modify the agent's state. The platform may use this
 * information to enable caching, side-effect detection, and to expose the method
 * via HTTP GET endpoints.
 *
 * `usesPrincipal` is derived automatically from the method signature: if any
 * parameter has type `Principal`, the resulting read-only configuration will be
 * marked as principal-dependent.
 *
 * `read-only` is not supported on `ephemeral` agents (they have no shared state
 * to read). Applying this decorator to a method of an ephemeral agent will throw
 * a build-time error during agent registration.
 *
 * @example
 * ```ts
 * @agent()
 * class Counter {
 *   private count = 0;
 *
 *   @readonly()
 *   getCount(): number { return this.count; }
 *
 *   @readonly({ cache: 'no-cache' })
 *   getLive(): number { return this.count; }
 *
 *   @readonly({ cache: { ttl: '30s' } })
 *   getCached(): number { return this.count; }
 * }
 * ```
 */
export function readonly(options?: ReadOnlyOptions) {
  return function (
    target: Object,
    propertyKey: string | symbol,
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    _descriptor?: PropertyDescriptor,
  ) {
    const className = target.constructor.name;

    const classMetadata = TypeMetadata.get(className);
    if (!classMetadata) {
      throw new Error(
        `Class metadata not found for agent ${className}. Ensure metadata is generated.`,
      );
    }

    const methodName = String(propertyKey);
    const cachePolicy = resolveCachePolicy(options?.cache);
    AgentMethodRegistry.setReadOnly(className, methodName, cachePolicy);
  };
}
