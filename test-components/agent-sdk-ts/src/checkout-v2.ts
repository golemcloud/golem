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

import { BaseAgent, agent, atomically } from '@golemcloud/golem-ts-sdk';

// ---------------------------------------------------------------------------
// CheckoutAgentV2 — verbatim port of the user's `craft/golem` test agent.
//
// Same business logic as CheckoutAgent (V1). All retry behavior lives
// entirely in the application manifest:
//
//   - 5xx responses are retried by the host via the `http-5xx-retry` named
//     policy (status-code is referenced in the predicate, so the host
//     re-sends the request transparently before `fetch()` resolves to user
//     code).
//   - Aborted requests (the AbortController timeout below) surface as a
//     thrown AbortError; the resulting trap is retried by Golem's default
//     trap-context retry policy, which replays the agent from the oplog and
//     re-issues the pending fetch.
//
// Differences from the user's source:
//   - The chaos backend host:port is taken as part of the `checkout`
//     arguments instead of being hard-coded to `http://localhost:4000` —
//     this lets the integration test bind the chaos backend on an
//     ephemeral port.
//   - Order fields are passed as individual primitives instead of as a
//     single `Order` record so the Rust test side can use the
//     `data_value!` macro without derives.
// ---------------------------------------------------------------------------

// 10s per-request deadline — analogous to Temporal's startToCloseTimeout
// and identical to the user's V2 source.
const REQUEST_TIMEOUT_MS = 10_000;

async function post(api: string, path: string, body: unknown): Promise<any> {
  console.log(`POST ${path}`);
  // Wrap the timer + fetch + throw in `atomically` so that an AbortController
  // timeout (or any other thrown error) is retried *inside the same wasm
  // instance* — no trap, no oplog replay. Without this, replay would re-read
  // the original `setTimeout`'s persisted wake time (already in the past) and
  // fire `ac.abort()` immediately on every retry, never giving the next live
  // fetch a chance to wait the full timeout window.
  return atomically(async () => {
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), REQUEST_TIMEOUT_MS);
    try {
      const res = await fetch(`${api}${path}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(body),
        signal: ac.signal,
      });
      if (!res.ok) {
        // With the manifest's status-code retry policy, the host re-sends the
        // request before this point on 5xx — so reaching here means either
        // the policy's retries were exhausted or it didn't match.
        throw new Error(`${path} failed: ${res.status} ${await res.text()}`);
      }
      return await res.json();
    } finally {
      clearTimeout(timer);
    }
  });
}

@agent()
class CheckoutAgentV2 extends BaseAgent {
  private readonly orderId: string;

  constructor(orderId: string) {
    super();
    this.orderId = orderId;
  }

  /// Mirrors the user's CheckoutAgentV2.checkout exactly — four sequential
  /// POSTs with 10s AbortController timeouts, JSON bodies, and `throw on
  /// !ok`. Returns `true` on success so the test can use a simple
  /// `Value::Bool` assertion.
  async checkout(
    host: string,
    port: number,
    customerEmail: string,
    amount: number,
    address: string,
    sku: string,
    qty: number,
  ): Promise<boolean> {
    const api = `http://${host}:${port}`;
    const orderId = this.orderId;

    const inv = await post(api, '/inventory/reserve', {
      orderId,
      items: [{ sku, qty }],
    });

    await post(api, '/payment/charge', {
      orderId,
      amount,
      currency: 'USD',
    });

    const ship = await post(api, '/shipment/create', {
      orderId,
      reservationId: inv.reservationId,
      address,
    });

    await post(api, '/email/send', {
      orderId,
      to: customerEmail,
      subject: `Your order ${orderId} is on its way`,
      body: `Tracking: ${ship.trackingNumber}`,
    });

    return true;
  }
}
