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

import {
  BaseAgent,
  agent,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  atomically,
  withRetryPolicy,
} from '@golemcloud/golem-ts-sdk';

@agent()
class RetryTest extends BaseAgent {
  constructor(private readonly _name: string) {
    super();
  }

  async withRetryPolicyTest(host: string, port: number): Promise<boolean> {
    const policy = NamedPolicy.named('retry-test', Policy.immediate().maxRetries(10));
    return withRetryPolicy(policy, () =>
      atomically(async () => {
        const response = await fetch(`http://${host}:${port}/attempt`);
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}`);
        }
        return true;
      }),
    );
  }

  async withStatusRetryPolicyTest(host: string, port: number): Promise<boolean> {
    const policy = NamedPolicy.named(
      'status-retry-test',
      Policy.immediate().maxRetries(10),
    ).appliesWhen(Predicate.eq(Props.statusCode, 500));

    return withRetryPolicy(policy, async () => {
      const response = await fetch(`http://${host}:${port}/attempt`);
      return response.ok;
    });
  }

  /**
   * Plain fetch + throw on !ok, with
   * NO `withRetryPolicy` and NO `atomically`.  All retry behaviour must
   * come from a `retryPolicyDefaults`-style policy defined at the
   * environment level (i.e. supplied to the executor through the
   * `EnvironmentStateService`).
   */
  async manifestStatusRetryTest(host: string, port: number): Promise<boolean> {
    const response = await fetch(`http://${host}:${port}/attempt`);
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    return true;
  }

  /**
   * Reproducer for the "V2" manifest-only status-code retry path with a
   * POST request that carries a JSON body. Mirrors the user-level pattern
   * used in the chaos-backend smoke test:
   *
   *   const res = await fetch(url, {
   *     method: 'POST',
   *     headers: { 'content-type': 'application/json' },
   *     body: JSON.stringify(payload),
   *   });
   *   if (!res.ok) throw new Error(...);
   *
   * The retry policy comes from the environment (no `withRetryPolicy`,
   * no `atomically`). The host's inline status-code retry path must
   * transparently re-issue the failing POST request — including its
   * `content-type` header and JSON body — until success.
   */
  async manifestStatusRetryPostTest(host: string, port: number): Promise<boolean> {
    const response = await fetch(`http://${host}:${port}/attempt-post`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orderId: 'ord_repro', amount: 29.99 }),
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    return true;
  }

  /**
   * Variant of `manifestStatusRetryPostTest` that targets a fixed path
   * (e.g. the user's chaos-backend `/payment/charge`) and uses the same
   * JSON body shape. Used to reproduce the V2 failure mode against the
   * actual Node.js chaos-backend.
   */
  async manifestStatusRetryPostPath(
    host: string,
    port: number,
    path: string,
  ): Promise<boolean> {
    const response = await fetch(`http://${host}:${port}${path}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ orderId: 'ord_repro', amount: 29.99, currency: 'USD' }),
    });
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`);
    }
    return true;
  }
}
