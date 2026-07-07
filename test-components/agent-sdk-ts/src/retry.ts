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

import { z } from 'zod';
import {
  defineAgent,
  method,
  NamedPolicy,
  Policy,
  Predicate,
  Props,
  atomically,
  withRetryPolicy,
} from '@golemcloud/golem-ts-sdk';

export const RetryTest = defineAgent({
  name: 'RetryTest',
  id: { _name: z.string() },
  methods: {
    withRetryPolicyTest: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    withStatusRetryPolicyTest: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryTest: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryPostTest: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryPostPath: method({
      input: { host: z.string(), port: z.number(), path: z.string() },
      returns: z.boolean(),
    }),
    manifestStatusRetryTwoStepGet: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryOkThenHang: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryOkThenCrash: method({
      input: { host: z.string(), port: z.number() },
      returns: z.boolean(),
    }),
    manifestStatusRetryV2OkThenForever500: method({
      input: { host: z.string(), port: z.number(), requestTimeoutMs: z.number() },
      returns: z.boolean(),
    }),
  },
});

export const RetryTestImpl = RetryTest.implement({
  init: () => ({}),
  methods: {
    async withRetryPolicyTest({ host, port }) {
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
    },

    async withStatusRetryPolicyTest({ host, port }) {
      const policy = NamedPolicy.named(
        'status-retry-test',
        Policy.immediate().maxRetries(10),
      ).appliesWhen(Predicate.eq(Props.statusCode, 500));

      return withRetryPolicy(policy, async () => {
        const response = await fetch(`http://${host}:${port}/attempt`);
        return response.ok;
      });
    },

    /**
     * Plain fetch + throw on !ok, with
     * NO `withRetryPolicy` and NO `atomically`.  All retry behaviour must
     * come from a `retryPolicyDefaults`-style policy defined at the
     * environment level (i.e. supplied to the executor through the
     * `EnvironmentStateService`).
     */
    async manifestStatusRetryTest({ host, port }) {
      const response = await fetch(`http://${host}:${port}/attempt`);
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      return true;
    },

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
    async manifestStatusRetryPostTest({ host, port }) {
      const response = await fetch(`http://${host}:${port}/attempt-post`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ orderId: 'ord_repro', amount: 29.99 }),
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      return true;
    },

    /**
     * Variant of `manifestStatusRetryPostTest` that targets a fixed path
     * (e.g. the user's chaos-backend `/payment/charge`) and uses the same
     * JSON body shape. Used to reproduce the V2 failure mode against the
     * actual Node.js chaos-backend.
     */
    async manifestStatusRetryPostPath({ host, port, path }) {
      const response = await fetch(`http://${host}:${port}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ orderId: 'ord_repro', amount: 29.99, currency: 'USD' }),
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      return true;
    },

    /**
     * Two sequential GETs in a single invocation, both to endpoints that
     * fail N times then return 200. Reproduces the V2 regression where the
     * host's inline status-code retry only fires on the FIRST matching
     * failure within an invocation; subsequent failures escape to the guest
     * and trigger the default trap-retry path. After the fix, both GETs
     * must succeed transparently via the manifest `http-5xx-retry` policy.
     */
    async manifestStatusRetryTwoStepGet({ host, port }) {
      const a = await fetch(`http://${host}:${port}/step-a`);
      if (!a.ok) {
        throw new Error(`/step-a failed: ${a.status}`);
      }
      const b = await fetch(`http://${host}:${port}/step-b`);
      if (!b.ok) {
        throw new Error(`/step-b failed: ${b.status}`);
      }
      return true;
    },

    /**
     * GET `/ok` (returns 200), then GET `/hang` (server reads the request
     * but never sends a response). Reproduces the V2 S2 ("hanging
     * shipment") failure mode: an in-flight request that times out at the
     * host's `first_byte_timeout` traps the worker, which then replays
     * the prior step and re-issues the doomed second call. After the fix,
     * the request count for `/hang` must be bounded by the worker's
     * `RetryConfig::max_attempts` rather than blowing up into thousands.
     *
     * The host MUST be configured with a short `first_byte_timeout` for
     * this test to terminate within a reasonable wall-clock budget.
     */
    async manifestStatusRetryOkThenHang({ host, port }) {
      const a = await fetch(`http://${host}:${port}/ok`);
      if (!a.ok) {
        throw new Error(`/ok failed: ${a.status}`);
      }
      const b = await fetch(`http://${host}:${port}/hang`);
      if (!b.ok) {
        throw new Error(`/hang failed: ${b.status}`);
      }
      return true;
    },

    /**
     * GET `/ok` (returns 200), then GET `/crash` (server accepts the
     * connection, reads the request headers, then drops the socket without
     * sending a response). Reproduces the V2 S3 ("process-crash mid-call")
     * failure mode where each replay re-issues the doomed second call.
     * After the fix, the request count for `/crash` must be bounded by the
     * worker's `RetryConfig::max_attempts`.
     */
    async manifestStatusRetryOkThenCrash({ host, port }) {
      const a = await fetch(`http://${host}:${port}/ok`);
      if (!a.ok) {
        throw new Error(`/ok failed: ${a.status}`);
      }
      const b = await fetch(`http://${host}:${port}/crash`);
      if (!b.ok) {
        throw new Error(`/crash failed: ${b.status}`);
      }
      return true;
    },

    /**
     * Mirrors the user's V2 chaos-test agent (CheckoutAgentV2):
     *
     *   - Two sequential POST requests with JSON bodies.
     *   - Each POST is bounded by `AbortController.signal` with a per-request
     *     deadline (`requestTimeoutMs`).
     *   - The first POST hits `/ok-post` (always 200).
     *   - The second POST hits `/perma-500` (always returns 500).
     *
     * With a manifest `http-5xx-retry` policy in place (maxRetries large), the
     * host transparently re-sends the second POST many times before fetch()
     * resolves to user code. When the policy budget is exhausted, fetch returns
     * the final 500, the agent throws, and the worker's trap-replay path
     * decides whether to retry the whole invocation.
     *
     * Reproduces the V2 "tight retry loop" symptom: the inline status-code
     * retry budget alone produces a large observed request count to
     * `/perma-500`.
     */
    async manifestStatusRetryV2OkThenForever500({ host, port, requestTimeoutMs }) {
      const ok = await postWithTimeout(
        `http://${host}:${port}/ok-post`,
        { step: 'ok' },
        requestTimeoutMs,
      );
      if (!ok.ok) {
        throw new Error(`/ok-post failed: ${ok.status}`);
      }
      const bad = await postWithTimeout(
        `http://${host}:${port}/perma-500`,
        { step: 'perma' },
        requestTimeoutMs,
      );
      if (!bad.ok) {
        throw new Error(`/perma-500 failed: ${bad.status}`);
      }
      return true;
    },
  },
});

async function postWithTimeout(
  url: string,
  body: unknown,
  timeoutMs: number,
): Promise<Response> {
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), timeoutMs);
  try {
    return await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
      signal: ac.signal,
    });
  } finally {
    clearTimeout(timer);
  }
}
